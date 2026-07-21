import type { SettingsSnapshot, UpdateSettingsRequest } from '../src/api/generated';
import { expect, test } from './fixtures/unifiedBinary';
import { openAuthenticatedRoute } from './fixtures/journey';
import { settings } from './fixtures/productApi';

test('keeps the Guide first-Task suggestion editable without auto-sending it', async ({
  browser,
  unifiedBinary,
}) => {
  const context = await browser.newContext();
  const guide = await openAuthenticatedRoute(context, unifiedBinary, '/guide');
  await expect(guide.getByRole('heading', { name: 'Guide' })).toBeVisible();
  await expect(guide.getByText(/first task/i)).toBeVisible();
  await expect(guide.getByRole('textbox')).toHaveCount(0);

  let submittedRuns = 0;
  guide.on('request', (request) => {
    if (request.method() === 'POST' && /\/api\/v1\/tasks\/[^/]+\/runs$/u.test(request.url())) {
      submittedRuns += 1;
    }
  });
  await guide.getByRole('button', { name: 'Prepare first task' }).click();
  await expect(guide).toHaveURL(`${unifiedBinary.baseUrl}/tasks/new`);
  await expect(guide.getByTestId('workbench-shell')).toBeVisible();
  await expect(guide.getByRole('textbox', { name: 'Message' })).toHaveValue(
    'Inspect this workspace and identify the most important next step.',
  );
  expect(submittedRuns).toBe(0);
});

test('resolves Guide deep links inside real Workbench and Settings routes', async ({
  browser,
  unifiedBinary,
}) => {
  const guide = await openAuthenticatedRoute(await browser.newContext(), unifiedBinary, '/guide');
  await guide.goto(`${unifiedBinary.baseUrl}/guide`);
  await guide.getByRole('button', { name: 'Inspect Context' }).click();
  await expect(guide).toHaveURL(`${unifiedBinary.baseUrl}/tasks/new?guide=context`);
  await expect(guide.getByTestId('workbench-shell')).toBeVisible();

  await guide.goto(`${unifiedBinary.baseUrl}/guide`);
  await guide.getByRole('button', { name: 'Open Connection QR' }).click();
  await expect(guide).toHaveURL(`${unifiedBinary.baseUrl}/settings?connection=qr`);
  await expect(guide.getByRole('dialog', { name: 'Connection QR' })).toBeVisible();
  await expect(guide.getByText(unifiedBinary.credential, { exact: false })).toHaveCount(0);
});

test('persists only valid revisioned Settings through public PATCH and readback', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  const expected = await settings(request, unifiedBinary);
  const context = await browser.newContext();
  const page = await openAuthenticatedRoute(context, unifiedBinary, '/settings');
  await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible();
  const maximumRuns = page.getByLabel('Maximum concurrent runs');
  await expect(maximumRuns).toHaveValue(String(expected.max_concurrent_runs));

  await maximumRuns.fill('0');
  await expect(page.getByRole('button', { name: 'Save Settings' })).toBeDisabled();
  await expect(maximumRuns).toHaveValue('0');
  expect((await settings(request, unifiedBinary)).server_revision).toBe(expected.server_revision);

  const nextMaximum = expected.max_concurrent_runs === 64 ? 63 : expected.max_concurrent_runs + 1;
  await maximumRuns.fill(String(nextMaximum));
  const discovery = page.getByRole('checkbox', {
    name: 'Discover Skills and Agents automatically',
  });
  await discovery.setChecked(!expected.discovery.auto_discover);
  const patchRequest = page.waitForRequest(
    (candidate) => candidate.method() === 'PATCH' && candidate.url().endsWith('/api/v1/settings'),
  );
  const patchResponse = page.waitForResponse(
    (candidate) =>
      candidate.request().method() === 'PATCH' && candidate.url().endsWith('/settings'),
  );
  await page.getByRole('button', { name: 'Save Settings' }).click();
  const requestBody = (await patchRequest).postDataJSON() as UpdateSettingsRequest;
  expect(requestBody.expected_revision).toBe(expected.server_revision);
  expect(requestBody.patch.max_concurrent_runs).toBe(nextMaximum);
  expect(requestBody.patch.discovery).toEqual({
    auto_discover: !expected.discovery.auto_discover,
  });
  const response = await patchResponse;
  expect(response.ok(), await response.text()).toBeTruthy();
  const saved = (await response.json()) as SettingsSnapshot;
  expect(saved.max_concurrent_runs).toBe(nextMaximum);
  expect(saved.server_revision).not.toBe(expected.server_revision);
  await expect(page.getByRole('status')).toContainText('Settings saved');

  const readback = await settings(request, unifiedBinary);
  expect(readback.max_concurrent_runs).toBe(nextMaximum);
  expect(readback.discovery.auto_discover).toBe(!expected.discovery.auto_discover);
  expect(readback.server_revision).toBe(saved.server_revision);
  await page.reload();
  await expect(page.getByLabel('Maximum concurrent runs')).toHaveValue(String(nextMaximum));
  const persistedDiscovery = page.getByRole('checkbox', {
    name: 'Discover Skills and Agents automatically',
  });
  if (expected.discovery.auto_discover) await expect(persistedDiscovery).not.toBeChecked();
  else await expect(persistedDiscovery).toBeChecked();
});

test('keeps the Settings connection QR secret-free and returns to Guide', async ({
  browser,
  unifiedBinary,
}) => {
  const page = await openAuthenticatedRoute(await browser.newContext(), unifiedBinary, '/settings');
  await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible();
  await page.getByRole('button', { name: 'Open Connection QR' }).click();
  const qr = page.getByRole('dialog', { name: 'Connection QR' });
  await expect(qr).toBeVisible();
  await expect(qr.getByRole('img', { name: 'Connection QR code' })).toBeVisible();
  await expect(qr.getByText(unifiedBinary.credential, { exact: false })).toHaveCount(0);
  await expect(page.locator('body')).not.toContainText(unifiedBinary.credential);
  await qr.getByRole('button', { name: 'Close Connection QR' }).click();
  await expect(qr).toHaveCount(0);

  await page.getByRole('button', { name: 'Open Guide' }).click();
  await expect(page).toHaveURL(`${unifiedBinary.baseUrl}/guide`);
  await expect(page.getByRole('heading', { name: 'Guide' })).toBeVisible();
});
