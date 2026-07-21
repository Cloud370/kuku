import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ReviewSnapshot } from '@/api/generated';

import { ChangeList } from './ChangeList';
import { DiffViewer } from './DiffViewer';
import { FileSearch } from './FileSearch';
import { FileViewer } from './FileViewer';
import type { ReviewDataSource } from './ReviewRoute';

const workspaceId = 'wsp_000000000000000000000001';
const revision = 'a'.repeat(64);

function source(tree: ReviewDataSource['tree']): ReviewDataSource {
  return {
    changes: vi.fn(),
    diff: vi.fn(),
    file: vi.fn(),
    search: vi.fn(),
    tree,
  };
}

afterEach(cleanup);

describe('Review paging', () => {
  it('continues file listing from the server cursor', async () => {
    const tree = vi
      .fn<ReviewDataSource['tree']>()
      .mockResolvedValueOnce({
        api_version: 1,
        entries: [
          {
            binary: false,
            change: null,
            kind: 'file',
            name: 'first.rs',
            path: 'src/first.rs',
            revision,
            size_bytes: 10,
          },
        ],
        next_cursor: 'next-files',
        revision,
        workspace_id: workspaceId,
      })
      .mockResolvedValueOnce({
        api_version: 1,
        entries: [
          {
            binary: false,
            change: null,
            kind: 'file',
            name: 'second.rs',
            path: 'src/second.rs',
            revision,
            size_bytes: 10,
          },
        ],
        next_cursor: null,
        revision,
        workspace_id: workspaceId,
      });
    render(<FileSearch dataSource={source(tree)} onSelect={vi.fn()} workspaceId={workspaceId} />);
    expect(await screen.findByText('src/first.rs')).toBeVisible();

    await userEvent.click(screen.getByRole('button', { name: 'Load more files' }));

    expect(await screen.findByText('src/second.rs')).toBeVisible();
    expect(tree).toHaveBeenLastCalledWith(workspaceId, {
      cursor: 'next-files',
      limit: 100,
      prefix: '',
    });
  });

  it('exposes the next aggregate Changes page', async () => {
    const onLoadMore = vi.fn();
    const snapshot: ReviewSnapshot = {
      api_version: 1,
      availability: 'available',
      entries: [
        {
          additions: 1,
          binary: false,
          deletions: 0,
          kind: 'modified',
          old_path: null,
          path: 'src/lib.rs',
          revision,
          staged: false,
          worktree: true,
        },
      ],
      next_cursor: 'next-changes',
      revision,
      workspace_id: workspaceId,
    };
    render(<ChangeList onLoadMore={onLoadMore} onSelect={vi.fn()} snapshot={snapshot} />);

    await userEvent.click(screen.getByRole('button', { name: 'Load more changes' }));

    expect(onLoadMore).toHaveBeenCalledWith('next-changes');
  });

  it('exposes bounded continuation for file and diff content', async () => {
    const onFileMore = vi.fn();
    const onDiffMore = vi.fn();
    const view = render(
      <FileViewer
        content={{
          api_version: 1,
          binary: false,
          end_line: 2,
          next_start_line: 3,
          path: 'src/lib.rs',
          revision,
          start_line: 1,
          text: 'one\ntwo',
          total_lines: 4,
          truncated: true,
          workspace_id: workspaceId,
        }}
        onDraftRange={vi.fn()}
        onLoadMore={onFileMore}
      />,
    );
    await userEvent.click(screen.getByRole('button', { name: 'Load more file content' }));
    expect(onFileMore).toHaveBeenCalledWith(3);

    view.rerender(
      <DiffViewer
        document={{
          api_version: 1,
          binary: false,
          hunks: [],
          next_cursor: 'next-diff',
          old_path: null,
          path: 'src/lib.rs',
          revision,
          truncated: true,
          workspace_id: workspaceId,
        }}
        onDraftRange={vi.fn()}
        onLoadMore={onDiffMore}
        view="unified"
      />,
    );
    await userEvent.click(screen.getByRole('button', { name: 'Load more diff' }));
    expect(onDiffMore).toHaveBeenCalledWith('next-diff');
  });
});
