import { ChevronDown } from 'lucide-react';
import { useState } from 'react';

import type { RequestId } from '../../api/generated';

export interface RequestLinksProps {
  requestIds: RequestId[];
  onSelect: (id: RequestId) => void;
}

export function RequestLinks({ requestIds, onSelect }: RequestLinksProps) {
  const [expandedRequestIds, setExpandedRequestIds] = useState<RequestId[]>([]);
  if (requestIds.length === 0) return null;
  return (
    <div aria-label="Provider Requests" className="space-y-1.5">
      {requestIds.map((requestId, index) => {
        const expanded = expandedRequestIds.includes(requestId);
        const requestNumber = String(index + 1);
        const detailId = `linked-request-${requestNumber}-details`;
        return (
          <div className="max-w-full" key={requestId}>
            <div className="inline-grid max-w-full grid-cols-[minmax(0,1fr)_2rem] rounded-[var(--radius-sm)] border border-[var(--color-border)]">
              <button
                aria-label={`Select Request ${requestNumber}`}
                className="truncate px-2 py-1 text-xs text-[var(--color-text-secondary)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
                onClick={() => {
                  onSelect(requestId);
                }}
                type="button"
              >
                Request {requestNumber}
              </button>
              <button
                aria-controls={detailId}
                aria-expanded={expanded}
                aria-label={`${expanded ? 'Hide' : 'Show'} Request ${requestNumber} details`}
                className="inline-flex size-8 items-center justify-center border-l border-[var(--color-border)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
                onClick={() => {
                  setExpandedRequestIds((current) =>
                    current.includes(requestId)
                      ? current.filter((value) => value !== requestId)
                      : [...current, requestId],
                  );
                }}
                title={`${expanded ? 'Hide' : 'Show'} Request ${requestNumber} details`}
                type="button"
              >
                <ChevronDown
                  aria-hidden="true"
                  className={`transition-transform motion-reduce:transition-none ${expanded ? 'rotate-180' : ''}`}
                  size={14}
                />
              </button>
            </div>
            {expanded ? (
              <p
                className="mt-1 break-all font-mono text-xs text-[var(--color-text-muted)]"
                id={detailId}
              >
                {requestId}
              </p>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}
