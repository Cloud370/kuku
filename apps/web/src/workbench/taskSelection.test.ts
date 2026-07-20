import { describe, expect, it, vi } from 'vitest';

import { webApi } from '../api/client';

import { resolveInitialTask } from './taskSelection';

type WebApi = typeof webApi;

const workspaceId = 'wsp_000000000000000000000001';

describe('resolveInitialTask', () => {
  it('resolves the root to the latest persisted Task', async () => {
    const list = vi.fn<WebApi['tasks']['list']>();
    list.mockResolvedValue({
      api_version: 1,
      items: [
        {
          active_run_id: null,
          latest_run_id: null,
          state: 'completed',
          task_id: 'tsk_000000000000000000000003',
          title: 'Inspect Unicode title',
          updated_at: '2026-07-18T09:00:00Z',
          workspace_id: workspaceId,
        },
      ],
      next_cursor: null,
    });
    const api = { ...webApi, tasks: { ...webApi.tasks, list } };

    await expect(resolveInitialTask(api, workspaceId)).resolves.toEqual({
      kind: 'task',
      taskId: 'tsk_000000000000000000000003',
    });
    expect(list).toHaveBeenCalledWith({
      workspace_id: workspaceId,
      search: null,
      limit: 1,
      cursor: null,
    });
  });

  it('resolves an empty workspace to explicit Task creation', async () => {
    const list = vi.fn<WebApi['tasks']['list']>();
    list.mockResolvedValue({ api_version: 1, items: [], next_cursor: null });
    const api = { ...webApi, tasks: { ...webApi.tasks, list } };

    await expect(resolveInitialTask(api, workspaceId)).resolves.toEqual({ kind: 'new' });
  });
});
