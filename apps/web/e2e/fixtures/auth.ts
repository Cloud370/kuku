import type { BrowserContext, Page } from '@playwright/test';

import type { UnifiedBinary } from './unifiedBinary';

export async function authenticatedPage(
  context: BrowserContext,
  server: UnifiedBinary,
): Promise<Page> {
  const page = await context.newPage();
  await page.goto(`${server.baseUrl}/#credential=${encodeURIComponent(server.credential)}`);
  await page.waitForURL((url) => url.hash.length === 0);
  return page;
}
