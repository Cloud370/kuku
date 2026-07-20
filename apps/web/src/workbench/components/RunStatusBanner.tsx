import { AlertCircle, CheckCircle2, CircleStop, LoaderCircle } from 'lucide-react';

import type { RunProjection, TaskState } from '../../api/generated';

interface RunStatusBannerProps {
  run: RunProjection | null;
  taskId: string;
  taskState: TaskState;
  onOpenReview: (taskId: string) => void;
}

function label(state: TaskState): string {
  return state
    .split('_')
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(' ');
}

export function RunStatusBanner({ run, taskId, taskState, onOpenReview }: RunStatusBannerProps) {
  const completion = run?.completion ?? null;
  const active = ['queued', 'running', 'needs_attention', 'stopping'].includes(taskState);
  return (
    <section className="border-b border-[var(--color-border)] px-4 py-3" aria-label="Run status">
      <div className="flex items-center gap-2 text-sm font-medium">
        {taskState === 'needs_attention' || taskState === 'failed' ? (
          <AlertCircle aria-hidden="true" size={16} />
        ) : active ? (
          <LoaderCircle
            aria-hidden="true"
            className="animate-spin motion-reduce:animate-none"
            size={16}
          />
        ) : taskState === 'completed' ? (
          <CheckCircle2 aria-hidden="true" size={16} />
        ) : (
          <CircleStop aria-hidden="true" size={16} />
        )}
        {label(taskState)}
      </div>
      {completion !== null ? (
        <div className="mt-3 space-y-2 text-sm">
          <p>{completion.summary}</p>
          <p className="text-xs text-[var(--color-text-secondary)]">
            {completion.checks === null
              ? 'Checks unavailable'
              : `${String(completion.checks.filter((check) => check.passed).length)} of ${String(completion.checks.length)} checks passed`}
          </p>
          {completion.workspace_changes === null ? (
            <p className="text-xs text-[var(--color-text-secondary)]">
              Workspace changes unavailable
            </p>
          ) : (
            <button
              className="text-xs font-medium text-[var(--color-accent)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
              onClick={() => {
                onOpenReview(taskId);
              }}
              type="button"
            >
              View changes
            </button>
          )}
        </div>
      ) : null}
    </section>
  );
}
