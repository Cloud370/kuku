import type { FileContent, ReviewSnapshot } from '../src/api/generated';
import { expect, test } from './fixtures/unifiedBinary';
import { firstTaskId, openAuthenticatedRoute } from './fixtures/journey';
import { workspacePage } from './fixtures/productApi';

test('keeps non-Git Files read-only without inventing a Changes model', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  const taskId = await firstTaskId(request, unifiedBinary);
  const workspaces = await workspacePage(request, unifiedBinary);
  const plain = workspaces.items.find((workspace) => workspace.branch === null);
  if (plain === undefined) throw new Error('scenario has no non-Git workspace');
  const headers = { Authorization: `Bearer ${unifiedBinary.credential}` };
  const changesResponse = await request.get(
    `${unifiedBinary.baseUrl}/api/v1/workspaces/${encodeURIComponent(plain.workspace_id)}/changes?limit=100`,
    { headers },
  );
  expect(changesResponse.ok(), await changesResponse.text()).toBeTruthy();
  const snapshot = (await changesResponse.json()) as ReviewSnapshot;
  expect(snapshot.workspace_id).toBe(plain.workspace_id);
  expect(snapshot.availability).toBe('not_git_repository');
  expect(snapshot.entries).toEqual([]);

  const page = await openAuthenticatedRoute(
    await browser.newContext(),
    unifiedBinary,
    `/tasks/${encodeURIComponent(taskId)}/review?workspace=${encodeURIComponent(plain.workspace_id)}`,
  );
  await expect(page.getByRole('region', { name: 'Review', exact: true })).toBeVisible();
  await page.getByRole('tab', { name: 'Changes' }).click();
  await expect(page.getByText('Changes unavailable for this workspace')).toBeVisible();
  await expect(page.getByLabel('Workspace changes')).toHaveCount(0);
  await expect(page.getByText(/staged|working tree|modified|untracked/i)).toHaveCount(0);

  await page.getByRole('tab', { name: 'Files' }).click();
  await expect(page.getByLabel('Find a file')).toBeVisible();
  const fileResponse = page.waitForResponse(
    (response) =>
      response
        .url()
        .includes(`/workspaces/${encodeURIComponent(plain.workspace_id)}/files/content`) &&
      new URL(response.url()).searchParams.get('path') === 'notes.txt',
  );
  await page.getByRole('button', { name: 'notes.txt', exact: true }).click();
  const content = (await (await fileResponse).json()) as FileContent;
  expect(content.path).toBe('notes.txt');
  expect(content.text).toContain('plain workspace fixture');
  await expect(page.getByText('plain workspace fixture', { exact: false })).toBeVisible();
  await expect(page.getByRole('textbox')).toHaveCount(0);
  await expect(page.locator('[contenteditable="true"]')).toHaveCount(0);
  await expect(page.getByText(unifiedBinary.plainWorkspace, { exact: false })).toHaveCount(0);
  await expect(page.getByRole('button', { name: /commit|stage|write|edit/i })).toHaveCount(0);
});
