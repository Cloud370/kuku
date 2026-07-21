import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ReviewSubmissionPage } from '@/api/generated';

import { SubmittedReviewNotes, type ReviewSubmissionSource } from './SubmittedReviewNotes';

const taskId = 'tsk_000000000000000000000001';

function page(comment = 'Please rename this'): ReviewSubmissionPage {
  return {
    api_version: 1,
    items: [
      {
        notes: [
          {
            comment,
            end_line: 4,
            excerpt: 'old_name',
            path: 'src/lib.rs',
            revision: 'a'.repeat(64),
            side: 'old',
            start_line: 4,
            status: 'current',
          },
        ],
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

afterEach(cleanup);

describe('SubmittedReviewNotes', () => {
  it('loads server-backed submissions and renders the follow-up Run', async () => {
    const source: ReviewSubmissionSource = { submissions: vi.fn().mockResolvedValue(page()) };
    render(<SubmittedReviewNotes refreshKey={0} source={source} taskId={taskId} />);

    expect(screen.getByRole('status')).toHaveTextContent('Loading submitted reviews');
    expect(await screen.findByText('Please rename this')).toBeVisible();
    expect(screen.getByRole('link', { name: 'Run run_000000000000000000000001' })).toBeVisible();
  });

  it('refetches from the server when the subscription handoff invalidates the Task', async () => {
    const submissions = vi
      .fn()
      .mockResolvedValueOnce(page('First client value'))
      .mockResolvedValueOnce(page('Updated by another client'));
    const source: ReviewSubmissionSource = { submissions };
    const view = render(<SubmittedReviewNotes refreshKey={0} source={source} taskId={taskId} />);
    expect(await screen.findByText('First client value')).toBeVisible();

    view.rerender(<SubmittedReviewNotes refreshKey={1} source={source} taskId={taskId} />);

    expect(await screen.findByText('Updated by another client')).toBeVisible();
    expect(submissions).toHaveBeenCalledTimes(2);
  });
});
