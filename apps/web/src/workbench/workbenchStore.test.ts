import { describe, expect, it, vi } from 'vitest';

import taskProjectionJson from '../api/generated/fixtures/task_projection.json';
import type {
  ApiError,
  CreateTaskResponse,
  InteractionProjection,
  TaskProjection,
  TaskStreamEvent,
  TimelineItemProjection,
  TimelinePage,
} from '../api/generated';
import { WebApiError, webApi } from '../api/client';
import { ContractDecodeError } from '../api/decode';

import { applyProjection, createWorkbenchSnapshot, selectTimelineItems } from './state';
import { createWorkbenchStore } from './workbenchStore';

type WebApi = typeof webApi;

const taskId = 'tsk_000000000000000000000001';
const otherTaskId = 'tsk_000000000000000000000002';
const workspaceId = 'wsp_000000000000000000000001';

function projection(
  id = taskId,
  cursor = 7,
  revision = 4,
  timeline: TimelineItemProjection[] = [],
): TaskProjection {
  const value = structuredClone(taskProjectionJson) as TaskProjection;
  value.task.task_id = id;
  value.task.workspace_id = workspaceId;
  value.cursor = cursor;
  value.task_revision = revision;
  value.timeline = timeline;
  return value;
}

function message(number: number): TimelineItemProjection {
  return {
    type: 'message',
    item: {
      file_references: [],
      finalized: true,
      message_id: `msg_${String(number)}`,
      order_key: number,
      request_ids: [],
      role: 'agent',
      text: `message ${String(number)}`,
    },
  };
}

function replacement(value = projection()): TaskStreamEvent {
  return {
    api_version: 1,
    cursor: value.cursor,
    task_id: value.task.task_id,
    task_revision: value.task_revision,
    event: { type: 'projection_replaced', projection: value },
  };
}

function apiError(code: ApiError['code']): WebApiError {
  return new WebApiError(409, {
    api_version: 1,
    code,
    details: null,
    message: code,
    trace_id: 'trace_000000000000000000000001',
  });
}

function mockApi() {
  const tasks = {
    ...webApi.tasks,
    list: vi.fn<WebApi['tasks']['list']>(),
    create: vi.fn<WebApi['tasks']['create']>(),
    get: vi.fn<WebApi['tasks']['get']>(),
    timeline: vi.fn<WebApi['tasks']['timeline']>(),
    submitRun: vi.fn<WebApi['tasks']['submitRun']>(),
    stopRun: vi.fn<WebApi['tasks']['stopRun']>(),
    respond: vi.fn<WebApi['tasks']['respond']>(),
    subscribe: vi.fn<WebApi['tasks']['subscribe']>(),
  };
  return { api: { ...webApi, tasks }, tasks };
}

function readySnapshot(value = projection()) {
  return applyProjection(createWorkbenchSnapshot(), value);
}

function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  let reject: (reason: unknown) => void = () => undefined;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

