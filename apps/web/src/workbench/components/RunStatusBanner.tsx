import { AlertCircle, CheckCircle2, CircleStop, LoaderCircle } from 'lucide-react';

import type { MetricProjection, RunProjection, TaskState } from '../../api/generated';

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

function formatMetric(metric: MetricProjection): string {
  const unit = metric.unit === null ? '' : ` ${metric.unit}`;
  return `${metric.name}: ${metric.value.toLocaleString('en-US')}${unit}`;
}

export function RunStatusBanner({ run, taskId, taskState, onOpenReview }: RunStatusBannerProps) {
  const completion = run?.completion ?? null;
  const active = ['queued', 'running', 'needs_attention', 'stopping'].includes(taskState);
  return (
    <section
      aria-label="Run status"
      className="h-24 overflow-hidden border-b border-[var(--color-border)] px-4 py-3"
    >
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
        <div className="mt-2 min-w-0 text-sm">
          <p className="overflow-hidden text-ellipsis whitespace-nowrap" title={completion.summary}>
            {completion.summary}
          </p>
          <div className="mt-1 flex min-w-0 items-center gap-3 overflow-x-auto text-xs text-[var(--color-text-secondary)]">
            {completion.metrics === null ? (
              <p className="shrink-0">Metrics unavailable</p>
            ) : (
              <ul aria-label="Run metrics" className="flex shrink-0 gap-3">
                {completion.metrics.map((metric, index) => (
                  <li
                    className="whitespace-nowrap tabular-nums"
                    key={`${metric.name}-${String(index)}`}
                  >
                    {formatMetric(metric)}
                  </li>
                ))}
              </ul>
            )}
            <p className="shrink-0 whitespace-nowrap">
              {completion.checks === null
                ? 'Checks unavailable'
                : `${String(completion.checks.filter((check) => check.passed).length)} of ${String(completion.checks.length)} checks passed`}
            </p>
            {completion.workspace_changes === null ? (
              <p className="shrink-0 whitespace-nowrap">Workspace changes unavailable</p>
            ) : (
              <button
                className="shrink-0 font-medium text-[var(--color-accent)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
                onClick={() => {
                  onOpenReview(taskId);
                }}
                type="button"
              >
                View changes
              </button>
            )}
          </div>
        </div>
      ) : run === null ? null : (
        <p className="mt-2 overflow-hidden text-ellipsis whitespace-nowrap text-xs text-[var(--color-text-secondary)]">
          Run in progress
        </p>
      )}
    </section>
  );
}
