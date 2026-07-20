import type { SubmitRunRequest, TaskStreamEvent } from '../../api/generated';
import { webApi } from '../../api/client';
import {
  fixtureCatalog,
  fixtureTaskId,
  fixtureTaskPage,
  fixtureWorkspacePage,
  readyProjection,
} from './projections';

export interface FakeWorkbenchServer {
  api: typeof webApi;
  submitBodies: SubmitRunRequest[];
  taskId: string;
}

export function createFakeWorkbenchServer(): FakeWorkbenchServer {
  const submitBodies: SubmitRunRequest[] = [];
  const projection = readyProjection();
  const replacement: TaskStreamEvent = {
    api_version: 1,
    cursor: projection.cursor,
    event: { projection, type: 'projection_replaced' },
    task_id: fixtureTaskId,
    task_revision: projection.task_revision,
  };
  const tasks = {
    ...webApi.tasks,
    create: () => Promise.resolve({ api_version: 1, projection, replayed: false }),
    get: () => Promise.resolve(structuredClone(projection)),
    list: () => Promise.resolve(fixtureTaskPage()),
    respond: () =>
      Promise.resolve({
        api_version: 1,
        replayed: false,
        task_id: fixtureTaskId,
        task_revision: projection.task_revision,
      }),
    stopRun: () =>
      Promise.resolve({
        api_version: 1,
        replayed: false,
        task_id: fixtureTaskId,
        task_revision: projection.task_revision,
      }),
    submitRun: (_taskId: string, body: SubmitRunRequest) => {
      submitBodies.push(structuredClone(body));
      return Promise.resolve({
        api_version: 1 as const,
        replayed: false,
        run_id: 'run_000000000000000000000001',
        task_id: fixtureTaskId,
        task_revision: projection.task_revision + 1,
      });
    },
    subscribe: (_taskId: string, _after: number | null, signal: AbortSignal) => {
      const encoder = new TextEncoder();
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          controller.enqueue(encoder.encode(`${JSON.stringify(replacement)}\n`));
          signal.addEventListener('abort', () => {
            controller.close();
          });
        },
      });
      return Promise.resolve(
        new Response(body, { headers: { 'content-type': 'application/x-ndjson' } }),
      );
    },
    timeline: () =>
      Promise.resolve({
        api_version: 1,
        items: [],
        next_cursor: null,
        task_id: fixtureTaskId,
      }),
  } satisfies typeof webApi.tasks;
  const api = {
    ...webApi,
    catalog: {
      ...webApi.catalog,
      workspace: () => Promise.resolve(fixtureCatalog()),
    },
    tasks,
    workspaces: {
      ...webApi.workspaces,
      list: () => Promise.resolve(fixtureWorkspacePage()),
    },
  } satisfies typeof webApi;

  return { api, submitBodies, taskId: fixtureTaskId };
}
