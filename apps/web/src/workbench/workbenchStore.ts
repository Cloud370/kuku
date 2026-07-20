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
type CommandStatus = 'pending' | 'unknown' | 'conflicted' | 'failed';

class StaleOperationError extends Error {
  constructor() {
    super('Operation is no longer current');
    this.name = 'StaleOperationError';
  }
}

interface PendingCommandMeta {
  commandId: number;
  controller: AbortController;
  taskGeneration: number;
  draftGeneration: number;
}

export type PendingCommand =
  | (PendingCommandMeta & {
      kind: 'create_task';
      status: CommandStatus;
      taskId: string | null;
      body: CreateTaskRequest;
    })
  | (PendingCommandMeta & {
      kind: 'submit_run';
      status: CommandStatus;
      taskId: string;
      body: SubmitRunRequest;
    })
  | (PendingCommandMeta & {
      kind: 'stop_run';
      status: CommandStatus;
      taskId: string;
      runId: RunId;
      body: StopRunRequest;
    })
  | (PendingCommandMeta & {
      kind: 'respond';
      status: CommandStatus;
      taskId: string;
      interactionId: InteractionId;
      body: InteractionResponseRequest;
    });

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
  clearTask(): void;
  setTaskError(message: string): void;
  loadTask(taskId: string): Promise<TaskProjection>;
  subscribeTask(taskId: string): () => void;
  reconnectFromCursor(): Promise<void>;
  acceptFrame(event: TaskStreamEvent): void;
  setTaskDeltaObserver(observer: TaskDeltaObserver | null): void;
  loadOlder(): Promise<void>;
  returnToRecent(): void;
  createTask(workspaceId: string): Promise<CreateTaskResponse | undefined>;
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
  let taskGeneration = 0;
  let draftGeneration = 0;
  let commandSequence = 0;
  let stopCommandPromise: Promise<PendingCommandResult> | null = null;

  const store = createStore<WorkbenchStoreState>((set, get) => {
    const acceptFrame = (event: TaskStreamEvent): void => {
      const next = applyStreamEvent(get().snapshot, event);
      set({ snapshot: next });
      observer?.(event.task_id, event.event);
    };

    const recoverProjection = async (
      taskId: string,
      expectedTaskGeneration = taskGeneration,
    ): Promise<TaskProjection> => {
      const localDraft = get().snapshot.localDraft;
      const expectedDraftGeneration = draftGeneration;
      const projection = await api.tasks.get(taskId);
      if (taskGeneration !== expectedTaskGeneration || get().snapshot.selectedTaskId !== taskId) {
        return projection;
      }
      const snapshot = applyProjection(get().snapshot, projection);
      snapshot.localDraft =
        draftGeneration === expectedDraftGeneration ? localDraft : get().snapshot.localDraft;
      set({ snapshot });
      return projection;
    };

    const consumeOnce = async (
      taskId: string,
      after: number | null,
      controller: AbortController,
      expectedTaskGeneration: number,
      isCurrent: () => boolean,
    ): Promise<void> => {
      if (taskGeneration !== expectedTaskGeneration || !isCurrent()) {
        throw new StaleOperationError();
      }
      set({ snapshot: beginTaskSubscription(get().snapshot, taskId) });
      await consumeTaskStream(
        taskId,
        after,
        controller.signal,
        (event) => {
          if (!isCurrent()) throw new StaleOperationError();
          acceptFrame(event);
        },
        api.tasks.subscribe,
      );
    };

    const handleCommandFailure = async (
      pending: PendingCommand,
      error: unknown,
    ): Promise<never> => {
      if (!isCurrentCommand(get().pendingCommand, pending)) throw error;
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
          await ignoreFailure(recoverProjection(pending.taskId, pending.taskGeneration));
        }
      } else if (error instanceof WebApiError && isDefinitiveCommandError(error.code)) {
        if (pending.kind === 'create_task') {
          set({ pendingCommand: { ...pending, status: 'failed' } });
        } else {
          set({ pendingCommand: null });
          if (pending.kind === 'stop_run') stoppedRunId = null;
          await ignoreFailure(recoverProjection(pending.taskId, pending.taskGeneration));
        }
      } else {
        set({ pendingCommand: { ...pending, status: 'unknown' } });
      }
      throw error;
    };

    const executePendingCommand = async (
      pending: PendingCommand,
    ): Promise<PendingCommandResult> => {
      try {
        switch (pending.kind) {
          case 'create_task': {
            const result = await api.tasks.create(pending.body);
            const current = isCurrentCommand(get().pendingCommand, pending);
            if (!current) return undefined;
            pending.controller.abort();
            set({ pendingCommand: null });
            if (!isCurrentTaskScope(get().snapshot, pending, taskGeneration, draftGeneration)) {
              return undefined;
            }
            const selected = beginTaskSubscription(get().snapshot, result.projection.task.task_id);
            taskGeneration += 1;
            if (selected.selectedTaskId !== result.projection.task.task_id) {
              draftGeneration += 1;
            }
            set({ snapshot: applyProjection(selected, result.projection) });
            return result;
          }
          case 'submit_run': {
            const result = await api.tasks.submitRun(pending.taskId, pending.body);
            if (!isCurrentCommand(get().pendingCommand, pending)) return result;
            set({ pendingCommand: null });
            if (isCurrentTaskScope(get().snapshot, pending, taskGeneration, draftGeneration)) {
              draftGeneration += 1;
              set({
                snapshot: {
                  ...get().snapshot,
                  localDraft: { text: '', skillIds: [], tierId: null },
                },
              });
            }
            return result;
          }
          case 'stop_run': {
            const result = await api.tasks.stopRun(pending.taskId, pending.body);
            if (!isCurrentCommand(get().pendingCommand, pending)) return result;
            set({ pendingCommand: null });
            if (isCurrentTaskScope(get().snapshot, pending, taskGeneration, draftGeneration)) {
              stoppedRunId = pending.runId;
            }
            return result;
          }
          case 'respond': {
            const result = await api.tasks.respond(
              pending.taskId,
              pending.interactionId,
              pending.body,
            );
            if (isCurrentCommand(get().pendingCommand, pending)) set({ pendingCommand: null });
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
        draftGeneration += 1;
        set({ snapshot: { ...get().snapshot, localDraft: structuredClone(draft) } });
      },
      clearTask() {
        subscriptionController?.abort();
        subscriptionController = null;
        taskGeneration += 1;
        draftGeneration += 1;
        stoppedRunId = null;
        set({ snapshot: createWorkbenchSnapshot() });
      },
      setTaskError(message) {
        const snapshot = get().snapshot;
        set({
          snapshot: {
            ...snapshot,
            connection: 'error',
            lastError: message,
          },
        });
      },
      async loadTask(taskId) {
        subscriptionController?.abort();
        subscriptionController = null;
        stoppedRunId = null;
        const previousTaskId = get().snapshot.selectedTaskId;
        taskGeneration += 1;
        if (previousTaskId !== taskId) draftGeneration += 1;
        const expectedTaskGeneration = taskGeneration;
        set({ snapshot: beginTaskSubscription(get().snapshot, taskId) });
        const projection = await api.tasks.get(taskId);
        if (get().snapshot.selectedTaskId === taskId && taskGeneration === expectedTaskGeneration) {
          set({ snapshot: applyProjection(get().snapshot, projection) });
        }
        return projection;
      },
      subscribeTask(taskId) {
        subscriptionController?.abort();
        const controller = new AbortController();
        subscriptionController = controller;
        const expectedTaskGeneration = taskGeneration;
        void runSubscriptionLoop(
          api,
          store,
          taskId,
          controller,
          expectedTaskGeneration,
          acceptFrame,
          recoverProjection,
          () =>
            isCurrentSubscription(
              store.getState().snapshot,
              taskId,
              expectedTaskGeneration,
              controller,
              subscriptionController,
              taskGeneration,
            ),
        );
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
        const expectedTaskGeneration = taskGeneration;
        const after = get().snapshot.cursor;
        const isCurrent = () =>
          isCurrentSubscription(
            get().snapshot,
            taskId,
            expectedTaskGeneration,
            controller,
            subscriptionController,
            taskGeneration,
          );
        try {
          await consumeOnce(taskId, after, controller, expectedTaskGeneration, isCurrent);
        } catch (error) {
          if (!isCurrent()) return;
          if (!isProjectionRecoveryError(error)) throw error;
          const projection = await recoverProjection(taskId, expectedTaskGeneration);
          if (!isCurrent()) return;
          await consumeOnce(
            taskId,
            projection.cursor,
            controller,
            expectedTaskGeneration,
            isCurrent,
          );
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
        assertNoPendingCommand(get().pendingCommand);
        const snapshot = get().snapshot;
        const pending: PendingCommand = {
          commandId: ++commandSequence,
          controller: new AbortController(),
          taskGeneration,
          draftGeneration,
          kind: 'create_task',
          status: 'pending',
          taskId: snapshot.selectedTaskId,
          body: { workspace_id: workspaceId, idempotency_key: newIdempotencyKey() },
        };
        set({ pendingCommand: pending });
        return (await executePendingCommand(pending)) as CreateTaskResponse | undefined;
      },
      async submitRun(input) {
        assertNoPendingCommand(get().pendingCommand);
        const snapshot = get().snapshot;
        const taskId = requireSelectedTask(snapshot);
        const revision = requireProjection(snapshot).task_revision;
        draftGeneration += 1;
        const pending: PendingCommand = {
          commandId: ++commandSequence,
          controller: new AbortController(),
          taskGeneration,
          draftGeneration,
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
        if (pending === null || pending.status !== 'unknown') return Promise.resolve(undefined);
        const retrying = { ...pending, status: 'pending' as const };
        set({ pendingCommand: retrying });
        return executePendingCommand(retrying);
      },
      abandonConflictedCommand() {
        const pending = get().pendingCommand;
        if (
          pending?.status !== 'unknown' &&
          pending?.status !== 'conflicted' &&
          pending?.status !== 'failed'
        )
          return;
        pending.controller.abort();
        if (pending.kind === 'stop_run') stoppedRunId = null;
        set({ pendingCommand: null });
      },
      async stopRun() {
        if (get().pendingCommand?.kind === 'stop_run' && stopCommandPromise !== null) {
          return stopCommandPromise;
        }
        assertNoPendingCommand(get().pendingCommand);
        const snapshot = get().snapshot;
        const projection = requireProjection(snapshot);
        const runId = projection.active_run?.run_id;
        if (runId === undefined || runId === stoppedRunId) return;
        const taskId = requireSelectedTask(snapshot);
        const pending: PendingCommand = {
          commandId: ++commandSequence,
          controller: new AbortController(),
          taskGeneration,
          draftGeneration,
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
        const commandPromise = executePendingCommand(pending);
        stopCommandPromise = commandPromise;
        void commandPromise.then(
          () => {
            if (stopCommandPromise === commandPromise) stopCommandPromise = null;
          },
          () => {
            if (stopCommandPromise === commandPromise) stopCommandPromise = null;
          },
        );
        return commandPromise;
      },
      async respond(interactionId, choiceId) {
        assertNoPendingCommand(get().pendingCommand);
        const snapshot = get().snapshot;
        const taskId = requireSelectedTask(snapshot);
        const pending: PendingCommand = {
          commandId: ++commandSequence,
          controller: new AbortController(),
          taskGeneration,
          draftGeneration,
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
  expectedTaskGeneration: number,
  acceptFrame: (event: TaskStreamEvent) => void,
  recoverProjection: (taskId: string, expectedTaskGeneration?: number) => Promise<TaskProjection>,
  isCurrent: () => boolean,
): Promise<void> {
  let attempt = 0;
  while (!controller.signal.aborted && isCurrent()) {
    const after = store.getState().snapshot.cursor;
    if (!isCurrent()) return;
    store.setState({
      snapshot: beginTaskSubscription(store.getState().snapshot, taskId),
    });
    try {
      await consumeTaskStream(
        taskId,
        after,
        controller.signal,
        (event) => {
          if (!isCurrent()) throw new StaleOperationError();
          acceptFrame(event);
        },
        api.tasks.subscribe,
      );
      if (!isCurrent()) return;
      attempt += 1;
    } catch (error) {
      if (!isCurrent() || isAbortError(error) || error instanceof StaleOperationError) return;
      if (error instanceof ContractDecodeError) {
        setConnectionError(store, error, isCurrent);
        return;
      }
      if (isProjectionRecoveryError(error)) {
        try {
          await recoverProjection(taskId, expectedTaskGeneration);
          if (!isCurrent()) return;
          attempt = 0;
          continue;
        } catch (recoveryError) {
          if (
            !isCurrent() ||
            isAbortError(recoveryError) ||
            recoveryError instanceof StaleOperationError
          )
            return;
          setConnectionError(store, recoveryError, isCurrent);
          return;
        }
      }
      attempt += 1;
    }

    if (!isCurrent()) return;
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

function setConnectionError(
  store: StoreApi<WorkbenchStoreState>,
  error: unknown,
  isCurrent: () => boolean,
): void {
  if (!isCurrent()) return;
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

function isCurrentCommand(current: PendingCommand | null, pending: PendingCommand): boolean {
  return current?.commandId === pending.commandId && current.controller === pending.controller;
}

function isCurrentTaskScope(
  snapshot: WorkbenchSnapshot,
  pending: PendingCommand,
  taskGeneration: number,
  draftGeneration: number,
): boolean {
  return (
    snapshot.selectedTaskId === pending.taskId &&
    pending.taskGeneration === taskGeneration &&
    pending.draftGeneration === draftGeneration
  );
}

function isCurrentSubscription(
  snapshot: WorkbenchSnapshot,
  taskId: string,
  expectedTaskGeneration: number,
  controller: AbortController,
  activeController: AbortController | null,
  currentTaskGeneration: number,
): boolean {
  return (
    !controller.signal.aborted &&
    activeController === controller &&
    currentTaskGeneration === expectedTaskGeneration &&
    snapshot.selectedTaskId === taskId
  );
}

function assertNoPendingCommand(pending: PendingCommand | null): void {
  if (pending !== null) throw new Error('A command outcome is already pending');
}

function isDefinitiveCommandError(code: ApiError['code']): boolean {
  return [
    'stale_command',
    'task_busy',
    'run_not_active',
    'interaction_not_pending',
    'outdated',
    'task_not_found',
    'workspace_not_found',
    'workspace_unavailable',
    'invalid_request',
    'forbidden',
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
