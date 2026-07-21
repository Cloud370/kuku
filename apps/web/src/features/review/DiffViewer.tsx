import { useState } from 'react';
import type { AnnotationDraft, AnnotationSide, DiffDocument, DiffLine } from '@/api/generated';

import { excerptAt } from './reviewSelectors';

export type DiffView = 'unified' | 'split';
type Selection = { side: Exclude<AnnotationSide, 'file'>; line: number } | null;

function lineNumber(line: DiffLine, side: Exclude<AnnotationSide, 'file'>) {
  return side === 'old' ? line.old_line : line.new_line;
}

function SplitSide({
  lines,
  onSelect,
  side,
}: {
  lines: DiffLine[];
  onSelect: (side: Exclude<AnnotationSide, 'file'>, line: number) => void;
  side: Exclude<AnnotationSide, 'file'>;
}) {
  const visible = lines.flatMap((line, index) => {
    const number = lineNumber(line, side);
    return number === null ? [] : [{ index, line, number }];
  });
  return (
    <section
      aria-label={side === 'old' ? 'Old side' : 'New side'}
      className="min-w-0 overflow-auto border-r border-[var(--color-border)]"
      role="region"
    >
      <h2 className="sticky top-0 border-b border-[var(--color-border)] bg-[var(--color-surface)] px-3 py-2 text-xs font-medium">
        {side === 'old' ? 'Old' : 'New'}
      </h2>
      <ol className="min-w-max font-mono text-xs">
        {visible.map(({ index, line, number }) => (
          <li
            className="grid min-h-7 grid-cols-[52px_minmax(16rem,1fr)] border-b border-[var(--color-border)]"
            data-kind={line.kind}
            key={`${side}:${String(index)}:${String(number)}`}
          >
            <button
              aria-label={`Select ${side} line ${String(number)}`}
              className="border-r border-[var(--color-border)] px-2 text-right hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
              onClick={() => {
                onSelect(side, number);
              }}
              type="button"
            >
              {number}
            </button>
            <code className="whitespace-pre-wrap break-words px-3 py-1">{line.text}</code>
          </li>
        ))}
      </ol>
    </section>
  );
}

export function DiffViewer({
  document,
  onDraftRange,
  onLoadMore,
  view,
}: {
  document: DiffDocument;
  onDraftRange: (anchor: AnnotationDraft) => void;
  onLoadMore?: (cursor: string) => void;
  view: DiffView;
}) {
  const [selection, setSelection] = useState<Selection>(null);
  const nextCursor = document.next_cursor;

  function select(side: Exclude<AnnotationSide, 'file'>, line: number) {
    if (selection === null || selection.side !== side) {
      setSelection({ side, line });
      return;
    }
    const start = Math.min(selection.line, line);
    const end = Math.max(selection.line, line);
    const excerpt = excerptAt(document, side, start, end);
    if (excerpt !== null) {
      onDraftRange({
        comment: '',
        end_line: end,
        excerpt,
        path: document.path,
        revision: document.revision,
        side,
        start_line: start,
      });
      setSelection(null);
    }
  }

  if (document.binary) return <p className="p-4">Binary diff preview is unavailable</p>;
  const lines = document.hunks.flatMap((hunk) => hunk.lines);
  return (
    <article className="min-w-0 overflow-auto" data-view={view}>
      <header className="sticky top-0 z-10 border-b border-[var(--color-border)] bg-[var(--color-surface)] px-3 py-2">
        <p
          className="overflow-hidden text-ellipsis whitespace-nowrap text-sm font-medium"
          title={document.path}
        >
          {document.path}
        </p>
        {document.old_path ? <p className="text-xs">Renamed from {document.old_path}</p> : null}
      </header>
      {view === 'split' ? (
        <div className="grid min-w-[36rem] grid-cols-2">
          <SplitSide lines={lines} onSelect={select} side="old" />
          <SplitSide lines={lines} onSelect={select} side="new" />
        </div>
      ) : (
        <ol className="min-w-max font-mono text-xs">
          {document.hunks.flatMap((hunk, hunkIndex) =>
            hunk.lines.map((line, lineIndex) => (
              <li
                className="grid min-h-7 grid-cols-[52px_52px_minmax(24rem,1fr)] items-stretch border-b border-[var(--color-border)]"
                data-kind={line.kind}
                key={`${String(hunkIndex)}:${String(lineIndex)}`}
              >
                {(['old', 'new'] as const).map((side) => {
                  const number = lineNumber(line, side);
                  return number === null ? (
                    <span
                      aria-hidden="true"
                      className="border-r border-[var(--color-border)]"
                      key={side}
                    />
                  ) : (
                    <button
                      aria-label={`Select ${side} line ${String(number)}`}
                      className="border-r border-[var(--color-border)] px-2 text-right hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
                      key={side}
                      onClick={() => {
                        select(side, number);
                      }}
                      type="button"
                    >
                      {number}
                    </button>
                  );
                })}
                <code className="whitespace-pre-wrap break-words px-3 py-1">{line.text}</code>
              </li>
            )),
          )}
        </ol>
      )}
      {document.truncated ? <p className="p-3 text-xs">Diff preview is truncated</p> : null}
      {nextCursor !== null && onLoadMore ? (
        <button
          className="m-3 h-9 rounded-[var(--radius-sm)] border border-[var(--color-border)] px-3 text-sm"
          onClick={() => {
            onLoadMore(nextCursor);
          }}
          type="button"
        >
          Load more diff
        </button>
      ) : null}
    </article>
  );
}