describe('idempotent Task commands', () => {
  it('rejects a second create without replacing the pending logical intent', async () => {
    const { api, tasks } = mockApi();
    const store = createWorkbenchStore(api);
    const firstRequest = deferred<CreateTaskResponse>();
    tasks.create
      .mockReturnValueOnce(firstRequest.promise)
      .mockRejectedValueOnce(new Error('second request should be rejected locally'));

    const first = store.getState().createTask(workspaceId);
    const pending = store.getState().pendingCommand;
    await expect(store.getState().createTask(workspaceId)).rejects.toThrow(
      'A command outcome is already pending',
    );

    expect(tasks.create).toHaveBeenCalledTimes(1);
    expect(store.getState().pendingCommand).toBe(pending);
    firstRequest.reject(new TypeError('offline'));
    await expect(first).rejects.toThrow('offline');
    expect(store.getState().pendingCommand).toMatchObject({
      kind: 'create_task',
      status: 'unknown',
      body: pending?.body,
    });
  });

  it('retries an unknown submit outcome with the exact logical body', async () => {
    const { api, tasks } = mockApi();
    const store = createWorkbenchStore(api);
    store.setState({ snapshot: readySnapshot() });
    tasks.submitRun.mockRejectedValueOnce(new TypeError('network outcome unknown'));

    await expect(
      store.getState().submitRun({ message: 'inspect', tier_id: 'tier:balanced', skill_ids: [] }),
    ).rejects.toThrow('network outcome unknown');
    const firstBody = tasks.submitRun.mock.calls[0]?.[1];
    tasks.submitRun.mockResolvedValue({
      api_version: 1,
      replayed: true,
      run_id: 'run_000000000000000000000001',
      task_id: taskId,
      task_revision: 5,
    });

    await store.getState().retryPendingCommand();

    expect(tasks.submitRun).toHaveBeenNthCalledWith(2, taskId, firstBody);
    expect(firstBody).toMatchObject({
      message: 'inspect',
      tier_id: 'tier:balanced',
      skill_ids: [],
      expected_task_revision: 4,
    });
    expect(firstBody?.idempotency_key).toMatch(/^idem-/);
    expect(store.getState().pendingCommand).toBeNull();
    expect(store.getState().snapshot.localDraft.text).toBe('');
  });

  it('retries unknown Task creation with the same workspace body and key', async () => {
    const { api, tasks } = mockApi();
    const store = createWorkbenchStore(api);
    tasks.create.mockRejectedValueOnce(new TypeError('network outcome unknown'));

    await expect(store.getState().createTask(workspaceId)).rejects.toThrow();
    const firstBody = tasks.create.mock.calls[0]?.[0];
    const created = projection(otherTaskId);
    tasks.create.mockResolvedValue({
      api_version: 1,
      projection: created,
      replayed: true,
    } satisfies CreateTaskResponse);

    const result = await store.getState().retryPendingCommand();

    expect(tasks.create).toHaveBeenNthCalledWith(2, firstBody);
    expect(firstBody).toEqual({
      workspace_id: workspaceId,
      idempotency_key: firstBody?.idempotency_key,
    });
    expect(firstBody?.idempotency_key).toMatch(/^idem-/);
    expect(result).toEqual(expect.objectContaining({ projection: created }));
    expect(store.getState().snapshot.selectedTaskId).toBe(otherTaskId);
    expect(store.getState().pendingCommand).toBeNull();
  });

  it('does not open a created Task over a newer Task selection or draft', async () => {
    const { api, tasks } = mockApi();
    const store = createWorkbenchStore(api);
    store.setState({ snapshot: readySnapshot() });
    const createRequest = deferred<CreateTaskResponse>();
    tasks.create.mockReturnValue(createRequest.promise);
    const creating = store.getState().createTask(workspaceId);
    const taskB = projection(otherTaskId, 9, 5);
    tasks.get.mockResolvedValue(taskB);

    await store.getState().loadTask(otherTaskId);
    store.getState().setDraft({ text: 'draft for B', skillIds: [], tierId: 'tier:balanced' });
    createRequest.resolve(createdResponse(projection('tsk_000000000000000000000003')));

    await expect(creating).resolves.toBeUndefined();
    expect(store.getState().snapshot.selectedTaskId).toBe(otherTaskId);
    expect(store.getState().snapshot.localDraft).toEqual({
      text: 'draft for B',
      skillIds: [],
      tierId: 'tier:balanced',
    });
    expect(store.getState().pendingCommand).toBeNull();
  });

  it('does not clear a newer Task draft when an older submit acknowledgement arrives', async () => {
    const { api, tasks } = mockApi();
    const store = createWorkbenchStore(api);
    store.setState({ snapshot: readySnapshot() });
    const submitRequest = deferred<Awaited<ReturnType<WebApi['tasks']['submitRun']>>>();
    tasks.submitRun.mockReturnValue(submitRequest.promise);
    const submitting = store
      .getState()
      .submitRun({ message: 'draft for A', tier_id: 'tier:balanced', skill_ids: [] });
    const taskB = projection(otherTaskId, 9, 5);
    tasks.get.mockResolvedValue(taskB);

    await store.getState().loadTask(otherTaskId);
    store.getState().setDraft({
      text: 'new draft for B',
      skillIds: ['skill:project:review'],
      tierId: 'tier:deep',
    });
    submitRequest.resolve({
      api_version: 1,
      replayed: false,
      run_id: 'run_000000000000000000000001',
      task_id: taskId,
      task_revision: 5,
    });
    await submitting;

    expect(store.getState().snapshot.selectedTaskId).toBe(otherTaskId);
    expect(store.getState().snapshot.localDraft).toEqual({
      text: 'new draft for B',
      skillIds: ['skill:project:review'],
      tierId: 'tier:deep',
    });
  });

  it('retains a permanent create failure for explicit abandon and recreation', async () => {
    const { api, tasks } = mockApi();
    const store = createWorkbenchStore(api);
    const current = readySnapshot();
    current.localDraft.text = 'keep this draft';
    store.setState({ snapshot: current });
    tasks.create.mockRejectedValueOnce(apiError('workspace_not_found'));

    await expect(store.getState().createTask(workspaceId)).rejects.toMatchObject({
      code: 'workspace_not_found',
    });

    expect(store.getState().pendingCommand).toMatchObject({
      kind: 'create_task',
      status: 'failed',
    });
    expect(store.getState().snapshot.localDraft.text).toBe('keep this draft');
    await expect(store.getState().retryPendingCommand()).resolves.toBeUndefined();
    expect(tasks.create).toHaveBeenCalledTimes(1);
    store.getState().abandonConflictedCommand();
    expect(store.getState().pendingCommand).toBeNull();
  });

  it('reconciles and retains an idempotency conflict for explicit acknowledgement', async () => {
    const { api, tasks } = mockApi();
    const store = createWorkbenchStore(api);
    tasks.create.mockRejectedValue(apiError('idempotency_conflict'));
    tasks.list.mockResolvedValue({ api_version: 1, items: [], next_cursor: null });

    await expect(store.getState().createTask(workspaceId)).rejects.toMatchObject({
      code: 'idempotency_conflict',
    });

    expect(tasks.list).toHaveBeenCalledWith({
      workspace_id: workspaceId,
      search: null,
      cursor: null,
      limit: 100,
    });
    expect(store.getState().pendingCommand).toMatchObject({
      kind: 'create_task',
      status: 'conflicted',
    });
    await expect(store.getState().retryPendingCommand()).resolves.toBeUndefined();
    expect(tasks.create).toHaveBeenCalledTimes(1);
    store.getState().abandonConflictedCommand();
    expect(store.getState().pendingCommand).toBeNull();
  });

  it('coalesces repeated Stop clicks for the same active Run', async () => {
    const { api, tasks } = mockApi();
    const store = createWorkbenchStore(api);
    const current = projection();
    current.active_run = {
      completion: null,
      finished_at: null,
      run_id: 'run_000000000000000000000001',
      started_at: '2026-07-18T09:00:00Z',
      state: 'running',
    };
    store.setState({ snapshot: readySnapshot(current) });
    let acceptStop: (() => void) | undefined;
    tasks.stopRun.mockReturnValue(
      new Promise((resolve) => {
        acceptStop = () => {
          resolve({ api_version: 1, replayed: false, task_id: taskId, task_revision: 5 });
        };
      }),
    );

    const first = store.getState().stopRun();
    const second = store.getState().stopRun();

    expect(tasks.stopRun).toHaveBeenCalledTimes(1);
    acceptStop?.();
    await Promise.all([first, second]);
  });

  it('allows a new Stop intent only after a conflicted command is explicitly abandoned', async () => {
    const { api, tasks } = mockApi();
    const store = createWorkbenchStore(api);
    const current = projection();
    current.active_run = {
      completion: null,
      finished_at: null,
      run_id: 'run_000000000000000000000001',
      started_at: '2026-07-18T09:00:00Z',
      state: 'running',
    };
    store.setState({ snapshot: readySnapshot(current) });
    tasks.stopRun.mockRejectedValueOnce(apiError('idempotency_conflict'));
    tasks.get.mockResolvedValue(current);

    await expect(store.getState().stopRun()).rejects.toMatchObject({
      code: 'idempotency_conflict',
    });
    const conflictedKey = tasks.stopRun.mock.calls[0]?.[1].idempotency_key;
    store.getState().abandonConflictedCommand();
    tasks.stopRun.mockResolvedValue({
      api_version: 1,
      replayed: false,
      task_id: taskId,
      task_revision: 5,
    });

    await store.getState().stopRun();

    expect(tasks.stopRun).toHaveBeenCalledTimes(2);
    expect(tasks.stopRun.mock.calls[1]?.[1].idempotency_key).not.toBe(conflictedKey);
  });

  it('allows a fresh Stop intent after a definitive stale command is reconciled', async () => {
    const { api, tasks } = mockApi();
    const store = createWorkbenchStore(api);
    const current = projection();
    current.active_run = {
      completion: null,
      finished_at: null,
      run_id: 'run_000000000000000000000001',
      started_at: '2026-07-18T09:00:00Z',
      state: 'running',
    };
    store.setState({ snapshot: readySnapshot(current) });
    const reconciled = structuredClone(current);
    reconciled.task_revision = 5;
    tasks.stopRun.mockRejectedValueOnce(apiError('stale_command'));
    tasks.get.mockResolvedValue(reconciled);

    await expect(store.getState().stopRun()).rejects.toMatchObject({ code: 'stale_command' });
    tasks.stopRun.mockResolvedValue({
      api_version: 1,
      replayed: false,
      task_id: taskId,
      task_revision: 6,
    });
    await store.getState().stopRun();

    expect(tasks.stopRun).toHaveBeenCalledTimes(2);
    expect(tasks.stopRun.mock.calls[1]?.[1].expected_task_revision).toBe(5);
  });

  it('keeps a resolved interaction visible without replacing it from a stale projection', async () => {
    const { api, tasks } = mockApi();
    const store = createWorkbenchStore(api);
    const interaction: InteractionProjection = {
      interaction_id: 'int_000000000000000000000001',
      order_key: 8,
      prompt: 'Permission request',
      choices: [{ choice_id: 'approve', label: 'Approve' }],
      status: 'pending',
      selected_choice_id: null,
    };
    store.setState({
      snapshot: readySnapshot(
        projection(taskId, 8, 4, [{ type: 'interaction', item: interaction }]),
      ),
    });
    tasks.respond.mockResolvedValue({
      api_version: 1,
      replayed: false,
      task_id: taskId,
      task_revision: 5,
    });
    tasks.get.mockResolvedValue(projection(taskId, 7, 4));

    await store.getState().respond(interaction.interaction_id, 'approve');

    expect(tasks.get).not.toHaveBeenCalled();
    expect(store.getState().snapshot.projection?.task_revision).toBe(5);
    expect(selectTimelineItems(store.getState().snapshot)).toMatchObject([
      {
        type: 'interaction',
        item: { status: 'resolved', selected_choice_id: 'approve' },
      },
    ]);
  });

  it('clears an unknown interaction outcome when the task stream confirms it', async () => {
    const { api, tasks } = mockApi();
    const store = createWorkbenchStore(api);
    const interaction: InteractionProjection = {
      interaction_id: 'int_000000000000000000000001',
      order_key: 8,
      prompt: 'Permission request',
      choices: [{ choice_id: 'approve', label: 'Approve' }],
      status: 'pending',
      selected_choice_id: null,
    };
    store.setState({
      snapshot: readySnapshot(
        projection(taskId, 8, 4, [{ type: 'interaction', item: interaction }]),
      ),
    });
    tasks.respond.mockRejectedValueOnce(new TypeError('response ended after commit'));

    await expect(store.getState().respond(interaction.interaction_id, 'approve')).rejects.toThrow(
      'response ended after commit',
    );
    expect(store.getState().pendingCommand).toMatchObject({ kind: 'respond', status: 'unknown' });

    store.getState().acceptFrame({
      api_version: 1,
      cursor: 9,
      task_id: taskId,
      task_revision: 5,
      event: {
        type: 'changes_applied',
        changes: [
          {
            type: 'interaction_upserted',
            interaction: {
              ...interaction,
              selected_choice_id: 'approve',
              status: 'resolved',
            },
          },
        ],
        timeline_window: null,
      },
    });

    expect(store.getState().pendingCommand).toBeNull();
    expect(selectTimelineItems(store.getState().snapshot)).toMatchObject([
      {
        type: 'interaction',
        item: { status: 'resolved', selected_choice_id: 'approve' },
      },
    ]);
  });

  it('retries an interaction once with the recovered task revision', async () => {
    const { api, tasks } = mockApi();
    const store = createWorkbenchStore(api);
    store.setState({ snapshot: readySnapshot() });
    const reconciled = projection(taskId, 8, 5);
    const currentWindow = projection(taskId, 9, 6);
    const delayedInteraction: InteractionProjection = {
      interaction_id: 'int_000000000000000000000001',
      order_key: 9,
      prompt: 'Permission request',
      choices: [{ choice_id: 'approve', label: 'Approve' }],
      status: 'pending',
      selected_choice_id: null,
    };
    tasks.respond.mockRejectedValueOnce(apiError('stale_command'));
    tasks.get.mockResolvedValueOnce(reconciled).mockResolvedValueOnce(currentWindow);
    tasks.respond.mockResolvedValueOnce({
      api_version: 1,
      replayed: false,
      task_id: taskId,
      task_revision: 6,
    });

    await store.getState().respond('int_000000000000000000000001', 'approve');

    expect(tasks.respond).toHaveBeenCalledTimes(2);
    expect(tasks.respond.mock.calls[0]?.[2].expected_task_revision).toBe(4);
    expect(tasks.respond.mock.calls[1]?.[2].expected_task_revision).toBe(5);
    expect(tasks.respond.mock.calls[1]?.[2].idempotency_key).toBe(
      tasks.respond.mock.calls[0]?.[2].idempotency_key,
    );
    expect(tasks.get).toHaveBeenCalledTimes(1);
    store.getState().acceptFrame({
      api_version: 1,
      cursor: 10,
      task_id: taskId,
      task_revision: 7,
      event: {
        type: 'changes_applied',
        changes: [{ type: 'interaction_upserted', interaction: delayedInteraction }],
        timeline_window: { evicted_items: [], next_cursor: null },
      },
    });
    expect(selectTimelineItems(store.getState().snapshot)).toMatchObject([
      {
        type: 'interaction',
        item: { status: 'resolved', selected_choice_id: 'approve' },
      },
    ]);
  });
});

