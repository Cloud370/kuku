import type { APIRequestContext, Locator, Page } from '@playwright/test';

import { expect, test, type UnifiedBinary } from './fixtures/unifiedBinary';
import { expectNoProductOverflow, firstTaskId, openAuthenticatedRoute } from './fixtures/journey';
import { taskPage } from './fixtures/productApi';
import { failScenarioBarrier, releaseScenarioBarrier } from './fixtures/scenarioControl';

async function expectFocusInside(page: Page, surface: Locator): Promise<void> {
  expect(
    await surface.evaluate((element) => element.contains(element.ownerDocument.activeElement)),
  ).toBe(true);
  await expect(page.locator(':focus')).toBeVisible();
}

async function expectNoUsageAnnouncements(page: Page): Promise<void> {
  const announcements = (await page.locator('[aria-live]').allTextContents()).join(' ');
  expect(announcements).not.toMatch(/token|usage/i);
}

async function draftTaskId(request: APIRequestContext, server: UnifiedBinary): Promise<string> {
  const tasks = await taskPage(request, server);
  const task = tasks.items.find((candidate) => candidate.state === 'draft');
  if (task === undefined) throw new Error('scenario must preload a durable Draft Task');
  return task.task_id;
}

test('traps and restores keyboard focus in mobile Tasks and Agent Context', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  const taskId = await firstTaskId(request, unifiedBinary);
  const draftId = await draftTaskId(request, unifiedBinary);
  const context = await browser.newContext({ viewport: { height: 800, width: 360 } });
  const page = await openAuthenticatedRoute(
    context,
    unifiedBinary,
    `/tasks/${encodeURIComponent(taskId)}`,
  );
  await page.emulateMedia({ forcedColors: 'active', reducedMotion: 'reduce' });
  expect(await page.evaluate(() => matchMedia('(forced-colors: active)').matches)).toBe(true);
  expect(await page.evaluate(() => matchMedia('(prefers-reduced-motion: reduce)').matches)).toBe(
    true,
  );
  const runIndicator = page.getByRole('region', { name: 'Run status' }).locator('svg');
  await expect(runIndicator).toBeVisible();
  expect(await runIndicator.evaluate((element) => getComputedStyle(element).animationName)).toBe(
    'none',
  );

  for (const surface of [
    { dialog: 'Tasks', trigger: 'Open Tasks' },
    { dialog: 'Agent Context', trigger: 'Open Agent Context' },
  ]) {
    const trigger = page.getByRole('button', { name: surface.trigger });
    await trigger.focus();
    await page.keyboard.press('Enter');
    const dialog = page.getByRole('dialog', { name: surface.dialog });
    await expect(dialog).toBeVisible();
    await expect(dialog).toHaveAttribute('aria-modal', 'true');
    await expect(dialog).toHaveAttribute('data-reduced-motion', 'true');
    if (surface.dialog === 'Tasks') {
      await expect(dialog.getByRole('combobox', { name: 'Workspace' })).toBeVisible();
    } else {
      await expect(dialog.getByText('Current Context')).toBeVisible();
    }
    const close = page.getByRole('button', { name: `Close ${surface.dialog}` });
    await expect(close).toBeFocused();
    const focusable = dialog.locator(
      'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
    );
    const last = focusable.last();
    await expect(last).toBeVisible();
    await last.focus();
    await page.keyboard.press('Tab');
    await expect(close).toBeFocused();
    await page.keyboard.press('Shift+Tab');
    await expect(last).toBeFocused();
    await expectFocusInside(page, dialog);
    await page.keyboard.press('Escape');
    await expect(dialog).toHaveCount(0);
    await expect(trigger).toBeFocused();
    expect(await trigger.evaluate((element) => getComputedStyle(element).outlineStyle)).not.toBe(
      'none',
    );
  }

  await page.goto(`${unifiedBinary.baseUrl}/tasks/${encodeURIComponent(draftId)}`);
  const addSkill = page.getByRole('button', { name: 'Add Skill' });
  await expect(addSkill).toBeEnabled();
  await addSkill.focus();
  await page.keyboard.press('Enter');
  const search = page.getByRole('searchbox', { name: 'Search Skills' });
  await expect(search).toBeVisible();
  await search.focus();
  await page.keyboard.type('review');
  await expect(page.getByRole('listbox', { name: 'Available Skills' })).toBeVisible();
  await addSkill.click();
  await expect(search).toHaveCount(0);
  await expectNoProductOverflow(page);
  await context.close();
});

