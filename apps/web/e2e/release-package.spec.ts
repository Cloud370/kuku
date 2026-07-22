import { openAuthenticatedRoute } from './fixtures/journey';
import {
  changes,
  reviewSubmissionCount,
  reviewSubmissions,
  taskProjection,
} from './fixtures/productApi';
import { expect, test } from './fixtures/unifiedBinary';

test.skip(
  process.env.KUKU_E2E_RELEASE_PACKAGE !== '1',
  'release-package checks require an extracted production-feature binary',
);

test('runs packaged Workbench, Composer, Review, Settings, and Guide behavior', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  expect(unifiedBinary.releasePackage).toBe(true);
  expect(unifiedBinary.scenario.name).toBe('release_package');

  const projection = await taskProjection(request, unifiedBinary, unifiedBinary.scenario.taskId);
  expect(['completed', 'stopped', 'failed', 'interrupted']).toContain(projection.latest_run?.state);

  const htmlResponse = await request.get(`${unifiedBinary.baseUrl}/`);
  expect(htmlResponse.status()).toBe(200);
  const html = await htmlResponse.text();
  const asset = html.match(/\/assets\/[^"']+\.js/)?.[0];
  expect(asset).toBeTruthy();
  if (asset === undefined) throw new Error('embedded HTML has no JavaScript asset');
  expect((await request.get(`${unifiedBinary.baseUrl}${asset}`)).status()).toBe(200);

  const context = await browser.newContext();
  const page = await openAuthenticatedRoute(
    context,
    unifiedBinary,
    `/tasks/${encodeURIComponent(unifiedBinary.scenario.taskId)}`,
  );
  await expect(page.getByRole('main', { name: 'Chat' })).toBeVisible();
  await expect(page.getByLabel('Composer')).toBeVisible();
  const message = page.getByRole('textbox', { name: 'Message' });
  await message.fill('Verify the packaged Composer path');
  await expect(page.getByRole('button', { name: 'Send' })).toBeEnabled();

  const snapshot = await changes(request, unifiedBinary, projection.task.workspace_id);
  expect(snapshot.availability).toBe('available');
  const changedPath = snapshot.entries[0]?.path;
  if (changedPath === undefined) throw new Error('release workspace returned no changed path');
  const before = await reviewSubmissions(request, unifiedBinary, unifiedBinary.scenario.taskId);
  await page.goto(
    `${unifiedBinary.baseUrl}/tasks/${encodeURIComponent(unifiedBinary.scenario.taskId)}/review`,
  );
  await expect(page.getByRole('region', { name: 'Review', exact: true })).toBeVisible();
  await page.getByRole('tab', { name: 'Changes' }).click();
  await expect(page.getByLabel('Workspace changes')).toContainText(changedPath);
  await page.getByLabel('Workspace changes').getByRole('button').first().click();
  const line = page.getByRole('button', { name: /^Select new line / }).first();
  await expect(line).toBeVisible();
  await line.click();
  await line.click();
  await page.getByRole('textbox', { name: /^Comment for / }).fill('Packaged review note.');
  const reviewResponse = page.waitForResponse(
    (response) =>
      response.url().endsWith('/review/annotations') && response.request().method() === 'POST',
  );
  await page.getByRole('button', { name: 'Submit review' }).click();
  expect((await reviewResponse).ok()).toBe(true);
  await expect
    .poll(async () => reviewSubmissionCount(request, unifiedBinary, unifiedBinary.scenario.taskId))
    .toBe(before.items.length + 1);

  await page.goto(`${unifiedBinary.baseUrl}/settings`);
  await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible();
  await page.goto(`${unifiedBinary.baseUrl}/guide`);
  await expect(page.getByRole('heading', { name: 'Guide' })).toBeVisible();
  await context.close();
});

test('keeps packaged mobile drawers, Composer, Review notes, and viewport bounds usable', async ({
  browser,
  request,
  unifiedBinary,
}, testInfo) => {
  test.skip(testInfo.project.name !== 'webkit-360', 'critical mobile path runs in webkit-360');
  const context = await browser.newContext({ viewport: { height: 800, width: 360 } });
  const page = await openAuthenticatedRoute(
    context,
    unifiedBinary,
    `/tasks/${encodeURIComponent(unifiedBinary.scenario.taskId)}`,
  );

  await page.getByRole('button', { name: 'Open Tasks' }).click();
  await expect(page.getByRole('dialog', { name: 'Tasks' })).toBeVisible();
  await page.getByRole('button', { name: 'Close Tasks' }).click();
  await expect(page.getByRole('dialog', { name: 'Tasks' })).toHaveCount(0);

  const composer = page.getByLabel('Composer');
  await expect(composer).toBeVisible();
  const message = page.getByRole('textbox', { name: 'Message' });
  await message.fill('Exercise the packaged permission interaction');
  const submitResponse = page.waitForResponse(
    (response) => response.url().endsWith('/runs') && response.request().method() === 'POST',
  );
  await page.getByRole('button', { name: 'Send' }).click();
  expect((await submitResponse).status()).toBe(202);
  await expect
    .poll(async () => {
      const current = await taskProjection(request, unifiedBinary, unifiedBinary.scenario.taskId);
      return current.timeline.some(
        (item) => item.type === 'interaction' && item.item.status === 'pending',
      );
    })
    .toBe(true);
  await page.reload();
  const permission = page.getByRole('group', { name: 'Permission request' });
  await expect(permission).toBeVisible();
  const decisionResponse = page.waitForResponse(
    (response) =>
      response.url().includes('/interactions/') && response.request().method() === 'POST',
  );
  await permission.getByRole('button', { name: 'Deny' }).click();
  expect((await decisionResponse).status()).toBe(202);
  await expect
    .poll(async () => {
      const current = await taskProjection(request, unifiedBinary, unifiedBinary.scenario.taskId);
      return current.timeline.some(
        (item) =>
          item.type === 'interaction' &&
          item.item.status === 'resolved' &&
          item.item.selected_choice_id === 'deny',
      );
    })
    .toBe(true);
  await page.reload();
  await expect(permission).toContainText('Resolved');
  await message.focus();
  const composerBounds = await composer.boundingBox();
  expect(composerBounds).not.toBeNull();
  expect((composerBounds?.x ?? 361) + (composerBounds?.width ?? 0)).toBeLessThanOrEqual(360);

  const projection = await taskProjection(request, unifiedBinary, unifiedBinary.scenario.taskId);
  const snapshot = await changes(request, unifiedBinary, projection.task.workspace_id);
  const changedPath = snapshot.entries[0]?.path;
  if (changedPath === undefined) throw new Error('release workspace returned no changed path');
  await page.goto(
    `${unifiedBinary.baseUrl}/tasks/${encodeURIComponent(unifiedBinary.scenario.taskId)}/review`,
  );
  await page.getByLabel('Workspace changes').getByRole('button').first().click();
  const mobileLine = page.getByRole('button', { name: /^Select new line / }).first();
  await mobileLine.click();
  await mobileLine.click();
  await page.getByRole('textbox', { name: /^Comment for / }).fill('Mobile review note.');
  await expect(page.getByLabel('Review notes')).toContainText('Mobile review note.');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
  await context.close();
});
