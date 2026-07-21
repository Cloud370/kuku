import type { Locator, Page } from '@playwright/test';

import { expect, test } from './fixtures/unifiedBinary';
import { expectNoProductOverflow, openAuthenticatedRoute } from './fixtures/journey';
import { taskPage } from './fixtures/productApi';

async function expectBox(
  locator: Locator,
  expected: { height: number; width?: number },
): Promise<void> {
  const box = await locator.boundingBox();
  expect(box).not.toBeNull();
  expect(box?.height).toBeCloseTo(expected.height, 0);
  if (expected.width !== undefined) expect(box?.width).toBeCloseTo(expected.width, 0);
}

async function expectInsideViewport(page: Page, locator: Locator): Promise<void> {
  const box = await locator.boundingBox();
  expect(box).not.toBeNull();
  const viewport = page.viewportSize();
  expect(viewport).not.toBeNull();
  expect(box?.x ?? -1).toBeGreaterThanOrEqual(0);
  expect(box?.y ?? -1).toBeGreaterThanOrEqual(0);
  expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual(viewport?.width ?? 0);
  expect((box?.y ?? 0) + (box?.height ?? 0)).toBeLessThanOrEqual(viewport?.height ?? 0);
}

for (const width of [360, 768, 1440]) {
  test(`keeps Workbench, Composer, and Review usable at ${String(width)}px`, async ({
    browser,
    request,
    unifiedBinary,
  }) => {
    const tasks = await taskPage(request, unifiedBinary);
    const task = tasks.items.find((candidate) => candidate.state === 'draft');
    if (task === undefined) throw new Error('scenario must preload a durable Draft Task');
    const taskId = task.task_id;
    const context = await browser.newContext({ viewport: { height: 900, width } });
    const page = await openAuthenticatedRoute(
      context,
      unifiedBinary,
      `/tasks/${encodeURIComponent(taskId)}`,
    );

    const shell = page.getByTestId('workbench-shell');
    await expect(shell).toBeVisible();
    await expect(page.getByLabel('Chat')).toBeVisible();
    await expect(page.getByLabel('Composer')).toBeVisible();
    const rem = await page.evaluate(() =>
      Number.parseFloat(getComputedStyle(document.documentElement).fontSize),
    );
    await expectBox(page.getByRole('banner'), { height: rem * 3.5, width });
    await expectBox(page.getByRole('button', { name: 'Add Skill' }), {
      height: rem * 2,
      width: rem * 2,
    });
    await expectBox(page.getByRole('button', { name: 'Send' }), {
      height: rem * 2.25,
      width: rem * 2.25,
    });

    if (width < 768) {
      await expectBox(page.getByRole('button', { name: 'Open Tasks' }), {
        height: rem * 2.25,
        width: rem * 2.25,
      });
      await expectBox(page.getByRole('button', { name: 'Open Agent Context' }), {
        height: rem * 2.25,
        width: rem * 2.25,
      });
    } else {
      await expect(page.getByRole('navigation', { name: 'Tasks' })).toBeVisible();
      await expect(page.getByRole('complementary', { name: 'Agent Context' })).toBeVisible();
      await expectBox(page.getByRole('button', { name: 'Collapse Tasks' }), {
        height: rem * 2.25,
        width: rem * 2.25,
      });
      await expectBox(page.getByRole('button', { name: 'Collapse Agent Context' }), {
        height: rem * 2.25,
        width: rem * 2.25,
      });
    }

    await page.getByRole('button', { name: 'Add Skill' }).click();
    const picker = page.getByRole('listbox', { name: 'Available Skills' });
    await expect(picker).toBeVisible();
    await expectInsideViewport(page, page.getByRole('searchbox', { name: 'Search Skills' }));
    await expectNoProductOverflow(page);
    await page.getByRole('button', { name: 'Add Skill' }).click();

    const runStatus = (await page.getByRole('region', { name: 'Run status' }).innerText()).trim();
    await page.goto(`${unifiedBinary.baseUrl}/tasks/${encodeURIComponent(taskId)}/review`);
    const review = page.getByRole('region', { name: 'Review', exact: true });
    await expect(review).toBeVisible();
    await expectBox(review.locator('header').first(), { height: 52 });
    await page.getByLabel('Workspace changes').getByRole('button').first().click();
    const newLines = page.getByRole('button', { name: /^Select new line / });
    await expect(newLines.first()).toBeVisible();
    await newLines.first().click();
    await newLines.first().click();
    await expect(page.getByRole('textbox', { name: /^Comment for .* line \d+$/ })).toBeVisible();
    await expect(page.getByLabel('Review notes')).toContainText('1');
    await expectNoProductOverflow(page);

    await page.getByRole('button', { name: 'Leave Review' }).click();
    await expect(page).toHaveURL(new RegExp(`/tasks/${taskId}$`));
    await expect(page.getByRole('region', { name: 'Run status' })).toContainText(runStatus);
    await expectNoProductOverflow(page);
    await context.close();
  });
}
