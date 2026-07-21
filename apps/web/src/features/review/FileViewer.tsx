import { useMemo, useState } from 'react';
import type { AnnotationDraft, FileContent } from '@/api/generated';
import { SafeCodeBlock } from '../../components/content/SafeCodeBlock';

import { resolveReviewLanguage } from './reviewLanguage';
import { excerptAt } from './reviewSelectors';

export function FileViewer({
  content,
  onDraftRange,
  onLoadMore,
}: {
  content: FileContent;
  onDraftRange: (anchor: AnnotationDraft) => void;
  onLoadMore?: (nextStartLine: number) => void;
}) {
  const [startLine, setStartLine] = useState(content.start_line);
  const [endLine, setEndLine] = useState(content.start_line);
  const excerpt = useMemo(
    () => excerptAt(content, 'file', startLine, endLine),
    [content, endLine, startLine],
  );
  const nextStartLine = content.next_start_line;
  return (
    <article className="min-w-0 p-3">
      <header className="mb-3 min-w-0">
        <p
          className="overflow-hidden text-ellipsis whitespace-nowrap text-sm font-medium"
          title={content.path}
        >
          {content.path}
        </p>
        <p className="mt-1 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-xs text-[var(--color-text-muted)]">
          Revision {content.revision}
        </p>
      </header>
      {content.binary ? <p>Binary file preview is unavailable</p> : null}
      {!content.binary && content.text === null ? <p>File content is unavailable</p> : null}
      {!content.binary && content.text !== null ? (
        <>
          <SafeCodeBlock code={content.text} language={resolveReviewLanguage(content.path)} />
          <div className="flex flex-wrap items-end gap-2 border-t border-[var(--color-border)] pt-3">
            <label className="grid gap-1 text-xs">
              Start line
              <input
                className="h-8 w-24 rounded-[var(--radius-sm)] border border-[var(--color-border)] bg-transparent px-2"
                min={content.start_line}
                onChange={(event) => {
                  setStartLine(event.target.valueAsNumber);
                }}
                type="number"
                value={startLine}
              />
            </label>
            <label className="grid gap-1 text-xs">
              End line
              <input
                className="h-8 w-24 rounded-[var(--radius-sm)] border border-[var(--color-border)] bg-transparent px-2"
                min={startLine}
                onChange={(event) => {
                  setEndLine(event.target.valueAsNumber);
                }}
                type="number"
                value={endLine}
              />
            </label>
            <button
              className="h-8 rounded-[var(--radius-sm)] bg-[var(--color-accent)] px-3 text-sm text-white disabled:opacity-40"
              disabled={excerpt === null}
              onClick={() => {
                if (excerpt === null) return;
                onDraftRange({
                  comment: '',
                  end_line: endLine,
                  excerpt,
                  path: content.path,
                  revision: content.revision,
                  side: 'file',
                  start_line: startLine,
                });
              }}
              type="button"
            >
              Add annotation
            </button>
          </div>
          {content.truncated ? <p className="mt-2 text-xs">File preview is truncated</p> : null}
          {nextStartLine !== null && onLoadMore ? (
            <button
              className="mt-2 h-9 rounded-[var(--radius-sm)] border border-[var(--color-border)] px-3 text-sm"
              onClick={() => {
                onLoadMore(nextStartLine);
              }}
              type="button"
            >
              Load more file content
            </button>
          ) : null}
        </>
      ) : null}
    </article>
  );
}
