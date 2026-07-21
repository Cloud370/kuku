import { expect, type APIRequestContext, type BrowserContext, type Page } from '@playwright/test';

import { authenticatedPage } from './auth';
import { taskPage } from './productApi';
import type { UnifiedBinary } from './unifiedBinary';

export async function openAuthenticatedRoute(
  context: BrowserContext,
  server: UnifiedBinary,
  path = '/',
): Promise<Page> {
  const page = await authenticatedPage(context, server);
  await page.goto(`${server.baseUrl}${path}`);
  return page;
}

export async function firstTaskId(
  request: APIRequestContext,
  server: UnifiedBinary,
): Promise<string> {
  const page = await taskPage(request, server, '?limit=1');
  expect(page.items.length, 'scenario must preload a durable task').toBeGreaterThan(0);
  const task = page.items[0];
  if (task === undefined) throw new Error('scenario returned no task');
  return task.task_id;
}

export async function expectNoProductOverflow(page: Page): Promise<void> {
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth > window.innerWidth,
  );
  expect(overflow).toBeFalsy();
}
