import { useEffect, useState } from 'react';
import type { ReviewSubmissionProjection, RunId, TaskId } from '@/api/generated';
import { webApi } from '../../api/client';

import { mergeSubmissionPage } from './reviewSubmissionCache';
import { retryReviewRead } from './retryReviewRead';

export type ReviewSubmissionSource = Pick<typeof webApi.review, 'submissions'>;

type SubmissionState =
  | { kind: 'loading' }
  | { kind: 'error' }
  | { kind: 'ready'; items: ReviewSubmissionProjection[]; nextCursor: string | null };

export function SubmittedReviewNotes({
  onOpenRun,
  refreshKey,
  source = webApi.review,
  taskId,
}: {
  onOpenRun?: (runId: RunId) => void;
  refreshKey: number;
  source?: ReviewSubmissionSource;
  taskId: TaskId;
}) {
  const [state, setState] = useState<SubmissionState>({ kind: 'loading' });

  useEffect(() => {
    let current = true;
    setState({ kind: 'loading' });
    void retryReviewRead(() => source.submissions(taskId, { cursor: null, limit: 50 }))
      .then((page) => {
        if (current) {
          setState({
            items: mergeSubmissionPage([], page),
            kind: 'ready',
            nextCursor: page.next_cursor,
          });
        }
      })
      .catch(() => {
        if (current) setState({ kind: 'error' });
      });
    return () => {
      current = false;
    };
  }, [refreshKey, source, taskId]);

  async function loadMore() {
    if (state.kind !== 'ready' || state.nextCursor === null) return;
    const page = await retryReviewRead(() =>
      source.submissions(taskId, { cursor: state.nextCursor, limit: 50 }),
    );
    setState({
      items: mergeSubmissionPage(state.items, page),
      kind: 'ready',
      nextCursor: page.next_cursor,
    });
  }

  if (state.kind === 'loading') return <p role="status">Loading submitted reviews</p>;
  if (state.kind === 'error') return <p role="alert">Unable to load submitted reviews</p>;
  if (state.items.length === 0) return <p>No submitted reviews</p>;
  return (
    <section aria-label="Submitted reviews" className="min-w-0">
      <h2 className="mb-2 text-sm font-medium">Submitted reviews</h2>
      <div className="grid gap-3">
        {state.items.map((submission) => (
          <article className="border border-[var(--color-border)]" key={submission.submission_id}>
            <header className="flex flex-wrap items-center justify-between gap-2 border-b border-[var(--color-border)] px-3 py-2 text-xs">
              <span>{submission.submitted_at}</span>
              {onOpenRun ? (
                <button
                  className="text-[var(--color-accent)]"
                  onClick={() => {
                    onOpenRun(submission.run_id);
                  }}
                  type="button"
                >
                  Run {submission.run_id}
                </button>
              ) : (
                <a
                  className="text-[var(--color-accent)]"
                  href={`/tasks/${encodeURIComponent(taskId)}#run-${encodeURIComponent(submission.run_id)}`}
                >
                  Run {submission.run_id}
                </a>
              )}
            </header>
            <ul className="divide-y divide-[var(--color-border)]">
              {submission.notes.map((note, index) => (
                <li
                  className="p-3"
                  key={`${note.path}:${note.side}:${String(note.start_line)}:${String(index)}`}
                >
                  <p
                    className="overflow-hidden text-ellipsis whitespace-nowrap text-sm font-medium"
                    title={note.path}
                  >
                    {note.path}
                  </p>
                  <p className="text-xs text-[var(--color-text-muted)]">
                    {note.side} · {String(note.start_line)}-{String(note.end_line)} · {note.status}
                  </p>
                  <pre className="my-2 max-h-24 overflow-auto whitespace-pre-wrap break-words border-l-2 border-[var(--color-border)] pl-3 text-xs">
                    {note.excerpt}
                  </pre>
                  <p className="whitespace-pre-wrap break-words text-sm">{note.comment}</p>
                </li>
              ))}
            </ul>
          </article>
        ))}
      </div>
      {state.nextCursor !== null ? (
        <button
          className="mt-3 h-9 rounded-[var(--radius-sm)] border border-[var(--color-border)] px-3 text-sm"
          onClick={() => {
            void loadMore();
          }}
          type="button"
        >
          Load more
        </button>
      ) : null}
    </section>
  );
}
