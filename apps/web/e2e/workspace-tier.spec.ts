import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import type { Page } from '@playwright/test';

import { expect, test } from './fixtures/unifiedBinary';
import { openAuthenticatedRoute } from './fixtures/journey';
import { taskProjection, workspacePage } from './fixtures/productApi';

const execFileAsync = promisify(execFile);

async function selectWorkspace(page: Page, label: string): Promise<void> {
  await page.getByRole('combobox', { name: 'Workspace' }).click();
  await page.getByRole('option', { name: new RegExp(label, 'u') }).click();
}

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

  await expect(page.getByLabel('Workspace')).toContainText('Git fixture');
  await page.getByLabel('Workspace').click();
  await expect(page.getByRole('option', { name: /Git fixture/u })).toContainText(git.workspace_id);
  await page.keyboard.press('Escape');
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
  await selectWorkspace(page, 'Plain fixture');
  await expect(page.getByTestId('workspace-branch')).toHaveCount(0);
  await selectWorkspace(page, 'Git fixture');
  await expect(page.getByTestId('workspace-branch')).toHaveText('feature/refetched');
  await expect(page.locator('body')).not.toContainText(unifiedBinary.gitWorkspace);
  await expect(page.locator('body')).not.toContainText(unifiedBinary.plainWorkspace);
  await expect(page.locator('body')).not.toContainText(unifiedBinary.home);
});
