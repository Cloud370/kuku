import type { ContextHealth, RequestSummary } from '../../api/generated';

import styles from './ContextPanel.module.css';

interface ContextSummaryProps {
  health: ContextHealth;
  historical: boolean;
  selectedRequest: RequestSummary | null;
}

function formatTokens(value: number | null): string {
  return value === null ? 'Token usage unavailable' : `${value.toLocaleString('en-US')} tokens`;
}

export function ContextSummary({ health, historical, selectedRequest }: ContextSummaryProps) {
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
    </div>
  );
}
