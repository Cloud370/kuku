import { useStore } from 'zustand';
import { createStore, type StoreApi } from 'zustand/vanilla';

import type {
  ApiError,
  CommandAccepted,
  CreateTaskRequest,
  CreateTaskResponse,
  InteractionId,
  InteractionResponseRequest,
  RunId,
  StopRunRequest,
  SubmitRunRequest,
  SubmitRunResponse,
  TaskDelta,
  TaskProjection,
  TaskStreamEvent,
  TimelineQuery,
} from '../api/generated';
import { ContractDecodeError } from '../api/decode';
import { WebApiError, webApi } from '../api/client';

import { newIdempotencyKey } from './idempotency';
import {
  ProjectionGapError,
  applyProjection,
  applyStreamEvent,
  beginTaskSubscription,
  createWorkbenchSnapshot,
  type LocalDraft,
  type WorkbenchSnapshot,
} from './state';
import { consumeTaskStream } from './subscription';
import { prependTimelinePage, returnToRecent, timelineHistoryError } from './timelineHistory';

type WebApi = typeof webApi;
type CommandStatus = 'pending' | 'conflicted';

export type PendingCommand =
  | {
      kind: 'create_task';
      status: CommandStatus;
      body: CreateTaskRequest;
    }
  | {
      kind: 'submit_run';
      status: CommandStatus;
      taskId: string;
      body: SubmitRunRequest;
    }
  | {
      kind: 'stop_run';
      status: CommandStatus;
      taskId: string;
      runId: RunId;
      body: StopRunRequest;
    }
  | {
      kind: 'respond';
      status: CommandStatus;
      taskId: string;
      interactionId: InteractionId;
      body: InteractionResponseRequest;
    };

export type SubmitRunInput = Pick<SubmitRunRequest, 'message' | 'tier_id' | 'skill_ids'>;
export type PendingCommandResult =
  | CreateTaskResponse
  | SubmitRunResponse
  | CommandAccepted
  | undefined;

export interface WorkbenchStoreState {
  snapshot: WorkbenchSnapshot;
  pendingCommand: PendingCommand | null;
  setDraft(draft: LocalDraft): void;
  loadTask(taskId: string): Promise<TaskProjection>;
  subscribeTask(taskId: string): () => void;
  reconnectFromCursor(): Promise<void>;
  acceptFrame(event: TaskStreamEvent): void;
  setTaskDeltaObserver(observer: TaskDeltaObserver | null): void;
  loadOlder(): Promise<void>;
  returnToRecent(): void;
  createTask(workspaceId: string): Promise<CreateTaskResponse>;
  submitRun(input: SubmitRunInput): Promise<PendingCommandResult>;
  retryPendingCommand(): Promise<PendingCommandResult>;
  abandonConflictedCommand(): void;
  stopRun(): Promise<PendingCommandResult>;
  respond(interactionId: string, choiceId: string): Promise<PendingCommandResult>;
}

export type TaskDeltaObserver = (taskId: string, delta: TaskDelta) => void;

