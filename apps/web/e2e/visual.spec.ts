import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';

import type { Browser, BrowserContext, Page } from '@playwright/test';

import type { SubmitRunResponse } from '../src/api/generated';
import { authenticatedPage } from './fixtures/auth';
import { firstTaskId } from './fixtures/journey';
import { changes, contextSnapshot, taskProjection, workspacePage } from './fixtures/productApi';
import { completeScenario, releaseScenarioBarrier } from './fixtures/scenarioControl';
import { expect, test, type UnifiedBinary } from './fixtures/unifiedBinary';

const desktop = { height: 900, width: 1440 };

async function newVisualPage(
  browser: Browser,
  server: UnifiedBinary,
  viewport = desktop,
): Promise<{ context: BrowserContext; page: Page }> {
  const context = await browser.newContext({ viewport });
  const page = await authenticatedPage(context, server);
  await page.clock.setFixedTime(new Date('2026-07-18T12:00:00Z'));
  await page.emulateMedia({ reducedMotion: 'reduce' });
  return { context, page };
}

async function capture(page: Page, name: string): Promise<void> {
  await page.addStyleTag({
    content:
      '*,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important}[data-testid="connection-display-name"]{display:inline-block!important;width:5rem!important}',
  });
  await page.evaluate(async () => document.fonts.ready);
  await expect(page).toHaveScreenshot(name, {
    animations: 'disabled',
    caret: 'hide',
    mask: [
      page.getByTestId('connection-display-name'),
      page.getByText(/^kuku [a-f0-9]{8}$/u),
      page.getByText(/http:\/\/127\.0\.0\.1:\d+/u),
      page.getByLabel('Context').locator('span.font-mono'),
      page.locator('button[aria-label^="Select Request"]'),
    ],
    maskColor: '#262a2d',
  });
}

async function openReviewWithEmptySubmissions(page: Page, url: string): Promise<void> {
  const empty = page.getByText('No submitted reviews', { exact: true });
  const terminal = page.getByText(/^(?:No submitted reviews|Unable to load submitted reviews)$/u);
  for (let attempt = 0; attempt < 3; attempt += 1) {
    await page.goto(url);
    await expect(terminal).toBeVisible();
    if (await empty.isVisible()) return;
    await page.waitForTimeout(100 * (attempt + 1));
  }
  await expect(empty).toBeVisible();
}

async function openAvailableDiff(page: Page, path: string): Promise<void> {
  for (let attempt = 0; attempt < 3; attempt += 1) {
    const diffResponse = page.waitForResponse((response) =>
      response.url().includes('/changes/diff?'),
    );
    await page.getByLabel('Workspace changes').getByText(path, { exact: true }).click();
    const response = await diffResponse;
    if (response.status() === 200) return;

    const body = await response.text();
    if (response.status() !== 503 || attempt === 2) {
      expect(response.status(), body).toBe(200);
    }
    await page.waitForTimeout(100 * (attempt + 1));
  }
}

async function submitFollowUp(
  request: Parameters<typeof taskProjection>[0],
  server: UnifiedBinary,
  taskId: string,
): Promise<void> {
  const current = await taskProjection(request, server, taskId);
  const response = await request.post(
    `${server.baseUrl}/api/v1/tasks/${encodeURIComponent(taskId)}/runs`,
    {
      data: {
        expected_task_revision: current.task_revision,
        idempotency_key: 'visual-historical-follow-up',
        message: 'Capture a second immutable Request snapshot',
        skill_ids: [],
        tier_id: 'tier:e2e-balanced',
      },
      headers: { Authorization: `Bearer ${server.credential}` },
    },
  );
  expect(response.status(), await response.text()).toBe(202);
  const accepted = (await response.json()) as SubmitRunResponse;
  expect(accepted.task_id).toBe(taskId);
  await expect
    .poll(async () => (await contextSnapshot(request, server, taskId)).request_history.length)
    .toBeGreaterThan(1);
}

for (const width of [360, 768, 1440]) {
  test(`captures the deterministic populated Workbench at ${String(width)}px`, async ({
    browser,
    request,
    unifiedBinary,
  }) => {
    const taskId = await firstTaskId(request, unifiedBinary);
    const { context, page } = await newVisualPage(browser, unifiedBinary, {
      height: 900,
      width,
    });
    await page.goto(`${unifiedBinary.baseUrl}/tasks/${encodeURIComponent(taskId)}`);
    await expect(page.getByRole('main', { name: 'Chat' })).toBeVisible();
    await capture(page, `workbench-populated-${String(width)}.png`);
    await context.close();
  });
}