describe('projection synchronization', () => {
  it('ignores a stale subscription error after loading another Task', async () => {
    const { api, tasks } = mockApi();
    const store = createWorkbenchStore(api);
    store.setState({ snapshot: readySnapshot() });
    const oldStream = deferred<Response>();
    tasks.subscribe.mockReturnValue(oldStream.promise);
    store.getState().subscribeTask(taskId);
    await vi.waitFor(() => {
      expect(tasks.subscribe).toHaveBeenCalled();
    });
    tasks.get.mockResolvedValue(projection(otherTaskId, 9, 5));

    await store.getState().loadTask(otherTaskId);
    oldStream.reject(new ContractDecodeError(['/ stale stream']));
    await new Promise((resolve) => window.setTimeout(resolve, 0));

    expect(store.getState().snapshot).toMatchObject({
      selectedTaskId: otherTaskId,
      connection: 'ready',
      lastError: null,
    });
  });

  it('notifies one observer only after a complete frame commits', () => {
    const { api } = mockApi();
    const store = createWorkbenchStore(api);
    const observer = vi.fn();
    store.setState({
      snapshot: { ...createWorkbenchSnapshot(), selectedTaskId: taskId, awaitingReplacement: true },
    });
    store.getState().setTaskDeltaObserver(observer);

    store.getState().acceptFrame(replacement());

    expect(observer).toHaveBeenCalledTimes(1);
    expect(observer).toHaveBeenCalledWith(
      taskId,
      expect.objectContaining({
        type: 'projection_replaced',
      }),
    );

    const invalid = replacement(projection(taskId, 8, 5));
    expect(() => {
      store.getState().acceptFrame(invalid);
    }).toThrow();
    expect(observer).toHaveBeenCalledTimes(1);
  });

  it('loads one opaque older page and ignores a late response after replacement', async () => {
    const { api, tasks } = mockApi();
    const store = createWorkbenchStore(api);
    const current = projection(taskId, 7, 4, [message(3)]);
    current.timeline_next_cursor = 'page:first';
    store.setState({ snapshot: readySnapshot(current) });
    let resolvePage: ((page: TimelinePage) => void) | undefined;
    tasks.timeline.mockReturnValue(
      new Promise((resolve) => {
        resolvePage = resolve;
      }),
    );

    const loading = store.getState().loadOlder();
    const refreshed = projection(taskId, 8, 4, [message(4)]);
    refreshed.timeline_next_cursor = null;
    store.setState({ snapshot: { ...store.getState().snapshot, awaitingReplacement: true } });
    store.getState().acceptFrame(replacement(refreshed));
    resolvePage?.({
      api_version: 1,
      items: [message(1), message(2)],
      next_cursor: null,
      task_id: taskId,
    });
    await loading;

    expect(tasks.timeline).toHaveBeenCalledWith(taskId, { before: 'page:first', limit: 500 });
    expect(
      selectTimelineItems(store.getState().snapshot).map((item) => item.item.order_key),
    ).toEqual([4]);
  });

  it('recovers cursor_ahead from one authoritative projection before reconnecting', async () => {
    const { api, tasks } = mockApi();
    const store = createWorkbenchStore(api);
    store.setState({ snapshot: readySnapshot(projection(taskId, 5, 3)) });
    const authoritative = projection(taskId, 6, 4);
    tasks.get.mockResolvedValue(authoritative);
    tasks.subscribe.mockRejectedValueOnce(apiError('cursor_ahead'));
    tasks.subscribe.mockResolvedValueOnce(
      new Response(
        new ReadableStream({
          start(controller) {
            controller.enqueue(
              new TextEncoder().encode(`${JSON.stringify(replacement(authoritative))}\n`),
            );
            controller.close();
          },
        }),
      ),
    );

    await store.getState().reconnectFromCursor();

    expect(tasks.get).toHaveBeenCalledWith(taskId);
    expect(tasks.subscribe).toHaveBeenNthCalledWith(2, taskId, 6, expect.any(AbortSignal));
    expect(store.getState().snapshot.cursor).toBe(6);
  });
});

function createdResponse(value: TaskProjection): CreateTaskResponse {
  return { api_version: 1, projection: value, replayed: false };
}
