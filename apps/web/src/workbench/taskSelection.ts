import type { TaskId, WorkspaceId } from '../api/generated';
import { webApi } from '../api/client';

type TaskSelectionApi = Pick<typeof webApi, 'tasks'>;

export type WorkbenchRoute =
  | { kind: 'latest' }
  | { kind: 'new' }
  | { kind: 'task'; taskId: TaskId };

export function resolveInitialTask(defaultWorkspaceId: WorkspaceId): Promise<WorkbenchRoute>;
export function resolveInitialTask(
  api: TaskSelectionApi,
  defaultWorkspaceId: WorkspaceId,
): Promise<WorkbenchRoute>;
export async function resolveInitialTask(
  apiOrWorkspaceId: TaskSelectionApi | WorkspaceId,
  maybeWorkspaceId?: WorkspaceId,
): Promise<WorkbenchRoute> {
  const api = typeof apiOrWorkspaceId === 'string' ? webApi : apiOrWorkspaceId;
  const workspaceId = typeof apiOrWorkspaceId === 'string' ? apiOrWorkspaceId : maybeWorkspaceId;
  if (workspaceId === undefined) throw new Error('A default workspace is required');

  const page = await api.tasks.list({
    workspace_id: workspaceId,
    search: null,
    limit: 1,
    cursor: null,
  });
  const latest = page.items[0];
  return latest === undefined ? { kind: 'new' } : { kind: 'task', taskId: latest.task_id };
}
