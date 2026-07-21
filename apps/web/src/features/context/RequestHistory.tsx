import { History } from 'lucide-react';

import type { RequestId, RequestSummary } from '../../api/generated';
import styles from './ContextPanel.module.css';

interface RequestHistoryProps {
  requests: RequestSummary[];
  selectedRequestId: RequestId | null;
  truncated: boolean;
  onSelect: (requestId: RequestId) => void;
}

export function RequestHistory({
  requests,
  selectedRequestId,
  truncated,
  onSelect,
}: RequestHistoryProps) {
  return (
    <div className={styles.history}>
      <History aria-hidden="true" className="shrink-0 text-[var(--color-text-muted)]" size={14} />
      {truncated ? (
        <span className="shrink-0 text-xs text-[var(--color-text-secondary)]">
          Earlier Requests are available from Chat
        </span>
      ) : null}
      {requests.map((request) => {
        const selected = request.request_id === selectedRequestId;
        return (
          <button
            aria-label={`Select Request ${request.request_id}`}
            aria-pressed={selected}
            className={`shrink-0 rounded-[var(--radius-sm)] border px-2 py-1 font-mono text-xs focus-visible:outline-2 focus-visible:outline-[var(--color-accent)] ${
              selected
                ? 'border-[var(--color-accent)] bg-[var(--color-accent-muted)]'
                : 'border-[var(--color-border)] hover:bg-[var(--color-surface-hover)]'
            }`}
            key={request.request_id}
            onClick={() => {
              onSelect(request.request_id);
            }}
            title={`${request.model} · ${request.status}`}
            type="button"
          >
            {request.request_id}
          </button>
        );
      })}
    </div>
  );
}