test('captures loading, empty, transport error, and Needs Attention Workbench states', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  const taskId = await firstTaskId(request, unifiedBinary);

  const loading = await newVisualPage(browser, unifiedBinary);
  let releaseLoading = () => {};
  const blockedTask = new Promise<void>((resolveBlocked) => {
    releaseLoading = resolveBlocked;
  });
  await loading.page.route(`**/api/v1/tasks/${encodeURIComponent(taskId)}`, async (route) => {
    await blockedTask;
    await route.continue();
  });
  await loading.page.goto(`${unifiedBinary.baseUrl}/tasks/${encodeURIComponent(taskId)}`);
  await expect(loading.page.getByText('Loading Task', { exact: true })).toBeVisible();
  await capture(loading.page, 'workbench-loading.png');
  releaseLoading();
  await loading.context.close();

  const empty = await newVisualPage(browser, unifiedBinary);
  await empty.page.goto(`${unifiedBinary.baseUrl}/tasks/${encodeURIComponent(taskId)}`);
  await empty.page.getByRole('combobox', { name: 'Workspace' }).click();
  await empty.page.getByRole('option', { name: /Plain fixture/u }).click();
  await expect(empty.page.getByText('No Tasks yet')).toBeVisible();
  await expect(empty.page.getByText('Choose a Task to start chatting.')).toBeVisible();
  await capture(empty.page, 'workbench-empty.png');
  await empty.context.close();

  const transport = await newVisualPage(browser, unifiedBinary);
  await transport.page.route(`**/api/v1/tasks/${encodeURIComponent(taskId)}`, async (route) => {
    await route.fulfill({
      body: JSON.stringify({
        api_version: 'v1',
        code: 'server_busy',
        message: 'The Task transport is temporarily unavailable.',
        retryable: true,
        trace_id: 'visual-transport-error',
      }),
      contentType: 'application/json',
      status: 503,
    });
  });
  await transport.page.goto(`${unifiedBinary.baseUrl}/tasks/${encodeURIComponent(taskId)}`);
  await expect(transport.page.getByRole('alert')).toContainText('Task unavailable');
  await capture(transport.page, 'workbench-transport-error.png');
  await transport.context.close();

  await releaseScenarioBarrier(request, unifiedBinary, 'after-tool');
  const attention = await newVisualPage(browser, unifiedBinary);
  await attention.page.goto(`${unifiedBinary.baseUrl}/tasks/${encodeURIComponent(taskId)}`);
  await expect(attention.page.getByRole('group', { name: 'Permission request' })).toBeVisible();
  await attention.page
    .getByRole('searchbox')
    .fill('Exercise the full deterministic browser scenario');
  await capture(attention.page, 'workbench-needs-attention.png');
  await attention.context.close();
});

test('captures current, historical, and mobile-sheet Context states', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  test.setTimeout(120_000);
  const taskId = await firstTaskId(request, unifiedBinary);

  const current = await newVisualPage(browser, unifiedBinary);
  await current.page.goto(`${unifiedBinary.baseUrl}/tasks/${encodeURIComponent(taskId)}`);
  await expect(current.page.getByText('Current Context')).toBeVisible();
  await capture(current.page, 'context-current.png');
  await current.context.close();

  const mobile = await newVisualPage(browser, unifiedBinary, { height: 800, width: 360 });
  await mobile.page.goto(`${unifiedBinary.baseUrl}/tasks/${encodeURIComponent(taskId)}`);
  await mobile.page.getByRole('button', { name: 'Open Agent Context' }).click();
  await expect(mobile.page.getByRole('dialog', { name: 'Agent Context' })).toBeVisible();
  await expect(mobile.page.getByText('Current Context')).toBeVisible();
  await capture(mobile.page, 'context-mobile-sheet-360.png');
  await mobile.context.close();

  await completeScenario(request, unifiedBinary);
  await submitFollowUp(request, unifiedBinary, taskId);
  const historical = await newVisualPage(browser, unifiedBinary);
  await historical.page.goto(`${unifiedBinary.baseUrl}/tasks/${encodeURIComponent(taskId)}`);
  const olderRequest = historical.page
    .locator('button[aria-label^="Select Request"][aria-pressed="false"]')
    .first();
  await expect(olderRequest).toBeVisible();
  await olderRequest.click();
  const context = historical.page.getByLabel('Context');
  await expect(context.getByRole('status')).toHaveText('Context loaded');
  await expect(context.getByText('Historical Request')).toBeVisible();
  await capture(historical.page, 'context-historical.png');
  await historical.context.close();
});

