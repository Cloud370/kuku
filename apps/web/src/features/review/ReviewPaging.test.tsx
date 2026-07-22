import '@testing-library/jest-dom/vitest';
import { act, cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { FileSearchPage, ReviewSnapshot } from '@/api/generated';

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

function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

function searchPage(path: string): FileSearchPage {
  return {
    api_version: 1,
    matches: [
      {
        entry: {
          binary: false,
          change: null,
          kind: 'file',
          name: path.split('/').at(-1) ?? path,
          path,
          revision,
          size_bytes: 10,
        },
        path_match_ranges: [],
      },
    ],
    next_cursor: null,
    revision,
    workspace_id: workspaceId,
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

  it('keeps the latest search when an older search finishes last', async () => {
    const firstRequest = deferred<FileSearchPage>();
    const secondRequest = deferred<FileSearchPage>();
    const api = source(
      vi.fn().mockResolvedValue({
        api_version: 1,
        entries: [],
        next_cursor: null,
        revision,
        workspace_id: workspaceId,
      }),
    );
    api.search = vi
      .fn<ReviewDataSource['search']>()
      .mockReturnValueOnce(firstRequest.promise)
      .mockReturnValueOnce(secondRequest.promise);
    const user = userEvent.setup();
    render(<FileSearch dataSource={api} onSelect={vi.fn()} workspaceId={workspaceId} />);
    const searchbox = await screen.findByRole('searchbox', { name: 'Find a file' });

    await user.type(searchbox, 'first{enter}');
    await user.clear(searchbox);
    await user.type(searchbox, 'second{enter}');
    await act(async () => {
      secondRequest.resolve(searchPage('src/second.ts'));
      await secondRequest.promise;
    });
    expect(screen.getByText('src/second.ts')).toBeVisible();

    await act(async () => {
      firstRequest.resolve(searchPage('src/first.ts'));
      await firstRequest.promise;
    });
    expect(screen.queryByText('src/first.ts')).toBeNull();
    expect(screen.getByText('src/second.ts')).toBeVisible();
  });

  it('does not append an old tree page to newer search results', async () => {
    const pageRequest = deferred<Awaited<ReturnType<ReviewDataSource['tree']>>>();
    const tree = vi
      .fn<ReviewDataSource['tree']>()
      .mockResolvedValueOnce({
        api_version: 1,
        entries: [
          {
            binary: false,
            change: null,
            kind: 'file',
            name: 'first.ts',
            path: 'src/first.ts',
            revision,
            size_bytes: 10,
          },
        ],
        next_cursor: 'next-files',
        revision,
        workspace_id: workspaceId,
      })
      .mockReturnValueOnce(pageRequest.promise);
    const api = source(tree);
    api.search = vi.fn().mockResolvedValue(searchPage('src/search-result.ts'));
    const user = userEvent.setup();
    render(<FileSearch dataSource={api} onSelect={vi.fn()} workspaceId={workspaceId} />);

    await user.click(await screen.findByRole('button', { name: 'Load more files' }));
    await user.type(screen.getByRole('searchbox', { name: 'Find a file' }), 'result{enter}');
    expect(await screen.findByText('src/search-result.ts')).toBeVisible();
    await act(async () => {
      pageRequest.resolve({
        api_version: 1,
        entries: [
          {
            binary: false,
            change: null,
            kind: 'file',
            name: 'stale.ts',
            path: 'src/stale.ts',
            revision,
            size_bytes: 10,
          },
        ],
        next_cursor: null,
        revision,
        workspace_id: workspaceId,
      });
      await pageRequest.promise;
    });

    expect(screen.queryByText('src/stale.ts')).toBeNull();
    expect(screen.getByText('src/search-result.ts')).toBeVisible();
  });
});
