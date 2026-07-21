import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { AnnotationDrafts } from './AnnotationDrafts';
import { makeLocalDraft } from './annotationState';

afterEach(cleanup);

describe('AnnotationDrafts', () => {
  it('disables submission for empty comments or outdated anchors', () => {
    const empty = makeLocalDraft(
      {
        comment: '',
        end_line: 4,
        excerpt: 'line',
        path: 'src/lib.rs',
        revision: 'a'.repeat(64),
        side: 'file',
        start_line: 4,
      },
      'empty',
    );
    const outdated = {
      ...makeLocalDraft({ ...empty.note, comment: 'Keep this' }, 'outdated'),
      state: 'outdated' as const,
    };
    render(
      <AnnotationDrafts
        notes={[empty, outdated]}
        onRemove={vi.fn()}
        onSubmit={vi.fn()}
        onUpdateComment={vi.fn()}
      />,
    );

    expect(screen.getByRole('button', { name: 'Submit review' })).toBeDisabled();
    expect(screen.getByText('Outdated anchor')).toBeVisible();
  });

  it('edits a comment without changing its anchor', () => {
    const onUpdateComment = vi.fn();
    const draft = makeLocalDraft(
      {
        comment: 'Initial',
        end_line: 9,
        excerpt: 'let value = 1;',
        path: 'src/lib.rs',
        revision: 'a'.repeat(64),
        side: 'new',
        start_line: 9,
      },
      'draft-one',
    );
    render(
      <AnnotationDrafts
        notes={[draft]}
        onRemove={vi.fn()}
        onSubmit={vi.fn()}
        onUpdateComment={onUpdateComment}
      />,
    );

    fireEvent.change(screen.getByRole('textbox', { name: 'Comment for src/lib.rs line 9' }), {
      target: { value: 'Revised' },
    });
    expect(onUpdateComment).toHaveBeenLastCalledWith('draft-one', 'Revised');
  });
});
