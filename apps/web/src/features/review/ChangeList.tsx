import { FileDiff, FilePlus2, FileX2 } from 'lucide-react';
import type { ChangeEntry, ReviewSnapshot } from '@/api/generated';

import { selectChanges } from './reviewSelectors';

function ChangeIcon({ entry }: { entry: ChangeEntry }) {
  if (entry.kind === 'added' || entry.kind === 'untracked') {
    return <FilePlus2 aria-hidden="true" size={15} />;
  }
  if (entry.kind === 'deleted') return <FileX2 aria-hidden="true" size={15} />;
  return <FileDiff aria-hidden="true" size={15} />;
}

export function ChangeList({
  snapshot,
  onSelect,
  onLoadMore,
}: {
  snapshot: ReviewSnapshot;
  onSelect: (entry: ChangeEntry) => void;
  onLoadMore?: (cursor: string) => void;
}) {
  const view = selectChanges(snapshot);
  if (view.kind === 'unavailable') {
    return (
      <div className="grid min-h-48 place-items-center px-6 text-center text-sm text-[var(--color-text-secondary)]">
        <div>
          <p className="font-medium text-[var(--color-text-primary)]">
            Changes unavailable for this workspace
          </p>
          <p className="mt-1">Files remain available for review.</p>
        </div>
      </div>
    );
  }
  if (view.kind === 'empty') {
    return <p className="grid min-h-48 place-items-center text-sm">No workspace changes</p>;
  }
  const nextCursor = view.nextCursor;
  return (
    <>
      <ul aria-label="Workspace changes" className="divide-y divide-[var(--color-border)]">
        {view.entries.map((entry) => (
          <li key={`${entry.path}:${entry.revision}`}>
            <button
              className="grid w-full min-w-0 grid-cols-[20px_minmax(0,1fr)_auto] items-center gap-2 px-3 py-2 text-left hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
              onClick={() => {
                onSelect(entry);
              }}
              type="button"
            >
              <ChangeIcon entry={entry} />
              <span
                className="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-sm"
                title={entry.path}
              >
                {entry.path}
              </span>
              <span className="flex items-center gap-2 text-xs text-[var(--color-text-muted)]">
                {entry.binary ? 'Binary' : null}
                {entry.staged ? 'Staged' : null}
                {entry.worktree ? 'Working tree' : null}
                <span className="capitalize">{entry.kind.replace('_', ' ')}</span>
              </span>
            </button>
          </li>
        ))}
      </ul>
      {nextCursor !== null && onLoadMore ? (
        <button
          className="h-9 w-full border-t border-[var(--color-border)] text-sm"
          onClick={() => {
            onLoadMore(nextCursor);
          }}
          type="button"
        >
          Load more changes
        </button>
      ) : null}
    </>
  );
}
