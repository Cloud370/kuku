import type { ContextHealth, RequestSummary, UsageSummary } from '../../api/generated';

import styles from './ContextPanel.module.css';

interface ContextSummaryProps {
  health: ContextHealth;
  historical: boolean;
  selectedRequest: RequestSummary | null;
  usage: UsageSummary;
}

function formatTokens(value: number | null): string {
  return value === null ? 'Token usage unavailable' : `${value.toLocaleString('en-US')} tokens`;
}

function percentage(value: number | null): string {
  if (value === null) return '--';
  return `${String(Math.round(Math.max(0, Math.min(1, value)) * 100))}%`;
}

function contextPercentage(health: ContextHealth): string {
  if (
    health.context_tokens_used === null ||
    health.context_token_limit === null ||
    health.context_token_limit <= 0
  ) {
    return '--';
  }
  return percentage(health.context_tokens_used / health.context_token_limit);
}

function Metric({ ariaLabel, label, value }: { ariaLabel: string; label: string; value: string }) {
  return (
    <div aria-label={ariaLabel} className="min-w-0 px-2 first:pl-0 last:pr-8">
      <span className="block truncate text-[10px] font-medium uppercase text-[var(--color-text-muted)]">
        {label}
      </span>
      <span className="mt-0.5 block truncate text-base font-semibold tabular-nums">{value}</span>
    </div>
  );
}

export function ContextSummary({ health, historical, selectedRequest, usage }: ContextSummaryProps) {
  return (
    <div className={styles.summary}>
      <div className="flex min-w-0 items-center justify-between gap-3">
        <p className="truncate text-sm font-medium">
          {historical ? 'Historical Request' : 'Current Context'}
        </p>
        <span
          className={`shrink-0 rounded-[var(--radius-sm)] px-2 py-0.5 text-xs ${
            health.level === 'warning'
              ? 'bg-[var(--color-warning)] text-yellow-300'
              : 'bg-[var(--color-surface-hover)] text-[var(--color-text-secondary)]'
          }`}
        >
          {health.level}
        </span>
      </div>
      <div className="mt-1 flex min-w-0 items-center justify-between gap-3 text-xs text-[var(--color-text-secondary)]">
        <span className="truncate font-mono">
          {selectedRequest?.request_id ?? 'No provider Request'}
        </span>
        <span className="shrink-0 tabular-nums">{formatTokens(health.context_tokens_used)}</span>
      </div>
      <div className="mt-3 grid grid-cols-3 divide-x divide-[var(--color-border)] border-t border-[var(--color-border)] pt-2">
        <Metric ariaLabel="Context usage" label="Context" value={contextPercentage(health)} />
        <Metric
          ariaLabel="Cache hit rate"
          label="Cache hit"
          value={percentage(usage.cached_input_ratio)}
        />
        <Metric
          ariaLabel="Task request count"
          label="Reqs"
          value={usage.request_count.toLocaleString('en-US')}
        />
      </div>
    </div>
  );
}
