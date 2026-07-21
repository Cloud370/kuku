import { expect, test } from './fixtures/unifiedBinary';
import { openAuthenticatedRoute } from './fixtures/journey';
import { taskPage } from './fixtures/productApi';

test('opens a durable latest task after a browser reload', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  const durable = await taskPage(request, unifiedBinary, '?limit=1');
  expect(durable.items.length).toBeGreaterThan(0);
  const latest = durable.items[0];
  if (latest === undefined) throw new Error('scenario has no durable task');

  const page = await openAuthenticatedRoute(await browser.newContext(), unifiedBinary, '/');
  const latestTask = page.getByRole('button', { name: latest.title, exact: true });
  await expect(latestTask).toBeVisible();
  await latestTask.click();
  await expect.poll(() => new URL(page.url()).pathname).toBe(`/tasks/${latest.task_id}`);
  await page.reload();
  await expect(page.getByRole('button', { name: latest.title, exact: true })).toHaveAttribute(
    'aria-current',
    'page',
  );
  const expectedStatus = {
    completed: 'Run completed',
    draft: undefined,
    failed: 'Run failed',
    interrupted: 'Run interrupted',
    needs_attention: 'Run started',
    queued: 'Run started',
    running: 'Run started',
    stopped: 'Run stopped',
    stopping: 'Run stopping',
  }[latest.state];
  if (expectedStatus === undefined) throw new Error(`Task has no Run status: ${latest.state}`);
  await expect(page.getByRole('status', { name: 'Run status' })).toContainText(expectedStatus);
});
