import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';

import type {
  AnnotationBatch,
  FileContent,
  ReviewSubmissionResult,
  TaskProjection,
} from '../src/api/generated';
import { expect, test } from './fixtures/unifiedBinary';
import { firstTaskId, openAuthenticatedRoute } from './fixtures/journey';
import { changes, reviewSubmissions, taskProjection } from './fixtures/productApi';
import { completeScenario } from './fixtures/scenarioControl';

const unchangedPath = 'src/lib.rs';

test('preserves typed file return state and one durable idempotent mixed-note Review Run', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  const taskId = await firstTaskId(request, unifiedBinary);
  const projection = await taskProjection(request, unifiedBinary, taskId);
  const workspaceId = projection.task.workspace_id;
  const initialChanges = await changes(request, unifiedBinary, workspaceId);
  expect(initialChanges.availability).toBe('available');
  expect(initialChanges.entries.map((entry) => entry.path)).not.toContain(unchangedPath);
  const unchangedResponse = await request.get(
    `${unifiedBinary.baseUrl}/api/v1/workspaces/${encodeURIComponent(workspaceId)}/files/content?path=${encodeURIComponent(unchangedPath)}&start_line=1&end_line=100`,
    { headers: { Authorization: `Bearer ${unifiedBinary.credential}` } },
  );
  expect(unchangedResponse.ok(), await unchangedResponse.text()).toBeTruthy();
  const unchanged = (await unchangedResponse.json()) as FileContent;
  expect(unchanged.path).toBe(unchangedPath);
  expect(unchanged.text).toContain('pub fn status()');

  const page = await openAuthenticatedRoute(
    await browser.newContext(),
    unifiedBinary,
    `/tasks/${encodeURIComponent(taskId)}`,
  );
  const openFromChat = page
    .getByRole('main', { name: 'Chat' })
    .getByRole('button', { name: `Open ${unchangedPath}` });
  await expect(openFromChat).toBeVisible();
  const chatFileRequest = page.waitForRequest(
    (candidate) =>
      candidate.url().includes(`/workspaces/${encodeURIComponent(workspaceId)}/files/content`) &&
      new URL(candidate.url()).searchParams.get('path') === unchangedPath,
  );
  await openFromChat.click();
  await chatFileRequest;
  await expect(page).toHaveURL(
    new RegExp(
      `/tasks/${encodeURIComponent(taskId)}/review\\?workspace=${encodeURIComponent(workspaceId)}&file=${encodeURIComponent(unchangedPath)}$`,
    ),
  );
  await expect(page.getByText('pub fn status()', { exact: false })).toBeVisible();
  await page.getByRole('button', { name: 'Leave Review' }).click();
  await expect(page).toHaveURL(new RegExp(`/tasks/${encodeURIComponent(taskId)}$`));
  await expect(page.getByRole('main', { name: 'Chat' })).toBeVisible();

  const context = page.getByRole('complementary', { name: 'Agent Context' });
  const observations = context.getByRole('button', { name: /Workspace observations/ });
  if ((await observations.getAttribute('aria-expanded')) !== 'true') await observations.click();
  const openFromContext = context.getByRole('button', { name: `Open ${unchangedPath}` });
  await expect(openFromContext).toBeVisible();
  const contextFileRequest = page.waitForRequest(
    (candidate) =>
      candidate.url().includes(`/workspaces/${encodeURIComponent(workspaceId)}/files/content`) &&
      new URL(candidate.url()).searchParams.get('path') === unchangedPath,
  );
  await openFromContext.click();
  await contextFileRequest;
  await expect(page).toHaveURL(
    new RegExp(
      `/tasks/${encodeURIComponent(taskId)}/review\\?workspace=${encodeURIComponent(workspaceId)}&file=${encodeURIComponent(unchangedPath)}$`,
    ),
  );
  await page.getByRole('button', { name: 'Leave Review' }).click();
  await expect(page).toHaveURL(new RegExp(`/tasks/${encodeURIComponent(taskId)}$`));
  await expect(
    page
      .getByRole('complementary', { name: 'Agent Context' })
      .getByRole('button', { name: /Workspace observations/ }),
  ).toHaveAttribute('aria-expanded', 'true');

  await completeScenario(request, unifiedBinary);
  const before = await reviewSubmissions(request, unifiedBinary, taskId);
  const beforeProjection = await taskProjection(request, unifiedBinary, taskId);
  const reviewUrl = `/tasks/${encodeURIComponent(taskId)}/review`;
  await page.goto(`${unifiedBinary.baseUrl}${reviewUrl}`);
  await expect(page.getByRole('region', { name: 'Review', exact: true })).toBeVisible();
  const changedPath = initialChanges.entries[0]?.path;
  if (changedPath === undefined) throw new Error('scenario returned no changed path');
  await expect(page.getByLabel('Workspace changes')).toContainText(changedPath);
  await page.getByLabel('Workspace changes').getByRole('button').first().click();
  const staleLine = page.getByRole('button', { name: /^Select new line / }).first();
  await staleLine.click();
  await staleLine.click();
  await page.getByRole('textbox', { name: /^Comment for / }).fill('This anchor must go stale.');

  await writeFile(join(unifiedBinary.gitWorkspace, changedPath), 'export const answer = 43;\n');
  const staleResponse = page.waitForResponse(
    (response) => response.url().endsWith('/review/annotations') && response.status() === 409,
  );
  await page.getByRole('button', { name: 'Submit review' }).click();
  const staleError = (await (await staleResponse).json()) as { code: string };
  expect(staleError.code).toBe('outdated');
  await expect(page.getByText('Outdated anchor')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Submit review' })).toBeDisabled();
  expect((await reviewSubmissions(request, unifiedBinary, taskId)).items).toHaveLength(
    before.items.length,
  );
  expect((await taskProjection(request, unifiedBinary, taskId)).task.latest_run_id).toBe(
    beforeProjection.task.latest_run_id,
  );

  await page.goto(`${unifiedBinary.baseUrl}${reviewUrl}`);
  await page.getByLabel('Workspace changes').getByRole('button').first().click();
  const changedRange = page.getByRole('button', { name: /^Select new line / }).first();
  await changedRange.click();
  await changedRange.click();
  await page.getByRole('textbox', { name: /^Comment for / }).fill('Keep the verified change.');

  await page.getByRole('tab', { name: 'Files' }).click();
  await page.getByLabel('Find a file').fill('lib.rs');
  await page.getByRole('button', { name: unchangedPath, exact: true }).click();
  await expect(page.getByText('pub fn status()', { exact: false })).toBeVisible();
  await page.getByLabel('Start line').fill('1');
  await page.getByLabel('End line').fill('2');
  await page.getByRole('button', { name: 'Add annotation' }).click();
  await page
    .getByRole('textbox', { name: new RegExp(`^Comment for ${unchangedPath}`) })
    .fill('Keep this unchanged reference readable.');
  await expect(page.getByLabel('Draft annotations')).toContainText('2');

  let browserSubmissionCount = 0;
  page.on('request', (candidate) => {
    if (
      candidate.method() === 'POST' &&
      candidate.url().endsWith(`/tasks/${encodeURIComponent(taskId)}/review/annotations`)
    ) {
      browserSubmissionCount += 1;
    }
  });
  const submittedRequest = page.waitForRequest(
    (candidate) =>
      candidate.method() === 'POST' &&
      candidate.url().endsWith(`/tasks/${encodeURIComponent(taskId)}/review/annotations`),
  );
  const submittedResponse = page.waitForResponse(
    (response) =>
      response.url().endsWith('/review/annotations') && response.request().method() === 'POST',
  );
  await page.getByRole('button', { name: 'Submit review' }).click();
  const response = await submittedResponse;
  expect(response.status()).toBe(201);
  const submitted = (await response.json()) as ReviewSubmissionResult;
  const submittedBatch = (await submittedRequest).postDataJSON() as AnnotationBatch;
  expect(submitted.replayed).toBe(false);
  expect(browserSubmissionCount).toBe(1);
  expect(submitted.submission.notes).toHaveLength(2);
  expect(new Set(submitted.submission.notes.map((note) => note.side))).toEqual(
    new Set(['new', 'file']),
  );
  const expectedTaskRevision = submittedBatch.expected_task_revision;
  expect(expectedTaskRevision).toBe(beforeProjection.task_revision);
  expect(submitted.submission.task_revision).toBe(expectedTaskRevision + 1);

  await expect
    .poll(async () => {
      try {
        return (await reviewSubmissions(request, unifiedBinary, taskId)).items.length;
      } catch {
        return -1;
      }
    })
    .toBe(before.items.length + 1);
  const after = await taskProjection(request, unifiedBinary, taskId);
  expect(after.review_summary.total_submissions).toBe(
    beforeProjection.review_summary.total_submissions + 1,
  );
  expect(after.task.latest_run_id).toBe(submitted.submission.run_id);

  const replay = await request.post(
    `${unifiedBinary.baseUrl}/api/v1/tasks/${encodeURIComponent(taskId)}/review/annotations`,
    {
      data: submittedBatch,
      headers: { Authorization: `Bearer ${unifiedBinary.credential}` },
    },
  );
  expect(replay.status(), await replay.text()).toBe(201);
  const replayed = (await replay.json()) as ReviewSubmissionResult;
  expect(replayed.replayed).toBe(true);
  expect(replayed.submission.submission_id).toBe(submitted.submission.submission_id);
  expect(replayed.submission.run_id).toBe(submitted.submission.run_id);
  const afterReplay = await reviewSubmissions(request, unifiedBinary, taskId);
  expect(afterReplay.items).toHaveLength(before.items.length + 1);
  const afterReplayProjection: TaskProjection = await taskProjection(
    request,
    unifiedBinary,
    taskId,
  );
  expect(afterReplayProjection.task.latest_run_id).toBe(submitted.submission.run_id);

  await page.reload();
  await expect(page.getByLabel('Submitted reviews')).toContainText('Keep the verified change.');
  await expect(page.getByLabel('Submitted reviews')).toContainText(
    'Keep this unchanged reference readable.',
  );
  await expect(page.getByLabel('Submitted reviews')).toContainText(submitted.submission.run_id);
});