test('captures Git, non-Git Files, and outdated Review states', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  test.setTimeout(120_000);
  const taskId = await firstTaskId(request, unifiedBinary);
  await completeScenario(request, unifiedBinary);
  const projection = await taskProjection(request, unifiedBinary, taskId);
  const snapshot = await changes(request, unifiedBinary, projection.task.workspace_id);
  const selectableEntry = snapshot.entries.find((entry) => (entry.additions ?? 0) > 0);
  const changedPath = selectableEntry?.path;
  if (changedPath === undefined) throw new Error('scenario returned no change with new lines');

  const git = await newVisualPage(browser, unifiedBinary);
  await openReviewWithEmptySubmissions(
    git.page,
    `${unifiedBinary.baseUrl}/tasks/${encodeURIComponent(taskId)}/review`,
  );
  await git.page.getByRole('tab', { name: 'Changes' }).click();
  await expect(git.page.getByLabel('Workspace changes')).toContainText(changedPath);
  await openAvailableDiff(git.page, changedPath);
  const line = git.page.getByRole('button', { name: /^Select new line /u }).first();
  await expect(line).toBeVisible();
  await capture(git.page, 'review-git-changes.png');

  await line.click();
  await line.click();
  await git.page
    .getByRole('textbox', { name: /^Comment for /u })
    .fill('This visual anchor must become outdated.');
  await writeFile(join(unifiedBinary.gitWorkspace, changedPath), 'export const answer = 43;\n');
  const rejected = git.page.waitForResponse(
    (response) => response.url().endsWith('/review/annotations') && response.status() === 409,
  );
  await git.page.getByRole('button', { name: 'Submit review' }).click();
  await rejected;
  await expect(git.page.getByText('Outdated anchor')).toBeVisible();
  await capture(git.page, 'review-outdated-note.png');
  await git.context.close();

  const workspaces = await workspacePage(request, unifiedBinary);
  const plain = workspaces.items.find((workspace) => workspace.branch === null);
  if (plain === undefined) throw new Error('scenario has no non-Git workspace');
  const nonGit = await newVisualPage(browser, unifiedBinary);
  await openReviewWithEmptySubmissions(
    nonGit.page,
    `${unifiedBinary.baseUrl}/tasks/${encodeURIComponent(taskId)}/review?workspace=${encodeURIComponent(plain.workspace_id)}`,
  );
  await nonGit.page.getByRole('tab', { name: 'Files' }).click();
  await nonGit.page.getByRole('button', { name: 'notes.txt', exact: true }).click();
  await expect(nonGit.page.getByText('plain workspace fixture', { exact: false })).toBeVisible();
  await capture(nonGit.page, 'review-non-git-files.png');
  await nonGit.context.close();
});

test('captures Settings loading, transport, validation, populated, and Guide states', async ({
  browser,
  unifiedBinary,
}) => {
  const loading = await newVisualPage(browser, unifiedBinary);
  let releaseLoading = () => {};
  const blockedSettings = new Promise<void>((resolveBlocked) => {
    releaseLoading = resolveBlocked;
  });
  await loading.page.route('**/api/v1/settings', async (route) => {
    await blockedSettings;
    await route.continue();
  });
  await loading.page.goto(`${unifiedBinary.baseUrl}/settings`);
  await expect(loading.page.getByRole('status')).toContainText('Loading Settings');
  await capture(loading.page, 'settings-loading.png');
  releaseLoading();
  await loading.context.close();

  const transport = await newVisualPage(browser, unifiedBinary);
  await transport.page.route('**/api/v1/settings', async (route) => {
    await route.fulfill({
      body: JSON.stringify({
        api_version: 'v1',
        code: 'server_busy',
        message: 'Settings transport unavailable.',
        retryable: true,
        trace_id: 'visual-settings-transport',
      }),
      contentType: 'application/json',
      status: 503,
    });
  });
  await transport.page.goto(`${unifiedBinary.baseUrl}/settings`);
  await expect(transport.page.getByRole('alert')).toContainText('Settings could not be loaded');
  await capture(transport.page, 'settings-transport-error.png');
  await transport.context.close();

  const validation = await newVisualPage(browser, unifiedBinary);
  await validation.page.route('**/api/v1/settings', async (route) => {
    if (route.request().method() !== 'PATCH') {
      await route.continue();
      return;
    }
    await route.fulfill({
      body: JSON.stringify({
        api_version: 'v1',
        code: 'invalid_request',
        message: 'Maximum concurrent runs is outside the accepted range.',
        retryable: false,
        trace_id: 'visual-settings-validation',
      }),
      contentType: 'application/json',
      status: 400,
    });
  });
  await validation.page.goto(`${unifiedBinary.baseUrl}/settings`);
  const maximumRuns = validation.page.getByLabel('Maximum concurrent runs');
  await expect(maximumRuns).toBeVisible();
  await maximumRuns.fill('2');
  await validation.page.getByRole('button', { name: 'Save Settings' }).click();
  await expect(validation.page.getByRole('alert')).toContainText(
    'Settings reconciled with current server state',
  );
  await capture(validation.page, 'settings-validation-error.png');
  await validation.context.close();

  const populated = await newVisualPage(browser, unifiedBinary);
  await populated.page.goto(`${unifiedBinary.baseUrl}/settings`);
  await expect(populated.page.getByRole('heading', { name: 'Settings' })).toBeVisible();
  await capture(populated.page, 'settings-populated.png');
  await populated.page.goto(`${unifiedBinary.baseUrl}/guide`);
  await expect(populated.page.getByRole('heading', { name: 'Guide' })).toBeVisible();
  await capture(populated.page, 'guide-populated.png');
  await populated.context.close();
});
