import { expect, test } from './fixtures/unifiedBinary';
import { firstTaskId, openAuthenticatedRoute } from './fixtures/journey';
import { contextSnapshot, taskProjection, timelinePage } from './fixtures/productApi';
import { releaseScenarioBarrier } from './fixtures/scenarioControl';

async function durableInteractionReceipt(
  request: Parameters<typeof timelinePage>[0],
  unifiedBinary: Parameters<typeof timelinePage>[1],
  taskId: string,
) {
  let before: string | null = null;
  for (let pageIndex = 0; pageIndex < 256; pageIndex += 1) {
    const timeline = await timelinePage(
      request,
      unifiedBinary,
      taskId,
      before === null ? '?limit=500' : `?limit=500&before=${encodeURIComponent(before)}`,
    );
    const receipt = timeline.items.find(({ type }) => type === 'interaction');
    if (receipt?.type === 'interaction') {
      return { choice: receipt.item.selected_choice_id, status: receipt.item.status };
    }
    before = timeline.next_cursor;
    if (before === null) return null;
  }
  throw new Error('interaction receipt exceeded the bounded scenario history');
}

test('shows ordered activity and a server-owned interaction without inventing a completion', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  const taskId = await firstTaskId(request, unifiedBinary);
  const page = await openAuthenticatedRoute(
    await browser.newContext(),
    unifiedBinary,
    `/tasks/${encodeURIComponent(taskId)}`,
  );
  await releaseScenarioBarrier(request, unifiedBinary, 'after-tool');
  const interaction = page.getByRole('group', { name: 'Permission request' });
  await expect(interaction).toBeVisible();
  const waiting = await taskProjection(request, unifiedBinary, taskId);
  const toolIndex = waiting.timeline.findIndex(
    ({ type, item }) => type === 'activity' && 'kind' in item && item.kind === 'tool',
  );
  const interactionIndex = waiting.timeline.findIndex(({ type }) => type === 'interaction');
  expect(toolIndex).toBeGreaterThanOrEqual(0);
  expect(interactionIndex).toBeGreaterThan(toolIndex);
  expect(waiting.task.state).toBe('needs_attention');
  await expect(page.getByText('read_file', { exact: true })).toBeVisible();

  const choice = interaction.getByRole('button', { name: 'Approve' });
  await expect(choice).toBeEnabled();
  await choice.click();
  await expect(interaction).toContainText('Resolved');
  await expect
    .poll(async () => {
      const current = await taskProjection(request, unifiedBinary, taskId);
      return current.timeline.some(
        ({ type, item }) =>
          type === 'activity' && 'kind' in item && item.kind === 'delegated_agent',
      );
    })
    .toBeTruthy();
  expect(await durableInteractionReceipt(request, unifiedBinary, taskId)).toEqual({
    choice: 'approve',
    status: 'resolved',
  });
  await expect(page.getByText('Delegated Agent', { exact: true })).toBeVisible();
  const context = await contextSnapshot(request, unifiedBinary, taskId);
  const delegated = context.sections.agents[0];
  if (delegated === undefined) throw new Error('scenario exposed no delegated Context fact');
  const threadResponse = await request.get(
    `${unifiedBinary.baseUrl}/api/v1/tasks/${encodeURIComponent(taskId)}/agents/${encodeURIComponent(delegated.conversation_id)}`,
    { headers: { Authorization: `Bearer ${unifiedBinary.credential}` } },
  );
  expect(threadResponse.ok(), await threadResponse.text()).toBeTruthy();
  const thread = (await threadResponse.json()) as {
    conversation_id: string;
    result_in_main: boolean;
  };
  expect(thread.conversation_id).toBe(delegated.conversation_id);
  expect(thread.result_in_main).toBeTruthy();

  await page.reload();
  const agents = page.getByRole('button', { name: /^Agents 1$/ });
  if ((await agents.getAttribute('aria-expanded')) !== 'true') await agents.click();
  await page.getByRole('button', { name: 'Open scenario-agent' }).click();
  const agentThread = page.getByRole('dialog', { name: 'Agent thread' });
  await expect(agentThread).toBeVisible();
  await expect(agentThread).toContainText(/completed/i);
  await expect(agentThread.getByRole('textbox')).toHaveCount(0);
  await page.getByRole('button', { name: 'Close Agent thread' }).click();

  await expect(page.getByLabel('Composer')).toBeVisible();
  const stop = page.getByLabel('Composer').getByRole('button', { name: /stop run/i });
  await expect(stop).toBeEnabled();
  await stop.click();
  await expect
    .poll(async () => (await taskProjection(request, unifiedBinary, taskId)).task.state)
    .toMatch(/stopped|interrupted/);
  await expect(page.getByRole('status', { name: 'Run status' })).toContainText(/run stopped/i);
  await expect(page.getByLabel('Composer').getByRole('textbox')).toBeEnabled();
});
