import { expect, type APIRequestContext } from '@playwright/test';

import type {
  ContextSnapshot,
  PlatformStatus,
  ReviewSnapshot,
  ReviewSubmissionPage,
  SettingsSnapshot,
  TaskPage,
  TaskProjection,
  TimelinePage,
  WorkspacePage,
} from '../../src/api/generated';

import type { UnifiedBinary } from './unifiedBinary';

function headers(server: UnifiedBinary): Record<string, string> {
  return { Authorization: `Bearer ${server.credential}` };
}

async function responseJson<T>(
  response: Awaited<ReturnType<APIRequestContext['get']>>,
): Promise<T> {
  expect(response.ok(), await response.text()).toBeTruthy();
  return response.json() as Promise<T>;
}

export function authenticatedGet(request: APIRequestContext, server: UnifiedBinary, path: string) {
  return request.get(`${server.baseUrl}/api/v1${path}`, { headers: headers(server) });
}

export async function status(
  request: APIRequestContext,
  server: UnifiedBinary,
): Promise<PlatformStatus> {
  return responseJson<PlatformStatus>(await authenticatedGet(request, server, '/status'));
}

export async function taskPage(
  request: APIRequestContext,
  server: UnifiedBinary,
  query = '',
): Promise<TaskPage> {
  const workspaces = await workspacePage(request, server);
  const workspace = workspaces.items[0];
  if (workspace === undefined) throw new Error('scenario has no registered workspace');
  const params = new URLSearchParams(query.startsWith('?') ? query.slice(1) : query);
  params.set('workspace_id', workspace.workspace_id);
  if (!params.has('limit')) params.set('limit', '100');
  return responseJson<TaskPage>(
    await authenticatedGet(request, server, `/tasks?${params.toString()}`),
  );
}

export async function workspacePage(
  request: APIRequestContext,
  server: UnifiedBinary,
): Promise<WorkspacePage> {
  return responseJson<WorkspacePage>(await authenticatedGet(request, server, '/workspaces'));
}

export async function taskProjection(
  request: APIRequestContext,
  server: UnifiedBinary,
  taskId: string,
): Promise<TaskProjection> {
  return responseJson<TaskProjection>(
    await authenticatedGet(request, server, `/tasks/${encodeURIComponent(taskId)}`),
  );
}

export async function timelinePage(
  request: APIRequestContext,
  server: UnifiedBinary,
  taskId: string,
  query = '',
): Promise<TimelinePage> {
  return responseJson<TimelinePage>(
    await authenticatedGet(
      request,
      server,
      `/tasks/${encodeURIComponent(taskId)}/timeline${query}`,
    ),
  );
}

export async function contextSnapshot(
  request: APIRequestContext,
  server: UnifiedBinary,
  taskId: string,
): Promise<ContextSnapshot> {
  return responseJson<ContextSnapshot>(
    await authenticatedGet(request, server, `/tasks/${encodeURIComponent(taskId)}/context`),
  );
}

export async function changes(
  request: APIRequestContext,
  server: UnifiedBinary,
  workspaceId: string,
): Promise<ReviewSnapshot> {
  return responseJson<ReviewSnapshot>(
    await authenticatedGet(
      request,
      server,
      `/workspaces/${encodeURIComponent(workspaceId)}/changes?limit=100`,
    ),
  );
}

export async function reviewSubmissions(
  request: APIRequestContext,
  server: UnifiedBinary,
  taskId: string,
): Promise<ReviewSubmissionPage> {
  return responseJson<ReviewSubmissionPage>(
    await authenticatedGet(
      request,
      server,
      `/tasks/${encodeURIComponent(taskId)}/review/submissions?limit=50`,
    ),
  );
}

export async function reviewSubmissionCount(
  request: APIRequestContext,
  server: UnifiedBinary,
  taskId: string,
): Promise<number | null> {
  const response = await authenticatedGet(
    request,
    server,
    `/tasks/${encodeURIComponent(taskId)}/review/submissions?limit=50`,
  );
  if (response.status() === 503) return null;
  return (await responseJson<ReviewSubmissionPage>(response)).items.length;
}

export async function settings(
  request: APIRequestContext,
  server: UnifiedBinary,
): Promise<SettingsSnapshot> {
  return responseJson<SettingsSnapshot>(await authenticatedGet(request, server, '/settings'));
}
