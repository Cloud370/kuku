import { Search } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import type { FileEntry, WorkspaceId } from '@/api/generated';

import type { ReviewDataSource } from './ReviewRoute';
import { retryReviewRead } from './retryReviewRead';

type SearchRequest = { kind: 'tree' } | { kind: 'search'; query: string };

type SearchState =
  | { kind: 'loading' }
  | { kind: 'error' }
  | {
      kind: 'ready';
      entries: FileEntry[];
      emptyLabel: string;
      nextCursor: string | null;
      request: SearchRequest;
      revision: string;
    };

function sameRequest(left: SearchRequest, right: SearchRequest) {
  if (left.kind !== right.kind) return false;
  return left.kind === 'tree' || (right.kind === 'search' && left.query === right.query);
}

export function FileSearch({
  dataSource,
  onSelect,
  workspaceId,
}: {
  dataSource: ReviewDataSource;
  onSelect: (path: string) => void;
  workspaceId: WorkspaceId;
}) {
  const [query, setQuery] = useState('');
  const [state, setState] = useState<SearchState>({ kind: 'loading' });
  const [loadingMore, setLoadingMore] = useState(false);
  const requestGeneration = useRef(0);

  useEffect(() => {
    const generation = ++requestGeneration.current;
    setQuery('');
    setState({ kind: 'loading' });
    setLoadingMore(false);
    void retryReviewRead(() =>
      dataSource.tree(workspaceId, { cursor: null, limit: 100, prefix: '' }),
    )
      .then((page) => {
        if (generation !== requestGeneration.current) return;
        if (page.workspace_id !== workspaceId) {
          setState({ kind: 'error' });
          return;
        }
        setState({
          emptyLabel: 'No files found',
          entries: page.entries,
          kind: 'ready',
          nextCursor: page.next_cursor,
          request: { kind: 'tree' },
          revision: page.revision,
        });
      })
      .catch(() => {
        if (generation === requestGeneration.current) setState({ kind: 'error' });
      });
    return () => {
      if (generation === requestGeneration.current) requestGeneration.current += 1;
    };
  }, [dataSource, workspaceId]);

  async function submit() {
    const normalized = query.trim();
    if (!normalized) return;
    const generation = ++requestGeneration.current;
    setState({ kind: 'loading' });
    setLoadingMore(false);
    try {
      const page = await retryReviewRead(() =>
        dataSource.search(workspaceId, {
          cursor: null,
          limit: 100,
          prefix: '',
          query: normalized,
        }),
      );
      if (generation !== requestGeneration.current) return;
      if (page.workspace_id !== workspaceId) {
        setState({ kind: 'error' });
        return;
      }
      setState({
        kind: 'ready',
        entries: page.matches.map((match) => match.entry),
        emptyLabel: 'No matching files',
        nextCursor: page.next_cursor,
        request: { kind: 'search', query: normalized },
        revision: page.revision,
      });
    } catch {
      if (generation === requestGeneration.current) setState({ kind: 'error' });
    }
  }

  async function loadMore() {
    if (state.kind !== 'ready' || state.nextCursor === null) return;
    const base = state;
    const generation = ++requestGeneration.current;
    setLoadingMore(true);
    try {
      let entries: FileEntry[];
      let nextCursor: string | null;
      let revision: string;
      let responseWorkspaceId: WorkspaceId;
      if (base.request.kind === 'tree') {
        const page = await retryReviewRead(() =>
          dataSource.tree(workspaceId, {
            cursor: base.nextCursor,
            limit: 100,
            prefix: '',
          }),
        );
        entries = page.entries;
        nextCursor = page.next_cursor;
        revision = page.revision;
        responseWorkspaceId = page.workspace_id;
      } else {
        const searchQuery = base.request.query;
        const page = await retryReviewRead(() =>
          dataSource.search(workspaceId, {
            cursor: base.nextCursor,
            limit: 100,
            prefix: '',
            query: searchQuery,
          }),
        );
        entries = page.matches.map((match) => match.entry);
        nextCursor = page.next_cursor;
        revision = page.revision;
        responseWorkspaceId = page.workspace_id;
      }
      if (generation !== requestGeneration.current) return;
      if (responseWorkspaceId !== workspaceId || revision !== base.revision) {
        setState({ kind: 'error' });
        return;
      }
      setState((current) => {
        if (
          generation !== requestGeneration.current ||
          current.kind !== 'ready' ||
          current.nextCursor !== base.nextCursor ||
          current.revision !== base.revision ||
          !sameRequest(current.request, base.request)
        ) {
          return current;
        }
        return {
          ...current,
          entries: [...current.entries, ...entries],
          nextCursor,
        };
      });
    } catch {
      if (generation === requestGeneration.current) setState({ kind: 'error' });
    } finally {
      if (generation === requestGeneration.current) setLoadingMore(false);
    }
  }

  return (
    <section aria-label="Files" className="min-w-0 border-r border-[var(--color-border)]">
      <form
        className="flex h-12 items-center gap-2 border-b border-[var(--color-border)] px-2"
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <Search aria-hidden="true" size={15} />
        <input
          aria-label="Find a file"
          className="h-8 min-w-0 flex-1 bg-transparent text-sm outline-none"
          onChange={(event) => {
            setQuery(event.target.value);
          }}
          placeholder="Find a file"
          type="search"
          value={query}
        />
      </form>
      <div className="max-h-[calc(100dvh-9rem)] overflow-auto">
        {state.kind === 'loading' ? (
          <p className="p-3 text-sm" role="status">
            Loading files
          </p>
        ) : null}
        {state.kind === 'error' ? (
          <p className="p-3 text-sm" role="alert">
            Unable to load files
          </p>
        ) : null}
        {state.kind === 'ready' && state.entries.length === 0 ? (
          <p className="p-3 text-sm text-[var(--color-text-muted)]">{state.emptyLabel}</p>
        ) : null}
        {state.kind === 'ready' ? (
          <>
            <ul className="divide-y divide-[var(--color-border)]">
              {state.entries.map((entry) => (
                <li key={entry.path}>
                  <button
                    className="w-full overflow-hidden text-ellipsis whitespace-nowrap px-3 py-2 text-left text-sm disabled:text-[var(--color-text-muted)]"
                    disabled={entry.kind === 'directory'}
                    onClick={() => {
                      onSelect(entry.path);
                    }}
                    title={entry.path}
                    type="button"
                  >
                    {entry.path}
                  </button>
                </li>
              ))}
            </ul>
            {state.nextCursor !== null ? (
              <button
                className="h-9 w-full border-t border-[var(--color-border)] text-sm"
                disabled={loadingMore}
                onClick={() => {
                  void loadMore();
                }}
                type="button"
              >
                Load more files
              </button>
            ) : null}
          </>
        ) : null}
      </div>
    </section>
  );
}
