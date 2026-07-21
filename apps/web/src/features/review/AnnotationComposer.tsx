import { Trash2 } from 'lucide-react';

import type { LocalReviewNote } from './annotationState';

function rangeLabel(note: LocalReviewNote): string {
  const { end_line: end, start_line: start } = note.note;
  return start === end ? `line ${String(start)}` : `lines ${String(start)}-${String(end)}`;
}

export function AnnotationComposer({
  local,
  onRemove,
  onUpdateComment,
}: {
  local: LocalReviewNote;
  onRemove: (localId: string) => void;
  onUpdateComment: (localId: string, comment: string) => void;
}) {
  const label = `Comment for ${local.note.path} ${rangeLabel(local)}`;
  return (
    <article className="border-b border-[var(--color-border)] p-3">
      <header className="grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-start gap-2">
        <div className="min-w-0">
          <p
            className="overflow-hidden text-ellipsis whitespace-nowrap text-sm font-medium"
            title={local.note.path}
          >
            {local.note.path}
          </p>
          <p className="text-xs text-[var(--color-text-muted)]">
            {local.note.side} · {rangeLabel(local)}
          </p>
        </div>
        <button
          aria-label={`Remove annotation for ${local.note.path}`}
          className="inline-flex size-8 items-center justify-center rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)]"
          onClick={() => {
            onRemove(local.localId);
          }}
          title="Remove annotation"
          type="button"
        >
          <Trash2 aria-hidden="true" size={15} />
        </button>
      </header>
      <pre className="my-2 max-h-24 overflow-auto whitespace-pre-wrap break-words border-l-2 border-[var(--color-border)] pl-3 text-xs">
        {local.note.excerpt}
      </pre>
      <label className="grid gap-1 text-xs">
        <span>{label}</span>
        <textarea
          aria-label={label}
          className="min-h-20 resize-y rounded-[var(--radius-sm)] border border-[var(--color-border)] bg-transparent p-2 text-sm outline-none focus:border-[var(--color-accent)]"
          onChange={(event) => {
            onUpdateComment(local.localId, event.target.value);
          }}
          value={local.note.comment}
        />
      </label>
      {local.state === 'outdated' ? (
        <p className="mt-2 text-xs text-[var(--color-error)]">Outdated anchor</p>
      ) : null}
      {local.state === 'error' && local.error !== null ? (
        <p className="mt-2 text-xs text-[var(--color-error)]">{local.error.code}</p>
      ) : null}
    </article>
  );
}
