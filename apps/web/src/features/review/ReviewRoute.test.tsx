import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AnnotationDraft, DiffDocument, FileContent, ReviewSnapshot } from '@/api/generated';

import { DiffViewer } from './DiffViewer';
import { ReviewCode, ReviewRoute, type ReviewDataSource } from './ReviewRoute';

const workspaceId = 'wsp_000000000000000000000001';
const taskId = 'tsk_000000000000000000000001';
const revision = 'a'.repeat(64);

function changes(availability: ReviewSnapshot['availability'] = 'available'): ReviewSnapshot {
  return {
    api_version: 1,
    availability,
    entries: [],
    next_cursor: null,
    revision,
    workspace_id: workspaceId,
  };
}

function diff(): DiffDocument {
  return {
    api_version: 1,
    binary: false,
    hunks: [
      {
        lines: [
          { kind: 'addition', new_line: 18, old_line: null, text: 'const next = 1;' },
          { kind: 'addition', new_line: 19, old_line: null, text: 'return next;' },
          { kind: 'addition', new_line: 20, old_line: null, text: '}' },
        ],
        new_lines: 3,
        new_start: 18,
        old_lines: 0,
        old_start: 18,
      },
    ],
    next_cursor: null,
    old_path: null,
    path: 'src/lib.ts',
    revision,
    truncated: false,
    workspace_id: workspaceId,
  };
}

function source(snapshot = changes()): ReviewDataSource {
  return {
    changes: vi.fn().mockResolvedValue(snapshot),
    diff: vi.fn().mockResolvedValue(diff()),
    file: vi.fn().mockResolvedValue({
      api_version: 1,
      binary: false,
      end_line: 2,
      next_start_line: null,
      path: 'src/lib.ts',
      revision,
      start_line: 1,
      text: 'one\ntwo',
      total_lines: 2,
      truncated: false,
      workspace_id: workspaceId,
    } satisfies FileContent),
    search: vi.fn().mockResolvedValue({
      api_version: 1,
      matches: [],
      next_cursor: null,
      revision,
      workspace_id: workspaceId,
    }),
    tree: vi.fn().mockResolvedValue({
      api_version: 1,
      entries: [],
      next_cursor: null,
      revision,
      workspace_id: workspaceId,
    }),
  };
}

afterEach(cleanup);

describe('ReviewRoute', () => {
  it('keeps Files available when Changes is unavailable', async () => {
    const api = source(changes('not_git_repository'));
    render(
      <ReviewRoute
        dataSource={api}
        mode="changes"
        onDraftRange={vi.fn()}
        onLeave={vi.fn()}
        presentation="full"
        taskId={taskId}
        workspaceId={workspaceId}
      />,
    );

    expect(await screen.findByText('Changes unavailable for this workspace')).toBeVisible();
    expect(screen.getByRole('tab', { name: 'Files' })).toBeEnabled();
    await userEvent.click(screen.getByRole('tab', { name: 'Files' }));
    expect(await screen.findByRole('searchbox', { name: 'Find a file' })).toBeVisible();
  });

  it('renders explicit loading, error, and empty Changes states', async () => {
    let resolveChanges: ((value: ReviewSnapshot) => void) | undefined;
    const api = source();
    api.changes = vi.fn().mockReturnValue(
      new Promise<ReviewSnapshot>((resolve) => {
        resolveChanges = resolve;
      }),
    );
    const view = render(
      <ReviewRoute
        dataSource={api}
        mode="changes"
        onDraftRange={vi.fn()}
        onLeave={vi.fn()}
        presentation="embedded"
        taskId={taskId}
        workspaceId={workspaceId}
      />,
    );
    expect(screen.getByRole('status')).toHaveTextContent('Loading changes');
    resolveChanges?.(changes());
    expect(await screen.findByText('No workspace changes')).toBeVisible();

    api.changes = vi.fn().mockRejectedValue(new Error('offline'));
    view.rerender(
      <ReviewRoute
        dataSource={api}
        mode="files"
        onDraftRange={vi.fn()}
        onLeave={vi.fn()}
        presentation="embedded"
        taskId={taskId}
        workspaceId={workspaceId}
      />,
    );
    await userEvent.click(screen.getByRole('tab', { name: 'Changes' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Unable to load changes');
  });
});

describe('safe Review rendering', () => {
  it('anchors a selected diff range to the new side', async () => {
    const onDraftRange = vi.fn<(anchor: AnnotationDraft) => void>();
    render(<DiffViewer document={diff()} onDraftRange={onDraftRange} view="unified" />);
    const user = userEvent.setup();

    await user.click(screen.getByRole('button', { name: 'Select new line 18' }));
    await user.click(screen.getByRole('button', { name: 'Select new line 20' }));

    expect(onDraftRange).toHaveBeenCalledWith({
      comment: '',
      end_line: 20,
      excerpt: 'const next = 1;\nreturn next;\n}',
      path: 'src/lib.ts',
      revision,
      side: 'new',
      start_line: 18,
    });
  });

  it('renders unknown language source as escaped plaintext', async () => {
    render(<ReviewCode language="made-up-lang" source="<script>alert(1)</script>" />);

    expect(screen.getByText('<script>alert(1)</script>')).toBeVisible();
    expect(document.querySelector('script')).toBeNull();
    await waitFor(() => expect(screen.getByRole('region')).toHaveAccessibleName('plaintext code'));
  });

  it('renders split diff as independent old and new side regions', () => {
    const document = diff();
    document.hunks[0]?.lines.unshift({
      kind: 'deletion',
      new_line: null,
      old_line: 18,
      text: 'const previous = 1;',
    });
    render(<DiffViewer document={document} onDraftRange={vi.fn()} view="split" />);

    expect(screen.getByRole('region', { name: 'Old side' })).toHaveTextContent(
      'const previous = 1;',
    );
    expect(screen.getByRole('region', { name: 'New side' })).toHaveTextContent('const next = 1;');
  });
});
