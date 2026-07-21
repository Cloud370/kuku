import { Send } from 'lucide-react';

import { AnnotationComposer } from './AnnotationComposer';
import type { LocalReviewNote } from './annotationState';
import { canSubmitNotes } from './annotationValidation';

export function AnnotationDrafts({
  notes,
  onRemove,
  onSubmit,
  onUpdateComment,
}: {
  notes: readonly LocalReviewNote[];
  onRemove: (localId: string) => void;
  onSubmit: () => void;
  onUpdateComment: (localId: string, comment: string) => void;
}) {
  return (
    <section aria-label="Draft annotations" className="min-w-0 border border-[var(--color-border)]">
      <header className="flex h-11 items-center justify-between border-b border-[var(--color-border)] px-3">
        <h2 className="text-sm font-medium">Draft annotations</h2>
        <span className="text-xs text-[var(--color-text-muted)]">{String(notes.length)}</span>
      </header>
      {notes.length === 0 ? (
        <p className="p-4 text-sm text-[var(--color-text-muted)]">
          Select file or diff lines to add a note.
        </p>
      ) : (
        <div className="max-h-[28rem] overflow-auto">
          {notes.map((note) => (
            <AnnotationComposer
              key={note.localId}
              local={note}
              onRemove={onRemove}
              onUpdateComment={onUpdateComment}
            />
          ))}
        </div>
      )}
      <footer className="flex justify-end border-t border-[var(--color-border)] p-3">
        <button
          className="inline-flex h-9 items-center gap-2 rounded-[var(--radius-sm)] bg-[var(--color-accent)] px-3 text-sm text-white disabled:opacity-40"
          disabled={!canSubmitNotes(notes)}
          onClick={onSubmit}
          type="button"
        >
          <Send aria-hidden="true" size={15} />
          Submit review
        </button>
      </footer>
    </section>
  );
}