export function createWorkbenchStore(api: WebApi = webApi): StoreApi<WorkbenchStoreState> {
  let subscriptionController: AbortController | null = null;
  let observer: TaskDeltaObserver | null = null;
  let stoppedRunId: RunId | null = null;

  const store = createStore<WorkbenchStoreState>((set, get) => {
    const acceptFrame = (event: TaskStreamEvent): void => {
      const next = applyStreamEvent(get().snapshot, event);
      set({ snapshot: next });
      observer?.(event.task_id, event.event);
    };

    const recoverProjection = async (taskId: string): Promise<TaskProjection> => {
      const localDraft = get().snapshot.localDraft;
      const projection = await api.tasks.get(taskId);
      const snapshot = applyProjection(get().snapshot, projection);
      snapshot.localDraft = localDraft;
      set({ snapshot });
      return projection;
    };

    const consumeOnce = async (
      taskId: string,
      after: number | null,
      controller: AbortController,
    ): Promise<void> => {
      set({ snapshot: beginTaskSubscription(get().snapshot, taskId) });
      await consumeTaskStream(taskId, after, controller.signal, acceptFrame, api.tasks.subscribe);
    };

    const handleCommandFailure = async (
      pending: PendingCommand,
      error: unknown,
    ): Promise<never> => {
      if (error instanceof WebApiError && error.code === 'idempotency_conflict') {
        set({ pendingCommand: { ...pending, status: 'conflicted' } });
        if (pending.kind === 'create_task') {
          await ignoreFailure(
            api.tasks.list({
              workspace_id: pending.body.workspace_id,
              search: null,
              cursor: null,
              limit: 100,
            }),
          );
        } else {
          await ignoreFailure(recoverProjection(pending.taskId));
        }
      } else if (error instanceof WebApiError && isDefinitiveCommandError(error.code)) {
        set({ pendingCommand: null });
        if (pending.kind === 'stop_run') stoppedRunId = null;
        if (pending.kind !== 'create_task') {
          await ignoreFailure(recoverProjection(pending.taskId));
        }
      }
      throw error;
    };

    const executePendingCommand = async (
      pending: PendingCommand,
    ): Promise<Exclude<PendingCommandResult, undefined>> => {
      try {
        switch (pending.kind) {
          case 'create_task': {
            const result = await api.tasks.create(pending.body);
            const selected = beginTaskSubscription(get().snapshot, result.projection.task.task_id);
            set({
              snapshot: applyProjection(selected, result.projection),
              pendingCommand: null,
            });
            return result;
          }
          case 'submit_run': {
            const result = await api.tasks.submitRun(pending.taskId, pending.body);
            const snapshot = get().snapshot;
            set({
              snapshot: {
                ...snapshot,
                localDraft: { text: '', skillIds: [], tierId: null },
              },
              pendingCommand: null,
            });
            return result;
          }
          case 'stop_run': {
            const result = await api.tasks.stopRun(pending.taskId, pending.body);
            stoppedRunId = pending.runId;
            set({ pendingCommand: null });
            return result;
          }
          case 'respond': {
            const result = await api.tasks.respond(
              pending.taskId,
              pending.interactionId,
              pending.body,
            );
            set({ pendingCommand: null });
            return result;
          }
        }
      } catch (error) {
        return handleCommandFailure(pending, error);
      }
    };

    return {
      snapshot: createWorkbenchSnapshot(),
      pendingCommand: null,
      setDraft(draft) {
        set({ snapshot: { ...get().snapshot, localDraft: structuredClone(draft) } });
      },
      async loadTask(taskId) {
        subscriptionController?.abort();
        subscriptionController = null;
        stoppedRunId = null;
        set({ snapshot: beginTaskSubscription(get().snapshot, taskId) });
        const projection = await api.tasks.get(taskId);
        if (get().snapshot.selectedTaskId === taskId) {
          set({ snapshot: applyProjection(get().snapshot, projection) });
        }
        return projection;
      },
      subscribeTask(taskId) {
        subscriptionController?.abort();
        const controller = new AbortController();
        subscriptionController = controller;
        void runSubscriptionLoop(api, store, taskId, controller, acceptFrame, recoverProjection);
        return () => {
          controller.abort();
        };
      },
      async reconnectFromCursor() {
        const taskId = get().snapshot.selectedTaskId;
        if (taskId === null) return;
        subscriptionController?.abort();
        const controller = new AbortController();
        subscriptionController = controller;
        const after = get().snapshot.cursor;
        try {
          await consumeOnce(taskId, after, controller);
        } catch (error) {
          if (!isProjectionRecoveryError(error)) throw error;
          const projection = await recoverProjection(taskId);
          await consumeOnce(taskId, projection.cursor, controller);
        }
      },
      acceptFrame,
      setTaskDeltaObserver(nextObserver) {
        observer = nextObserver;
      },
      async loadOlder() {
        const initial = get().snapshot;
        const taskId = initial.selectedTaskId;
        const before = initial.timelineHistory.nextCursor;
        if (taskId === null || before === null || initial.timelineHistory.phase === 'loading')
          return;
        const generation = initial.timelineHistory.generation;
        set({
          snapshot: {
            ...initial,
            timelineHistory: { ...initial.timelineHistory, phase: 'loading', error: null },
          },
        });
        try {
          const query = { before, limit: 500 } satisfies TimelineQuery;
          const page = await api.tasks.timeline(taskId, query);
          const current = get().snapshot;
          if (
            current.selectedTaskId !== taskId ||
            current.timelineHistory.generation !== generation
          ) {
            return;
          }
          if (page.task_id !== taskId) throw new ProjectionGapError('Timeline page Task mismatch');
          set({
            snapshot: {
              ...current,
              timelineHistory: prependTimelinePage(
                current.timelineHistory,
                page.items,
                page.next_cursor,
              ),
            },
          });
        } catch (error) {
          const current = get().snapshot;
          if (
            current.selectedTaskId === taskId &&
            current.timelineHistory.generation === generation
          ) {
            set({
              snapshot: {
                ...current,
                timelineHistory: timelineHistoryError(current.timelineHistory, toApiError(error)),
              },
            });
          }
          throw error;
        }
      },
      returnToRecent() {
        const snapshot = get().snapshot;
        set({
          snapshot: {
            ...snapshot,
            timelineHistory: returnToRecent(
              snapshot.timelineHistory,
              snapshot.projection?.timeline_next_cursor ?? null,
            ),
          },
        });
      },
      async createTask(workspaceId) {
        const pending: PendingCommand = {
          kind: 'create_task',
          status: 'pending',
          body: { workspace_id: workspaceId, idempotency_key: newIdempotencyKey() },
        };
        set({ pendingCommand: pending });
        return (await executePendingCommand(pending)) as CreateTaskResponse;
      },
      async submitRun(input) {
        const snapshot = get().snapshot;
        const taskId = requireSelectedTask(snapshot);
        const revision = requireProjection(snapshot).task_revision;
        const pending: PendingCommand = {
          kind: 'submit_run',
          status: 'pending',
          taskId,
          body: {
            ...input,
            expected_task_revision: revision,
            idempotency_key: newIdempotencyKey(),
          },
        };
        set({
          pendingCommand: pending,
          snapshot: {
            ...snapshot,
            localDraft: {
              text: input.message,
              skillIds: [...input.skill_ids],
              tierId: input.tier_id,
            },
          },
        });
        return executePendingCommand(pending);
      },
      retryPendingCommand() {
        const pending = get().pendingCommand;
        if (pending === null || pending.status === 'conflicted') return Promise.resolve(undefined);
        return executePendingCommand(pending);
      },
      abandonConflictedCommand() {
        const pending = get().pendingCommand;
        if (pending?.status !== 'conflicted') return;
        if (pending.kind === 'stop_run') stoppedRunId = null;
        set({ pendingCommand: null });
      },
      async stopRun() {
        const snapshot = get().snapshot;
        const projection = requireProjection(snapshot);
        const runId = projection.active_run?.run_id;
        if (runId === undefined || runId === stoppedRunId) return;
        const taskId = requireSelectedTask(snapshot);
        const pending: PendingCommand = {
          kind: 'stop_run',
          status: 'pending',
          taskId,
          runId,
          body: {
            expected_task_revision: projection.task_revision,
            idempotency_key: newIdempotencyKey(),
          },
        };
        stoppedRunId = runId;
        set({ pendingCommand: pending });
        return executePendingCommand(pending);
      },
      async respond(interactionId, choiceId) {
        const snapshot = get().snapshot;
        const taskId = requireSelectedTask(snapshot);
        const pending: PendingCommand = {
          kind: 'respond',
          status: 'pending',
          taskId,
          interactionId,
          body: {
            choice_id: choiceId,
            expected_task_revision: requireProjection(snapshot).task_revision,
            idempotency_key: newIdempotencyKey(),
          },
        };
        set({ pendingCommand: pending });
        return executePendingCommand(pending);
      },
    };
  });

  return store;
}

