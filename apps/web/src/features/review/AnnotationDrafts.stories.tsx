import type { Meta, StoryObj } from '@storybook/react';

import { AnnotationDrafts } from './AnnotationDrafts';
import { makeLocalDraft, type LocalReviewNote } from './annotationState';

const noop = () => undefined;

function draft(overrides: Partial<LocalReviewNote> = {}): LocalReviewNote {
  return {
    ...makeLocalDraft(
      {
        comment: 'Please keep the name aligned with the public API.',
        end_line: 20,
        excerpt: 'const next = createReviewState();',
        path: 'src/features/review/annotationState.ts',
        revision: 'a'.repeat(64),
        side: 'new',
        start_line: 18,
      },
      'story-note',
    ),
    ...overrides,
  };
}

const meta: Meta<typeof AnnotationDrafts> = {
  component: AnnotationDrafts,
  title: 'Experience/Review/Annotation Drafts',
  args: {
    notes: [draft()],
    onRemove: noop,
    onSubmit: noop,
    onUpdateComment: noop,
  },
};

export default meta;
type Story = StoryObj<typeof AnnotationDrafts>;

export const Empty: Story = { args: { notes: [] } };
export const Ready: Story = {};
export const Outdated: Story = { args: { notes: [draft({ state: 'outdated' })] } };
export const Submitting: Story = { args: { notes: [draft({ state: 'submitting' })] } };
export const Error: Story = {
  args: {
    notes: [
      draft({
        error: {
          api_version: 1,
          code: 'stale_command',
          details: null,
          message: 'Task revision changed',
          trace_id: 'story-review',
        },
        state: 'error',
      }),
    ],
  },
};

export const LongPath: Story = {
  args: {
    notes: [
      draft({
        note: {
          ...draft().note,
          path: `${'very-long-directory/'.repeat(12)}annotationState.ts`,
        },
      }),
    ],
  },
};
