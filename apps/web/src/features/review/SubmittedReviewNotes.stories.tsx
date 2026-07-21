import type { Meta, StoryObj } from '@storybook/react';
import type { ReviewSubmissionPage } from '@/api/generated';

import { SubmittedReviewNotes, type ReviewSubmissionSource } from './SubmittedReviewNotes';

const taskId = 'tsk_000000000000000000000001';

function page(status: 'current' | 'outdated' = 'current'): ReviewSubmissionPage {
  return {
    api_version: 1,
    items: [
      {
        notes: [
          {
            comment: 'Please rename this before the next Run.',
            end_line: 20,
            excerpt: 'const previousName = value;',
            path: 'src/features/review/annotationState.ts',
            revision: 'a'.repeat(64),
            side: 'old',
            start_line: 20,
            status,
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

function source(value: ReviewSubmissionPage): ReviewSubmissionSource {
  return { submissions: () => Promise.resolve(value) };
}

const meta: Meta<typeof SubmittedReviewNotes> = {
  component: SubmittedReviewNotes,
  title: 'Experience/Review/Submitted Notes',
  args: {
    refreshKey: 0,
    source: source(page()),
    taskId,
  },
};

export default meta;
type Story = StoryObj<typeof SubmittedReviewNotes>;

export const Loading: Story = {
  args: { source: { submissions: () => new Promise(() => undefined) } },
};

export const Empty: Story = {
  args: { source: source({ ...page(), items: [] }) },
};

export const Error: Story = {
  args: {
    source: { submissions: () => Promise.reject(new globalThis.Error('offline')) },
  },
};

export const Ready: Story = {};
export const Outdated: Story = { args: { source: source(page('outdated')) } };