const singletonStore = createWorkbenchStore();

export function useWorkbenchStore<T>(selector: (state: WorkbenchStoreState) => T): T {
  return useStore(singletonStore, selector);
}

export const loadTask = (taskId: string) => singletonStore.getState().loadTask(taskId);
export const subscribeTask = (taskId: string) => singletonStore.getState().subscribeTask(taskId);
export const setTaskDeltaObserver = (observer: TaskDeltaObserver | null) => {
  singletonStore.getState().setTaskDeltaObserver(observer);
};
export const loadOlder = () => singletonStore.getState().loadOlder();
export const createTask = (workspaceId: string) =>
  singletonStore.getState().createTask(workspaceId);
export const submitRun = (input: SubmitRunInput) => singletonStore.getState().submitRun(input);
export const retryPendingCommand = () => singletonStore.getState().retryPendingCommand();
export const abandonConflictedCommand = () => {
  singletonStore.getState().abandonConflictedCommand();
};
export const stopRun = () => singletonStore.getState().stopRun();
export const respond = (interactionId: string, choiceId: string) =>
  singletonStore.getState().respond(interactionId, choiceId);
export const reconnectFromCursor = () => singletonStore.getState().reconnectFromCursor();

async function runSubscriptionLoop(
  api: WebApi,
  store: StoreApi<WorkbenchStoreState>,
  taskId: string,
  controller: AbortController,
  acceptFrame: (event: TaskStreamEvent) => void,
  recoverProjection: (taskId: string) => Promise<TaskProjection>,
): Promise<void> {
  let attempt = 0;
  while (!controller.signal.aborted && store.getState().snapshot.selectedTaskId === taskId) {
    const after = store.getState().snapshot.cursor;
    store.setState({
      snapshot: beginTaskSubscription(store.getState().snapshot, taskId),
    });
    try {
      await consumeTaskStream(taskId, after, controller.signal, acceptFrame, api.tasks.subscribe);
      attempt += 1;
    } catch (error) {
      if (isAbortError(error)) return;
      if (error instanceof ContractDecodeError) {
        setConnectionError(store, error);
        return;
      }
      if (isProjectionRecoveryError(error)) {
        try {
          await recoverProjection(taskId);
          attempt = 0;
          continue;
        } catch (recoveryError) {
          if (isAbortError(recoveryError)) return;
          setConnectionError(store, recoveryError);
          return;
        }
      }
      attempt += 1;
    }

    const snapshot = store.getState().snapshot;
    store.setState({
      snapshot: {
        ...snapshot,
        connection: snapshot.projection === null ? 'offline' : 'reconnecting',
        lastError: null,
      },
    });
    await reconnectDelay(attempt, controller.signal);
  }
}

