import type { ReviewSubmissionPage, ReviewSubmissionProjection, TaskChange } from '@/api/generated';

export function mergeSubmissionPage(
  current: readonly ReviewSubmissionProjection[],
  page: ReviewSubmissionPage,
): ReviewSubmissionProjection[] {
  const byId = new Map(current.map((submission) => [submission.submission_id, submission]));
  for (const submission of page.items) {
    if (submission.task_id === page.task_id) byId.set(submission.submission_id, submission);
  }
  return [...byId.values()].sort((left, right) =>
    right.submitted_at.localeCompare(left.submitted_at),
  );
}

export function reviewSubmissionsChanged(changes: readonly TaskChange[]): boolean {
  return changes.some((change) => change.type === 'review_submissions_changed');
}
