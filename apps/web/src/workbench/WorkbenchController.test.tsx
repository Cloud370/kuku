import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import platformStatusJson from '../api/generated/fixtures/platform_status.json';
import type { ContextCatalog, PlatformStatus, WorkspacePage } from '../api/generated';
import { webApi } from '../api/client';
import { createFakeWorkbenchServer } from './test/fakeServer';
import { WorkbenchController } from './WorkbenchController';

const workspaceId = 'wsp_000000000000000000000001';

afterEach(() => {
  cleanup();
});

describe('WorkbenchController', () => {
  it('loads the default Workspace catalog through the shared client contract', async () => {
    const workspaces = {
      api_version: 1,
      items: [
        {
          availability: 'available',
          branch: 'feature/web',
          is_default: true,
          label: 'kuku',
          workspace_id: workspaceId,
        },
      ],
      server_revision: 'revision-workspaces',
    } satisfies WorkspacePage;
    const catalog = {
      agents: [],
      api_version: 1,
      revision: 'revision-catalog',
      skills: [],
      tiers: [],
      tools: [],
    } satisfies ContextCatalog;
    const api = {
      ...webApi,
      catalog: { ...webApi.catalog, workspace: vi.fn().mockResolvedValue(catalog) },
      workspaces: { ...webApi.workspaces, list: vi.fn().mockResolvedValue(workspaces) },
    };

    render(
      <WorkbenchController
        api={api}
        platform={structuredClone(platformStatusJson) as PlatformStatus}
        route={{ kind: 'new' }}
      >
        {(view) => <p>{view.catalog?.revision ?? 'Loading catalog'}</p>}
      </WorkbenchController>,
    );

    expect(await screen.findByText('revision-catalog')).toBeVisible();
    expect(api.catalog.workspace).toHaveBeenCalledWith(workspaceId, { search: null });
  });

  it('surfaces a catalog failure and retries the workspace catalog', async () => {
    const user = userEvent.setup();
    const catalog = {
      agents: [],
      api_version: 1,
      revision: 'revision-recovered',
      skills: [],
      tiers: [],
      tools: [],
    } satisfies ContextCatalog;
    const api = {
      ...webApi,
      catalog: {
        ...webApi.catalog,
        workspace: vi
          .fn()
          .mockRejectedValueOnce(new Error('Catalog temporarily unavailable'))
          .mockResolvedValue(catalog),
      },
      workspaces: {
        ...webApi.workspaces,
        list: vi.fn().mockResolvedValue({
          api_version: 1,
          items: [
            {
              availability: 'available',
              branch: 'feature/web',
              is_default: true,
              label: 'kuku',
              workspace_id: workspaceId,
            },
          ],
          server_revision: 'revision-workspaces',
        }),
      },
    };

    render(
      <WorkbenchController
        api={api}
        platform={structuredClone(platformStatusJson) as PlatformStatus}
        route={{ kind: 'new' }}
      >
        {(view) => (
          <>
            {view.catalogError === null ? (
              <p>{view.catalog?.revision ?? 'Loading catalog'}</p>
            ) : (
              <>
                <p>{view.catalogError}</p>
                <button
                  type="button"
                  onClick={() => {
                    view.retryCatalog();
                  }}
                >
                  Retry catalog
                </button>
              </>
            )}
          </>
        )}
      </WorkbenchController>,
    );

    expect(await screen.findByText('Catalog temporarily unavailable')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Retry catalog' }));
    expect(await screen.findByText('revision-recovered')).toBeVisible();
  });

  it('reloads the catalog when the selected Workspace changes', async () => {
    const secondWorkspaceId = 'wsp_000000000000000000000002';
    const workspaces = {
      api_version: 1,
      items: [
        {
          availability: 'available',
          branch: 'feature/web',
          is_default: true,
          label: 'kuku',
          workspace_id: workspaceId,
        },
        {
          availability: 'available',
          branch: 'feature/docs',
          is_default: false,
          label: 'docs',
          workspace_id: secondWorkspaceId,
        },
      ],
      server_revision: 'revision-workspaces',
    } satisfies WorkspacePage;
    const api = {
      ...webApi,
      catalog: {
        ...webApi.catalog,
        workspace: vi.fn().mockImplementation((selectedWorkspaceId: string) =>
          Promise.resolve({
            agents: [],
            api_version: 1,
            revision: `catalog:${selectedWorkspaceId}`,
            skills: [],
            tiers: [],
            tools: [],
          }),
        ),
      },
      tasks: {
        ...webApi.tasks,
        list: vi.fn().mockResolvedValue({
          api_version: 1,
          items: [],
          next_cursor: null,
          server_revision: 'revision-tasks',
        }),
      },
      workspaces: { ...webApi.workspaces, list: vi.fn().mockResolvedValue(workspaces) },
    };
    const user = userEvent.setup();

    render(
      <WorkbenchController
        api={api}
        platform={structuredClone(platformStatusJson) as PlatformStatus}
        route={{ kind: 'new' }}
      >
        {(view) => (
          <>
            <p>{view.catalog?.revision ?? 'Loading catalog'}</p>
            <button
              type="button"
              onClick={() => {
                view.selectWorkspace(secondWorkspaceId);
              }}
            >
              Select docs
            </button>
            <button
              type="button"
              onClick={() => {
                view.searchCatalog('rust');
              }}
            >
              Search skills
            </button>
          </>
        )}
      </WorkbenchController>,
    );

    expect(await screen.findByText(`catalog:${workspaceId}`)).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Select docs' }));
    expect(await screen.findByText(`catalog:${secondWorkspaceId}`)).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Search skills' }));
    expect(api.catalog.workspace).toHaveBeenLastCalledWith(secondWorkspaceId, { search: 'rust' });
  });

  it('surfaces an initial Task load failure and recovers on retry', async () => {
    const user = userEvent.setup();
    const server = createFakeWorkbenchServer();
    const projection = await server.api.tasks.get(server.taskId);
    const get = vi
      .fn()
      .mockRejectedValueOnce(new Error('Task temporarily unavailable'))
      .mockResolvedValue(projection);
    const api = { ...server.api, tasks: { ...server.api.tasks, get } };

    render(
      <WorkbenchController
        api={api}
        platform={structuredClone(platformStatusJson) as PlatformStatus}
        route={{ kind: 'task', taskId: server.taskId }}
      >
        {(view) => (
          <>
            <p>{view.snapshot.lastError ?? view.snapshot.projection?.task.title ?? 'Loading'}</p>
            <button
              type="button"
              onClick={() => {
                void view.onRetry();
              }}
            >
              Retry Task
            </button>
          </>
        )}
      </WorkbenchController>,
    );

    expect(await screen.findByText('Task temporarily unavailable')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Retry Task' }));
    expect(await screen.findByText(projection.task.title)).toBeVisible();
    expect(get).toHaveBeenCalledTimes(2);
  });

  it('clears the selected Task when switching to an empty Workspace', async () => {
    const user = userEvent.setup();
    const server = createFakeWorkbenchServer();
    const secondWorkspaceId = 'wsp_000000000000000000000002';
    const api = {
      ...server.api,
      tasks: {
        ...server.api.tasks,
        list: vi.fn().mockImplementation((query: { workspace_id: string }) =>
          Promise.resolve(
            query.workspace_id === secondWorkspaceId
              ? {
                  api_version: 1,
                  items: [],
                  next_cursor: null,
                  server_revision: 'revision-empty',
                }
              : {
                  api_version: 1,
                  items: [
                    {
                      active_run_id: null,
                      latest_run_id: null,
                      state: 'draft',
                      task_id: server.taskId,
                      title: 'Loaded Task',
                      updated_at: '2026-07-18T00:00:00Z',
                      workspace_id: workspaceId,
                    },
                  ],
                  next_cursor: null,
                  server_revision: 'revision-loaded',
                },
          ),
        ),
      },
      workspaces: {
        ...server.api.workspaces,
        list: vi.fn().mockResolvedValue({
          api_version: 1,
          items: [
            {
              availability: 'available',
              branch: 'feature/web',
              is_default: true,
              label: 'kuku',
              workspace_id: workspaceId,
            },
            {
              availability: 'available',
              branch: 'feature/empty',
              is_default: false,
              label: 'empty',
              workspace_id: secondWorkspaceId,
            },
          ],
          server_revision: 'revision-workspaces',
        }),
      },
    };

    render(
      <WorkbenchController
        api={api}
        platform={structuredClone(platformStatusJson) as PlatformStatus}
      >
        {(view) => (
          <>
            <p>{view.snapshot.selectedTaskId ?? 'No Task'}</p>
            <button
              type="button"
              onClick={() => {
                view.selectWorkspace(secondWorkspaceId);
              }}
            >
              Select empty
            </button>
          </>
        )}
      </WorkbenchController>,
    );

    expect(await screen.findByText(server.taskId)).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Select empty' }));
    expect(await screen.findByText('No Task')).toBeVisible();
  });
});