function reconnectDelay(attempt: number, signal: AbortSignal): Promise<void> {
  const milliseconds = Math.min(5_000, 250 * 2 ** Math.min(attempt, 5));
  return new Promise((resolve) => {
    const timeout = window.setTimeout(resolve, milliseconds);
    signal.addEventListener(
      'abort',
      () => {
        window.clearTimeout(timeout);
        resolve();
      },
      { once: true },
    );
  });
}

function setConnectionError(store: StoreApi<WorkbenchStoreState>, error: unknown): void {
  const snapshot = store.getState().snapshot;
  store.setState({
    snapshot: {
      ...snapshot,
      connection: 'error',
      lastError: error instanceof Error ? error.message : 'Task subscription failed',
    },
  });
}

function requireSelectedTask(snapshot: WorkbenchSnapshot): string {
  if (snapshot.selectedTaskId === null) throw new Error('No Task is selected');
  return snapshot.selectedTaskId;
}

function requireProjection(snapshot: WorkbenchSnapshot): TaskProjection {
  if (snapshot.projection === null) throw new Error('No Task projection is loaded');
  return snapshot.projection;
}

function isProjectionRecoveryError(error: unknown): boolean {
  return (
    error instanceof ProjectionGapError ||
    (error instanceof WebApiError && error.code === 'cursor_ahead')
  );
}

function isDefinitiveCommandError(code: ApiError['code']): boolean {
  return [
    'stale_command',
    'task_busy',
    'run_not_active',
    'interaction_not_pending',
    'outdated',
    'task_not_found',
  ].includes(code);
}

function isAbortError(error: unknown): boolean {
  return error instanceof DOMException && error.name === 'AbortError';
}

function toApiError(error: unknown): ApiError | null {
  if (!(error instanceof WebApiError)) return null;
  return {
    api_version: 1,
    code: error.code,
    details: error.details,
    message: error.message,
    trace_id: error.traceId,
  };
}

async function ignoreFailure(promise: Promise<unknown>): Promise<void> {
  try {
    await promise;
  } catch {
    // Reconciliation failure must not replace the original command error.
  }
}
