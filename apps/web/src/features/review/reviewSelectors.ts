import type {
  AnnotationSide,
  ChangeEntry,
  DiffDocument,
  FileContent,
  ReviewSnapshot,
} from '@/api/generated';

export type ChangesView =
  | { kind: 'unavailable'; reason: ReviewSnapshot['availability'] }
  | { kind: 'empty' }
  | { kind: 'ready'; entries: ChangeEntry[]; nextCursor: string | null };

export function selectChanges(snapshot: ReviewSnapshot): ChangesView {
  if (snapshot.availability !== 'available') {
    return { kind: 'unavailable', reason: snapshot.availability };
  }
  if (snapshot.entries.length === 0) return { kind: 'empty' };
  return {
    kind: 'ready',
    entries: snapshot.entries,
    nextCursor: snapshot.next_cursor,
  };
}

function isFileContent(current: FileContent | DiffDocument): current is FileContent {
  return (
    typeof current.start_line === 'number' &&
    (typeof current.text === 'string' || current.text === null)
  );
}

export function excerptAt(
  current: FileContent | DiffDocument,
  side: AnnotationSide,
  startLine: number,
  endLine: number,
): string | null {
  if (startLine < 1 || endLine < startLine) return null;
  if (isFileContent(current)) {
    if (side !== 'file' || current.binary || current.text === null) return null;
    const offset = startLine - current.start_line;
    const count = endLine - startLine + 1;
    if (offset < 0) return null;
    const lines = current.text.split('\n').slice(offset, offset + count);
    return lines.length === count ? lines.join('\n') : null;
  }
  if (side === 'file' || current.binary) return null;
  const selected = current.hunks
    .flatMap((hunk) => hunk.lines)
    .filter((line) => {
      const number = side === 'old' ? line.old_line : line.new_line;
      return number !== null && number >= startLine && number <= endLine;
    });
  const numbers = selected.map((line) => (side === 'old' ? line.old_line : line.new_line));
  const expected = Array.from({ length: endLine - startLine + 1 }, (_, index) => startLine + index);
  return numbers.length === expected.length &&
    numbers.every((number, index) => number === expected[index])
    ? selected.map((line) => line.text).join('\n')
    : null;
}
