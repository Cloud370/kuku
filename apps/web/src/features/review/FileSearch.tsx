import { Search } from 'lucide-react';
import { useEffect, useState } from 'react';
import type { FileEntry, WorkspaceId } from '@/api/generated';

import type { ReviewDataSource } from './ReviewRoute';

type SearchState =
  | { kind: 'loading' }
  | { kind: 'error' }
  | {
      kind: 'ready';
      entries: FileEntry[];
      emptyLabel: string;
      nextCursor: string | null;
      request: { kind: 'tree' } | { kind: 'search'; query: string };
    };

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

  useEffect(() => {
    let current = true;
    void dataSource
      .tree(workspaceId, { cursor: null, limit: 100, prefix: '' })
      .then((page) => {
        if (current) {
          setState({
            emptyLabel: 'No files found',
            entries: page.entries,
            kind: 'ready',
            nextCursor: page.next_cursor,
            request: { kind: 'tree' },
          });
        }
      })
      .catch(() => {
        if (current) setState({ kind: 'error' });
      });
    return () => {
      current = false;
    };
  }, [dataSource, workspaceId]);

  async function submit() {
    const normalized = query.trim();
    if (!normalized) return;
    setState({ kind: 'loading' });
    try {
      const page = await dataSource.search(workspaceId, {
        cursor: null,
        limit: 100,
        prefix: '',
        query: normalized,
      });
      setState({
        kind: 'ready',
        entries: page.matches.map((match) => match.entry),
        emptyLabel: 'No matching files',
        nextCursor: page.next_cursor,
        request: { kind: 'search', query: normalized },
      });
    } catch {
      setState({ kind: 'error' });
    }
  }

  async function loadMore() {
    if (state.kind !== 'ready' || state.nextCursor === null) return;
    setLoadingMore(true);
    try {
      let entries: FileEntry[];
      let nextCursor: string | null;
      if (state.request.kind === 'tree') {
        const page = await dataSource.tree(workspaceId, {
          cursor: state.nextCursor,
          limit: 100,
          prefix: '',
        });
        entries = page.entries;
        nextCursor = page.next_cursor;
      } else {
        const page = await dataSource.search(workspaceId, {
          cursor: state.nextCursor,
          limit: 100,
          prefix: '',
          query: state.request.query,
        });
        entries = page.matches.map((match) => match.entry);
        nextCursor = page.next_cursor;
      }
      setState({
        ...state,
        entries: [...state.entries, ...entries],
        nextCursor,
      });
    } catch {
      setState({ kind: 'error' });
    } finally {
      setLoadingMore(false);
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
