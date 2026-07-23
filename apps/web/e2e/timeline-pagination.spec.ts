import type { APIRequestContext } from '@playwright/test';

import type {
  SubmitRunRequest,
  SubmitRunResponse,
  TimelineItemProjection,
  TimelinePage,
} from '../src/api/generated';
import { expect, test, type UnifiedBinary } from './fixtures/unifiedBinary';
import { firstTaskId, openAuthenticatedRoute } from './fixtures/journey';
import { taskProjection, timelinePage } from './fixtures/productApi';
import {
  releaseScenarioBarrier,
  respondToPendingInteraction,
  SCENARIO_SETTLE_TIMEOUT_MS,
} from './fixtures/scenarioControl';

test.use({ scenarioName: 'full_task' });

function timelineId(item: TimelineItemProjection): string {
  if (item.type === 'message') return `message:${item.item.message_id}`;
  if (item.type === 'activity') return `activity:${item.item.activity_id}`;
  return `interaction:${item.item.interaction_id}`;
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

async function appendLiveMessage(
  request: APIRequestContext,
  server: UnifiedBinary,
  taskId: string,
): Promise<SubmitRunResponse> {
  const projection = await taskProjection(request, server, taskId);
  const body = {
    expected_task_revision: projection.task_revision,
    idempotency_key: 'e2e-concurrent-timeline-append',
    message: 'Concurrent live append during history pagination',
    skill_ids: [],
    tier_id: projection.selected_tier_id,
  } satisfies SubmitRunRequest;
  const response = await request.post(
    `${server.baseUrl}/api/v1/tasks/${encodeURIComponent(taskId)}/runs`,
    {
      data: body,
      headers: { Authorization: `Bearer ${server.credential}` },
    },
  );
  expect(response.ok(), await response.text()).toBeTruthy();
  return response.json() as Promise<SubmitRunResponse>;
}

test('prepends authoritative history without duplicating the newest timeline window', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  test.setTimeout(90_000);
  const taskId = await firstTaskId(request, unifiedBinary);
  await completeLongScenario(request, unifiedBinary);
  const newest = await timelinePage(request, unifiedBinary, taskId, '?limit=500');
  expect(newest.items).toHaveLength(500);
  expect(newest.next_cursor).not.toBeNull();
  const newestIds = new Set(newest.items.map(timelineId));
  const page = await openAuthenticatedRoute(
    await browser.newContext(),
    unifiedBinary,
    `/tasks/${encodeURIComponent(taskId)}`,
  );
  const timeline = page.getByTestId('virtual-timeline');
  await expect(timeline).toBeVisible();
  const anchor = timeline.locator('[data-timeline-id]').first();
  await expect(anchor).toBeVisible();
  const anchorId = await anchor.getAttribute('data-timeline-id');
  const anchorBefore = await anchor.boundingBox();
  if (anchorId === null || anchorBefore === null) throw new Error('timeline exposed no anchor row');
  const loadOlder = page.getByRole('button', { name: 'Load earlier messages' });
  const historyResponse = page.waitForResponse((response) => {
    const url = new URL(response.url());
    return (
      response.request().method() === 'GET' &&
      url.pathname === `/api/v1/tasks/${encodeURIComponent(taskId)}/timeline`
    );
  });
  const [, appended] = await Promise.all([
    loadOlder.click(),
    appendLiveMessage(request, unifiedBinary, taskId),
  ]);
  const response = await historyResponse;
  expect(response.ok(), await response.text()).toBeTruthy();
  const requestedCursor = new URL(response.url()).searchParams.get('before');
  expect(requestedCursor).toBe(newest.next_cursor);
  const authoritative = (await response.json()) as TimelinePage;
  const authoritativeIds = authoritative.items.map(timelineId);
  expect(new Set(authoritativeIds).size).toBe(authoritativeIds.length);
  for (const id of authoritativeIds) expect(newestIds.has(id), `overlap ${id}`).toBeFalsy();

  await expect
    .poll(
      async () => {
        const projection = await taskProjection(request, unifiedBinary, appended.task_id);
        const item = projection.timeline.find(
          (candidate) =>
            candidate.type === 'message' &&
            candidate.item.text === 'Concurrent live append during history pagination',
        );
        return item === undefined ? null : { item, projection };
      },
      { timeout: SCENARIO_SETTLE_TIMEOUT_MS },
    )
    .not.toBeNull();
  const preserved = timeline.locator(`[data-timeline-id=${JSON.stringify(anchorId)}]`);
  await expect(preserved).toBeVisible();
  await expect
    .poll(async () => {
      const after = await preserved.boundingBox();
      return after === null ? Number.POSITIVE_INFINITY : Math.abs(after.y - anchorBefore.y);
    })
    .toBeLessThanOrEqual(2);

  await page.locator('[data-chat-scroll]').evaluate((chat) => {
    chat.scrollTop = chat.scrollHeight;
  });
  await expect(page.getByText('Concurrent live append during history pagination')).toBeVisible();
  await expect(page.getByText('Concurrent live append during history pagination')).toHaveCount(1);

  const mountedIds = await timeline
    .locator('[data-timeline-id]')
    .evaluateAll((rows) => rows.map((row) => row.getAttribute('data-timeline-id')));
  expect(new Set(mountedIds).size).toBe(mountedIds.length);
  expect(mountedIds.length).toBeLessThanOrEqual(120);
});