test('restores selected Context state after a Review file range', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  const taskId = await firstTaskId(request, unifiedBinary);
  const context = await browser.newContext({ viewport: { height: 900, width: 1440 } });
  const page = await openAuthenticatedRoute(
    context,
    unifiedBinary,
    `/tasks/${encodeURIComponent(taskId)}`,
  );
  const agentContext = page.getByRole('complementary', { name: 'Agent Context' });
  await expect(agentContext.getByText('Current Context')).toBeVisible();
  const requestChoice = agentContext.getByRole('button', { name: /^Select Request / }).first();
  await requestChoice.click();
  await expect(agentContext.getByText('Historical Request')).toBeVisible();
  const selectedRequest = new URL(page.url()).searchParams.get('request');
  expect(selectedRequest).not.toBeNull();

  const observations = agentContext.getByRole('button', { name: /Workspace observations/ });
  if ((await observations.getAttribute('aria-expanded')) !== 'true') await observations.click();
  await expect(observations).toHaveAttribute('aria-expanded', 'true');
  await agentContext.getByRole('button', { name: 'Open src/lib.rs' }).click();

  await expect(page.getByRole('region', { name: 'Review', exact: true })).toBeVisible();
  await expect(page.getByRole('tab', { name: 'Files' })).toHaveAttribute('aria-selected', 'true');
  await page.getByRole('spinbutton', { name: 'Start line' }).fill('1');
  await page.getByRole('spinbutton', { name: 'End line' }).fill('3');
  await page.getByRole('button', { name: 'Add annotation' }).click();
  await expect(
    page.getByRole('textbox', { name: 'Comment for src/lib.rs lines 1-3' }),
  ).toBeVisible();

  await page.getByRole('button', { name: 'Leave Review' }).click();
  await expect(page).toHaveURL(
    new RegExp(`/tasks/${taskId}\\?request=${encodeURIComponent(selectedRequest ?? '')}$`),
  );
  const restoredContext = page.getByRole('complementary', { name: 'Agent Context' });
  await expect(restoredContext.getByText('Historical Request')).toBeVisible();
  const restoredObservations = restoredContext.getByRole('button', {
    name: /Workspace observations/,
  });
  await expect(restoredObservations).toHaveAttribute('aria-expanded', 'true');
  await expect(restoredContext.getByRole('button', { name: 'Open src/lib.rs' })).toBeVisible();
  await context.close();
});

test('announces Run, interaction, and completion changes politely without usage chatter', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  const taskId = await firstTaskId(request, unifiedBinary);
  const context = await browser.newContext();
  const page = await openAuthenticatedRoute(
    context,
    unifiedBinary,
    `/tasks/${encodeURIComponent(taskId)}`,
  );
  const live = page.getByRole('status', { name: 'Run status' });
  await expect(live).toHaveAttribute('aria-live', 'polite');
  await expect(live).toContainText(/run started/i);
  await expectNoUsageAnnouncements(page);
  const stopControls = page.getByRole('button', { name: 'Stop run' });
  await expect(stopControls).toHaveCount(2);
  await expect(stopControls.first()).toBeVisible();
  await expect(stopControls.last()).toBeVisible();

  await releaseScenarioBarrier(request, unifiedBinary, 'after-tool');
  const interaction = page.getByRole('group', { name: 'Permission request' });
  await expect(interaction).toBeVisible();
  await expect(live).toContainText(/run needs attention/i);
  await expectNoUsageAnnouncements(page);
  await interaction.getByRole('button').first().click();
  await expect(interaction).toContainText('Resolved');
  await releaseScenarioBarrier(request, unifiedBinary, 'continuity-before-finish');
  await expect(live).toContainText(/run completed/i);
  await expectNoUsageAnnouncements(page);
  await context.close();
});

test('announces a failed Run without exposing usage details', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  const taskId = await firstTaskId(request, unifiedBinary);
  const context = await browser.newContext();
  const page = await openAuthenticatedRoute(
    context,
    unifiedBinary,
    `/tasks/${encodeURIComponent(taskId)}`,
  );
  const live = page.getByRole('status', { name: 'Run status' });
  await expect(live).toHaveAttribute('aria-live', 'polite');
  await failScenarioBarrier(request, unifiedBinary, 'after-tool', 'deterministic failure');
  await expect(live).toContainText(/run failed/i);
  await expectNoUsageAnnouncements(page);
  await context.close();
});

test('stops an active Run through the named keyboard control', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  const taskId = await firstTaskId(request, unifiedBinary);
  const context = await browser.newContext();
  const page = await openAuthenticatedRoute(
    context,
    unifiedBinary,
    `/tasks/${encodeURIComponent(taskId)}`,
  );
  const live = page.getByRole('status', { name: 'Run status' });
  const stopped = page.waitForResponse(
    (response) => response.url().endsWith('/stop') && response.request().method() === 'POST',
  );
  await page.getByRole('banner').getByRole('button', { name: 'Stop run' }).click();
  expect((await stopped).status()).toBe(202);
  await expect(live).toContainText(/run stopped/i);
  await expectNoUsageAnnouncements(page);
  await expect(page.getByRole('button', { name: 'Stop run' })).toHaveCount(0);
  await context.close();
});

test('reflows named controls at an effective 200 percent browser scale', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  const taskId = await draftTaskId(request, unifiedBinary);
  const context = await browser.newContext({
    deviceScaleFactor: 2,
    viewport: { height: 450, width: 640 },
  });
  const page = await openAuthenticatedRoute(
    context,
    unifiedBinary,
    `/tasks/${encodeURIComponent(taskId)}`,
  );
  expect(await page.evaluate(() => devicePixelRatio)).toBe(2);
  const message = page.getByRole('textbox', { name: 'Message' });
  await message.fill('Verify reflow controls');
  for (const control of ['Open Tasks', 'Open Agent Context', 'Add Skill', 'Send']) {
    const button = page.getByRole('button', { name: control });
    await expect(button).toBeVisible();
    await expect(button).toBeEnabled();
    await expectInsideVisualViewport(page, button);
  }
  await expect(message).toBeVisible();
  await expectNoProductOverflow(page);
  await context.close();
});

async function expectInsideVisualViewport(page: Page, locator: Locator): Promise<void> {
  const box = await locator.boundingBox();
  const viewport = page.viewportSize();
  expect(box).not.toBeNull();
  expect(viewport).not.toBeNull();
  expect(box?.x ?? -1).toBeGreaterThanOrEqual(0);
  expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual(viewport?.width ?? 0);
  expect(box?.y ?? -1).toBeGreaterThanOrEqual(0);
  expect((box?.y ?? 0) + (box?.height ?? 0)).toBeLessThanOrEqual(viewport?.height ?? 0);
}
