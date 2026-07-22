import { expect, test } from './fixtures/unifiedBinary';
import { firstTaskId, openAuthenticatedRoute } from './fixtures/journey';
import { authenticatedGet, contextSnapshot } from './fixtures/productApi';
import { completeScenario } from './fixtures/scenarioControl';

test('opens an exact immutable historical request from typed Context history', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  const taskId = await firstTaskId(request, unifiedBinary);
  await completeScenario(request, unifiedBinary);
  const current = await contextSnapshot(request, unifiedBinary, taskId);
  const historicalRequest = current.request_history[0];
  expect(historicalRequest).toBeDefined();
  if (historicalRequest === undefined) throw new Error('scenario has no historical Request');
  const historicalResponse = await authenticatedGet(
    request,
    unifiedBinary,
    `/tasks/${encodeURIComponent(taskId)}/context/${encodeURIComponent(historicalRequest.request_id)}`,
  );
  expect(historicalResponse.ok(), await historicalResponse.text()).toBeTruthy();
  const historical = (await historicalResponse.json()) as typeof current;
  expect(historical.selected_request?.request_id).toBe(historicalRequest.request_id);
  expect(historical.exact_request).toEqual(current.exact_request);
  expect(historical.exact_payload_hash).toBe(current.exact_payload_hash);
  expect(historical.exact_request?.messages[0]?.content[0]).toMatchObject({
    kind: 'text',
    text: 'Exercise the full deterministic browser scenario',
  });
  const page = await openAuthenticatedRoute(
    await browser.newContext(),
    unifiedBinary,
    `/tasks/${encodeURIComponent(taskId)}`,
  );
  await expect(page.getByRole('complementary', { name: 'Agent Context' })).toBeVisible();
  await page
    .getByRole('button', { name: `Select Request ${historicalRequest.request_id}` })
    .click();
  await expect(page).toHaveURL(
    new RegExp(`request=${encodeURIComponent(historicalRequest.request_id)}`),
  );
  await expect(page.getByText('Historical Request', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'View exact Request' }).click();
  const dialog = page.getByRole('dialog', { name: 'Exact Request' });
  await expect(dialog).toBeVisible();
  const dialogBounds = await dialog.boundingBox();
  expect(dialogBounds).not.toBeNull();
  expect(dialogBounds?.width ?? 0).toBeGreaterThanOrEqual(900);
  expect(dialogBounds?.height ?? 0).toBeGreaterThanOrEqual(600);
  await expect(dialog.getByRole('navigation', { name: 'Message navigator' })).toBeVisible();
  await expect(dialog.getByLabel('Request usage')).toContainText('Cache hit');
  await expect(dialog.getByLabel('Request usage')).toContainText('Input tokens');
  await expect(dialog).toContainText('Exercise the full deterministic browser scenario');
  await expect(dialog).toContainText(historical.exact_payload_hash ?? 'missing-payload-hash');
  await expect(
    page.getByRole('complementary', { name: 'Agent Context' }).getByRole('textbox'),
  ).toHaveCount(0);
  await expect(dialog.getByRole('textbox')).toHaveCount(0);
});
