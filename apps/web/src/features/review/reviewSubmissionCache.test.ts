import { describe, expect, it } from 'vitest';
import type { ReviewSubmissionPage, TaskChange } from '@/api/generated';

import { mergeSubmissionPage, reviewSubmissionsChanged } from './reviewSubmissionCache';

const taskId = 'tsk_000000000000000000000001';

function page(): ReviewSubmissionPage {
  return {
    api_version: 1,
    items: [
      {
        notes: [],
        run_id: 'run_000000000000000000000001',
        submission_id: 'rsub_000000000000000000000001',
        submitted_at: '2026-07-21T00:00:00Z',
        task_id: taskId,
        task_revision: 8,
      },
    ],
    next_cursor: null,
    task_id: taskId,
  };
}

describe('review submission cache helpers', () => {
  it('merges only server projections and deduplicates submission IDs', () => {
    const first = mergeSubmissionPage([], page());
    const second = mergeSubmissionPage(first, page());
    expect(second).toHaveLength(1);
    expect(second[0]?.submission_id).toBe('rsub_000000000000000000000001');
  });

  it('recognizes the typed review submission change', () => {
    const submission = page().items[0];
    if (submission === undefined) throw new Error('fixture submission missing');
    const change: TaskChange = {
      change: { submission, total_submissions: 1 },
      type: 'review_submissions_changed',
    };
    expect(reviewSubmissionsChanged([change])).toBe(true);
    expect(reviewSubmissionsChanged([])).toBe(false);
  });
});
