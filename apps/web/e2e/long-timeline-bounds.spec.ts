import type { APIRequestContext, Page } from '@playwright/test';

import { expect, test, type UnifiedBinary } from './fixtures/unifiedBinary';
import type { TimelineItemProjection } from '../src/api/generated';
import { firstTaskId, openAuthenticatedRoute } from './fixtures/journey';
import { taskProjection, timelinePage } from './fixtures/productApi';
import { releaseScenarioBarrier, respondToPendingInteraction } from './fixtures/scenarioControl';

test.use({ scenarioName: 'full_task' });

function timelineId(item: TimelineItemProjection): string {
  if (item.type === 'message') return `message:${item.item.message_id}`;
  if (item.type === 'activity') return `activity:${item.item.activity_id}`;
  return `interaction:${item.item.interaction_id}`;
}

async function scrollToHistoryGap(page: Page): Promise<void> {
  const chat = page.locator('[data-chat-scroll]');
  const returnRecent = page.getByRole('button', { name: 'Return to recent messages' });
  for (let attempt = 0; attempt < 100 && (await returnRecent.count()) === 0; attempt += 1) {
    const advanced = await chat.evaluate((element) => {
      const before = element.scrollTop;
      element.scrollTop = Math.min(
        element.scrollHeight - element.clientHeight,
        before + element.clientHeight * 4,
      );
      return element.scrollTop > before;
    });
    await page.evaluate(
      () =>
        new Promise<void>((resolve) => {
          requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
        }),
    );
    if (!advanced) break;
  }
}

async function completeLongScenario(
  request: APIRequestContext,
  server: UnifiedBinary,
): Promise<void> {
  await releaseScenarioBarrier(request, server, 'after-tool');
  await respondToPendingInteraction(request, server);
  await releaseScenarioBarrier(request, server, 'continuity-before-finish');
  await expect
    .poll(async () => (await taskProjection(request, server, server.scenario.taskId)).task.state, {
      timeout: 40_000,
    })
    .toMatch(/completed|stopped|failed|interrupted/);
}

test('keeps long history bounded while opaque pagination continues', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  test.setTimeout(120_000);
  const taskId = await firstTaskId(request, unifiedBinary);
  await completeLongScenario(request, unifiedBinary);
  const ids = new Set<string>();
  const historyIds = new Set<string>();
  const pages: Awaited<ReturnType<typeof timelinePage>>[] = [];
  const cursors = new Set<string>();
  let cursor: string | null = null;
  for (let pageIndex = 0; pageIndex < 25; pageIndex += 1) {
    const query =
      cursor === null ? '?limit=500' : `?limit=500&before=${encodeURIComponent(cursor)}`;
    const history = await timelinePage(request, unifiedBinary, taskId, query);
    pages.push(history);
    expect(history.task_id).toBe(taskId);
    expect(history.items.length).toBeLessThanOrEqual(500);
    const orderKeys = history.items.map((item) => item.item.order_key);
    expect(orderKeys).toEqual([...orderKeys].sort((left, right) => left - right));
    for (const item of history.items) {
      const id = timelineId(item);
      expect(ids.has(id), `duplicate timeline item ${id}`).toBeFalsy();
      ids.add(id);
      if (id.startsWith('activity:scenario-history-7-')) historyIds.add(id);
    }
    cursor = history.next_cursor;
    if (cursor === null) break;
    expect(Number(cursor)).toBeNaN();
    expect(cursors.has(cursor), `repeated opaque cursor ${cursor}`).toBeFalsy();
    cursors.add(cursor);
  }
  expect(cursor).toBeNull();
  expect(historyIds.size).toBe(10_000);
  expect(ids.size).toBeGreaterThan(10_000);
  expect(pages.length).toBeGreaterThanOrEqual(20);

  for (const width of [360, 1440]) {
    const context = await browser.newContext({ viewport: { height: 800, width } });
    const page = await openAuthenticatedRoute(
      context,
      unifiedBinary,
      `/tasks/${encodeURIComponent(taskId)}`,
    );
    const loadOlder = page.getByRole('button', { name: 'Load earlier messages' });
    await expect(loadOlder).toBeVisible();
    for (let pageIndex = 1; pageIndex <= 5; pageIndex += 1) {
      const responsePromise = page.waitForResponse((response) => {
        const url = new URL(response.url());
        return (
          response.request().method() === 'GET' &&
          url.pathname === `/api/v1/tasks/${encodeURIComponent(taskId)}/timeline` &&
          url.searchParams.has('before')
        );
      });
      await loadOlder.click();
      const response = await responsePromise;
      expect(response.ok(), await response.text()).toBeTruthy();
      const before = new URL(response.url()).searchParams.get('before');
      if (before === null) throw new Error('browser omitted its opaque timeline cursor');
      expect(Number(before)).toBeNaN();
      const expectedPage = await timelinePage(
        request,
        unifiedBinary,
        taskId,
        `?limit=500&before=${encodeURIComponent(before)}`,
      );
      const actual = (await response.json()) as typeof expectedPage;
      expect(actual.items.map(timelineId)).toEqual(expectedPage.items.map(timelineId));
      expect(actual.next_cursor).toBe(expectedPage.next_cursor);
      await expect(page.locator('[data-timeline-id]')).not.toHaveCount(0);
      expect(await page.locator('[data-timeline-id]').count()).toBeLessThanOrEqual(120);
      if (pageIndex < 4) {
        await expect(page.getByRole('button', { name: 'Return to recent messages' })).toHaveCount(
          0,
        );
      }
    }

    await scrollToHistoryGap(page);
    const gap = page.getByText('Earlier history is separated from recent messages.');
    await expect(gap).toBeVisible();
    const returnRecent = page.getByRole('button', { name: 'Return to recent messages' });
    await expect(returnRecent).toBeVisible();
    await returnRecent.click();
    await expect(returnRecent).toHaveCount(0);
    await expect(loadOlder).toBeVisible();
    await expect(page.getByRole('status', { name: 'Run status' })).toContainText('Run completed');
    expect(await page.locator('[data-timeline-id]').count()).toBeLessThanOrEqual(120);
    await context.close();
  }
});
