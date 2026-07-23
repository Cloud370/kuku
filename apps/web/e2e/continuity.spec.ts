import { expect, test } from './fixtures/unifiedBinary';
import { authenticatedPage } from './fixtures/auth';
import type {
  CreateTaskResponse,
  InteractionResponseRequest,
  SubmitRunResponse,
} from '../src/api/generated';
import { taskProjection, workspacePage } from './fixtures/productApi';
import {
  releaseScenarioBarrier,
  SCENARIO_SETTLE_TIMEOUT_MS,
} from './fixtures/scenarioControl';

test('an independent phone follows the same committed task after desktop closes', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  const headers = { Authorization: `Bearer ${unifiedBinary.credential}` };
  const workspaces = await workspacePage(request, unifiedBinary);
  const workspace = workspaces.items.find(({ label }) => label === 'Git fixture');
  if (workspace === undefined) throw new Error('scenario has no Git fixture workspace');
  const createResponse = await request.post(`${unifiedBinary.baseUrl}/api/v1/tasks`, {
    data: {
      idempotency_key: 'continuity-create-task',
      workspace_id: workspace.workspace_id,
    },
    headers,
  });
  expect(createResponse.ok(), await createResponse.text()).toBeTruthy();
  const created = (await createResponse.json()) as CreateTaskResponse;
  const taskId = created.projection.task.task_id;
  const desktop = await browser.newContext({ viewport: { height: 900, width: 1440 } });
  const phone = await browser.newContext({ viewport: { height: 800, width: 360 } });
  const desktopPage = await authenticatedPage(desktop, unifiedBinary);
  await desktopPage.goto(`${unifiedBinary.baseUrl}/tasks/${encodeURIComponent(taskId)}`);
  const unsent = 'desktop-only unsent draft';
  await desktopPage.getByRole('textbox', { name: 'Message', exact: true }).fill(unsent);
  const submitResponse = await request.post(
    `${unifiedBinary.baseUrl}/api/v1/tasks/${encodeURIComponent(taskId)}/runs`,
    {
      data: {
        expected_task_revision: created.projection.task_revision,
        idempotency_key: 'continuity-start-run',
        message: 'Continue on the independent phone',
        skill_ids: [],
        tier_id: 'tier:e2e-balanced',
      },
      headers,
    },
  );
  expect(submitResponse.ok(), await submitResponse.text()).toBeTruthy();
  const accepted = (await submitResponse.json()) as SubmitRunResponse;
  await releaseScenarioBarrier(request, unifiedBinary, 'after-tool');
  await expect(desktopPage.getByRole('status', { name: 'Run status' })).toContainText(
    /run needs attention/i,
  );
  await expect
    .poll(async () => {
      const current = await taskProjection(request, unifiedBinary, taskId);
      return current.timeline.find(
        ({ type, item }) => type === 'interaction' && 'status' in item && item.status === 'pending',
      )?.item;
    })
    .toBeDefined();
  const needsAttention = await taskProjection(request, unifiedBinary, taskId);
  const pending = needsAttention.timeline.find(
    ({ type, item }) => type === 'interaction' && 'status' in item && item.status === 'pending',
  );
  if (pending?.type !== 'interaction') throw new Error('continuity Run has no interaction');
  const choice = pending.item.choices[0];
  if (choice === undefined) throw new Error('continuity interaction has no choice');
  const interactionResponse = await request.post(
    `${unifiedBinary.baseUrl}/api/v1/tasks/${encodeURIComponent(taskId)}/interactions/${encodeURIComponent(pending.item.interaction_id)}`,
    {
      data: {
        choice_id: choice.choice_id,
        expected_task_revision: needsAttention.task_revision,
        idempotency_key: 'continuity-resolve-interaction',
      } satisfies InteractionResponseRequest,
      headers,
    },
  );
  expect(interactionResponse.ok(), await interactionResponse.text()).toBeTruthy();
  await expect
    .poll(async () => (await taskProjection(request, unifiedBinary, taskId)).task.state)
    .toBe('running');
  const active = await taskProjection(request, unifiedBinary, taskId);
  expect(active.active_run?.run_id).toBe(accepted.run_id);
  await desktop.close();

  const phonePage = await authenticatedPage(phone, unifiedBinary);
  await phonePage.goto(`${unifiedBinary.baseUrl}/tasks/${encodeURIComponent(taskId)}`);
  await expect(phonePage.getByRole('main')).toBeVisible();
  await expect(phonePage.getByRole('textbox', { name: 'Message', exact: true })).toHaveValue('');
  await expect(phonePage.locator('body')).not.toContainText(unsent);
  await expect(phonePage.getByRole('dialog', { name: 'Agent Context' })).toHaveCount(0);
  const followed = await taskProjection(request, unifiedBinary, taskId);
  expect(followed.task.task_id).toBe(active.task.task_id);
  expect(followed.active_run?.run_id).toBe(active.active_run?.run_id);
  await releaseScenarioBarrier(request, unifiedBinary, 'continuity-before-finish');
  await expect(phonePage.getByRole('status', { name: 'Run status' })).toContainText(
    /run completed/i,
    { timeout: SCENARIO_SETTLE_TIMEOUT_MS },
  );
  const terminal = await taskProjection(request, unifiedBinary, taskId);
  expect(terminal.latest_run?.run_id).toBe(accepted.run_id);
  expect(terminal.task.state).toBe('completed');
});
