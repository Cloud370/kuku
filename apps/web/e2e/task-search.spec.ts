import type { APIRequestContext } from '@playwright/test';

import type { CreateTaskRequest, TaskSummary } from '../src/api/generated';
import { expect, test, type UnifiedBinary } from './fixtures/unifiedBinary';
import { openAuthenticatedRoute } from './fixtures/journey';
import { authenticatedGet, taskPage, workspacePage } from './fixtures/productApi';

const REQUIRED_DURABLE_TASKS = 151;

async function allTasks(request: APIRequestContext, server: UnifiedBinary): Promise<TaskSummary[]> {
  const tasks: TaskSummary[] = [];
  let cursor: string | null = null;
  do {
    const page = await taskPage(
      request,
      server,
      `?limit=100${cursor === null ? '' : `&cursor=${encodeURIComponent(cursor)}`}`,
    );
    tasks.push(...page.items);
    cursor = page.next_cursor;
  } while (cursor !== null);
  return tasks;
}

async function ensureDurableTaskDepth(
  request: APIRequestContext,
  server: UnifiedBinary,
): Promise<void> {
  const tasks = await allTasks(request, server);
  const workspaces = await workspacePage(request, server);
  const workspace = workspaces.items[0];
  if (workspace === undefined) throw new Error('scenario has no registered workspace');
  await Promise.all(
    Array.from({ length: Math.max(0, REQUIRED_DURABLE_TASKS - tasks.length) }, (_, index) =>
      request
        .post(`${server.baseUrl}/api/v1/tasks`, {
          data: {
            idempotency_key: `e2e-search-depth-${String(index)}`,
            workspace_id: workspace.workspace_id,
          } satisfies CreateTaskRequest,
          headers: { Authorization: `Bearer ${server.credential}` },
        })
        .then(async (response) => {
          expect(response.ok(), await response.text()).toBeTruthy();
        }),
    ),
  );
}

test('server search finds a task beyond the loaded page and scopes opaque cursors to its query', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  await ensureDurableTaskDepth(request, unifiedBinary);
  expect((await allTasks(request, unifiedBinary)).length).toBeGreaterThanOrEqual(
    REQUIRED_DURABLE_TASKS,
  );
  const initial = await taskPage(request, unifiedBinary, '?limit=100');
  expect(initial.next_cursor).not.toBeNull();
  const hidden = await taskPage(request, unifiedBinary, '?limit=100&search=only-hidden-match');
  expect(hidden.items).toHaveLength(1);
  const hiddenTask = hidden.items[0];
  if (hiddenTask === undefined) throw new Error('server search exposed no hidden Task');
  expect(initial.items.some((task) => task.task_id === hiddenTask.task_id)).toBeFalsy();

  const workspaces = await workspacePage(request, unifiedBinary);
  const workspace = workspaces.items[0];
  if (workspace === undefined || initial.next_cursor === null) {
    throw new Error('scenario exposed no workspace-bound opaque cursor');
  }
  expect(Number(initial.next_cursor)).toBeNaN();
  const crossQuery = new URLSearchParams({
    cursor: initial.next_cursor,
    limit: '100',
    search: 'only-hidden-match',
    workspace_id: workspace.workspace_id,
  });
  const rejected = await authenticatedGet(request, unifiedBinary, `/tasks?${crossQuery}`);
  expect(rejected.status()).toBe(400);

  const page = await openAuthenticatedRoute(await browser.newContext(), unifiedBinary);
  const searchRequest = page.waitForRequest((candidate) => {
    const url = new URL(candidate.url());
    return (
      url.pathname === '/api/v1/tasks' && url.searchParams.get('search') === 'only-hidden-match'
    );
  });
  await page.getByLabel('Search Tasks').fill('only-hidden-match');
  const observed = new URL((await searchRequest).url());
  expect(observed.searchParams.get('workspace_id')).toBe(workspace.workspace_id);
  expect(observed.searchParams.get('search')).toBe('only-hidden-match');
  expect(observed.searchParams.has('cursor')).toBeFalsy();

  const result = page.getByRole('button', { name: hiddenTask.title, exact: true });
  await expect(result).toBeVisible();
  await result.click();
  await expect(page).toHaveURL(new RegExp(`/tasks/${hiddenTask.task_id}$`));

  await page.getByLabel('Search Tasks').fill('');
  const newest = initial.items[0];
  if (newest === undefined) throw new Error('server exposed no newest Task');
  const newestResult = page
    .getByLabel('Task navigation')
    .locator('section')
    .getByRole('button', { name: newest.title, exact: true })
    .first();
  await expect(newestResult).toBeVisible();
  await newestResult.click();
  await expect.poll(() => new URL(page.url()).pathname).toBe(`/tasks/${newest.task_id}`);
});
