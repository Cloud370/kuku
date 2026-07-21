import type { DiffDocument, FileContent } from '@/api/generated';

import type { LocalReviewNote } from './annotationState';
import { excerptAt } from './reviewSelectors';

export type NoteValidation = { kind: 'ready' } | { kind: 'outdated' };

export function validateNote(
  local: LocalReviewNote,
  current: FileContent | DiffDocument,
): NoteValidation {
  const note = local.note;
  if (note.path !== current.path || note.revision !== current.revision) {
    return { kind: 'outdated' };
  }
  return excerptAt(current, note.side, note.start_line, note.end_line) === note.excerpt
    ? { kind: 'ready' }
    : { kind: 'outdated' };
}

export function canSubmitNotes(notes: readonly LocalReviewNote[]): boolean {
  return (
    notes.length > 0 &&
    notes.every(
      (note) =>
        note.note.comment.trim().length > 0 &&
        note.state !== 'outdated' &&
        note.state !== 'submitting',
    )
  );
}
