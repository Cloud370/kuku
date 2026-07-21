import type { PlatformStatus, RegistrationRootPage, TaskProjection } from '../src/api/generated';
import { expect, startFreshUnifiedBinary, test } from './fixtures/unifiedBinary';

test('authenticates with a fragment credential and opens the configured first task flow', async ({
  browser,
  request,
  workspaces,
}, testInfo) => {
  const server = await startFreshUnifiedBinary(workspaces, testInfo.workerIndex);
  const headers = { Authorization: `Bearer ${server.credential}` };
  const context = await browser.newContext();
  try {
    const page = await context.newPage();
    await page.goto(`${server.baseUrl}/#credential=${encodeURIComponent(server.credential)}`);
    await expect(page).toHaveURL(/^(?!.*credential=)/u);
    await expect(page.getByRole('heading', { name: 'Initialize kuku' })).toBeVisible();
    await expect(page.getByText(server.credential, { exact: false })).toHaveCount(0);

    await page.getByLabel('Provider ID').fill('e2e-init-provider');
    await page.getByLabel('API format').fill('anthropic');
    await page.getByLabel('Base URL').fill(server.providerOrigin);
    await page.getByLabel('Provider credential').fill('e2e-init-provider-key');
    await page.getByLabel('Tier ID').fill('e2e-init-balanced');
    await page.getByLabel('Model').fill('deterministic-fixture');
    await page.getByRole('button', { name: 'Save providers' }).click();

    await expect(page.getByLabel('Default Tier ID')).toBeVisible();
    await page.getByLabel('Default Tier ID').fill('e2e-init-balanced');
    await page.getByRole('button', { name: 'Save default Tier' }).click();

    const rootsResponse = await request.get(`${server.baseUrl}/api/v1/registration-roots`, {
      headers,
    });
    expect(rootsResponse.ok(), await rootsResponse.text()).toBeTruthy();
    const roots = (await rootsResponse.json()) as RegistrationRootPage;
    const root = roots.items[0];
    if (root === undefined) throw new Error('fresh server has no registration root');
    await expect(page.getByLabel('Workspace label')).toBeVisible();
    await page.getByLabel('Workspace label').fill('Git fixture');
    await page.getByLabel('Registration root ID').fill(root.root_id);
    await page.getByLabel('Workspace path').fill('git-workspace');
    await page.getByRole('button', { name: 'Register workspace' }).click();

    await expect(page.getByLabel('Test Tier ID')).toBeVisible();
    await page.getByLabel('Test Tier ID').fill('e2e-init-balanced');
    await page.getByRole('button', { name: 'Test provider' }).click();
    await expect(page.getByText('Provider and workspace checks passed.')).toBeVisible();
    let initialTaskLists = 0;
    page.on('response', (response) => {
      const url = new URL(response.url());
      if (response.request().method() === 'GET' && url.pathname === '/api/v1/tasks') {
        initialTaskLists += 1;
      }
    });
    await page.getByRole('button', { name: 'Complete setup' }).click();
    await expect(page.getByLabel('Task navigation')).toBeVisible();
    await expect(page.getByText('No Tasks yet')).toBeVisible();
    await expect.poll(() => initialTaskLists).toBeGreaterThanOrEqual(2);

    await page.getByRole('button', { name: 'New Task' }).click();
    const createResponse = page.waitForResponse(
      (response) => response.url().endsWith('/api/v1/tasks') && response.status() === 201,
    );
    await page.getByRole('button', { name: 'Create Task' }).click();
    const created = (await (await createResponse).json()) as { projection: TaskProjection };
    const taskId = created.projection.task.task_id;
    await expect(page).toHaveURL(new RegExp(`/tasks/${encodeURIComponent(taskId)}$`, 'u'));
    await page.getByRole('textbox', { name: 'Message' }).fill('Run the first configured Task');
    await page.getByRole('button', { name: 'Choose Tier' }).click();
    await page.getByRole('option', { name: 'e2e-init-balanced' }).click();
    const submitResponse = page.waitForResponse((response) =>
      response.url().endsWith(`/tasks/${taskId}/runs`),
    );
    await page.getByRole('button', { name: 'Send' }).click();
    const submitted = await submitResponse;
    expect(submitted.status(), await submitted.text()).toBe(202);

    const statusResponse = await request.get(`${server.baseUrl}/api/v1/status`, { headers });
    expect(statusResponse.ok(), await statusResponse.text()).toBeTruthy();
    const configured = (await statusResponse.json()) as PlatformStatus;
    expect(configured.auth.authenticated).toBeTruthy();
    expect(configured.init.phase).toBe('complete');
    expect(configured.init.provider_test_passed).toBeTruthy();
    expect(configured.ready).toBeTruthy();
    const escaped = await request.post(`${server.baseUrl}/api/v1/workspaces`, {
      data: {
        expected_revision: configured.init.server_revision,
        label: 'Escaped workspace',
        relative_path: '../outside-registration-root',
        root_id: root.root_id,
      },
      headers,
    });
    expect(escaped.status()).toBe(400);
    await expect
      .poll(async () => {
        const response = await request.get(
          `${server.baseUrl}/api/v1/tasks/${encodeURIComponent(taskId)}`,
          { headers },
        );
        if (!response.ok()) return null;
        return ((await response.json()) as TaskProjection).latest_run?.state ?? null;
      })
      .toBe('completed');
    expect(taskId).not.toContain(workspaces.registrationRoot);
    await expect(page.locator('body')).not.toContainText(server.credential);
    await expect(page.locator('body')).not.toContainText(workspaces.registrationRoot);
  } finally {
    await context.close();
    await server.stop();
  }
});
