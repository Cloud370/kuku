import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

import { expect, test } from './fixtures/unifiedBinary';
import { openAuthenticatedRoute } from './fixtures/journey';
import { taskProjection, workspacePage } from './fixtures/productApi';

const execFileAsync = promisify(execFile);

test('renders a registered workspace and selected tier without exposing its filesystem path', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  const workspaces = await workspacePage(request, unifiedBinary);
  const git = workspaces.items.find(({ label }) => label === 'Git fixture');
  const plain = workspaces.items.find(({ label }) => label === 'Plain fixture');
  expect(git?.branch).toBe('feature/ui');
  expect(plain?.branch).toBeNull();
  if (git === undefined || plain === undefined)
    throw new Error('scenario workspaces are incomplete');
  const task = await taskProjection(request, unifiedBinary, unifiedBinary.scenario.taskId);
  expect(task.task.workspace_id).toBe(git.workspace_id);
  expect(task.selected_tier_id).toBe('tier:e2e-balanced');
  const page = await openAuthenticatedRoute(
    await browser.newContext(),
    unifiedBinary,
    `/tasks/${encodeURIComponent(task.task.task_id)}`,
  );

  await expect(page.getByLabel('Workspace')).toHaveValue(git.workspace_id);
  await expect(page.getByTestId('workspace-branch')).toHaveText('feature/ui');
  await expect(page.getByLabel('Choose Tier')).toContainText('e2e-balanced');
  await execFileAsync('git', ['checkout', '-b', 'feature/refetched'], {
    cwd: unifiedBinary.gitWorkspace,
  });
  await expect
    .poll(async () => {
      const refreshed = await workspacePage(request, unifiedBinary);
      return refreshed.items.find(({ workspace_id }) => workspace_id === git.workspace_id)?.branch;
    })
    .toBe('feature/refetched');
  await page.reload();
  await expect(page.getByTestId('workspace-branch')).toHaveText('feature/refetched');
  await page.getByLabel('Workspace').selectOption(plain.workspace_id);
  await expect(page.getByTestId('workspace-branch')).toHaveCount(0);
  await page.getByLabel('Workspace').selectOption(git.workspace_id);
  await expect(page.getByTestId('workspace-branch')).toHaveText('feature/refetched');
  await expect(page.locator('body')).not.toContainText(unifiedBinary.gitWorkspace);
  await expect(page.locator('body')).not.toContainText(unifiedBinary.plainWorkspace);
  await expect(page.locator('body')).not.toContainText(unifiedBinary.home);
});
