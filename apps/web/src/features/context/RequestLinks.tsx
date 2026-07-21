import type { RequestId } from '../../api/generated';

export interface RequestLinksProps {
  requestIds: RequestId[];
  onSelect: (id: RequestId) => void;
}

export function RequestLinks({ requestIds, onSelect }: RequestLinksProps) {
  if (requestIds.length === 0) return null;
  return (
    <div aria-label="Provider Requests" className="flex flex-wrap gap-1.5">
      {requestIds.map((requestId) => (
        <button
          aria-label={`Request ${requestId}`}
          className="max-w-full truncate rounded-[var(--radius-sm)] border border-[var(--color-border)] px-2 py-1 font-mono text-xs text-[var(--color-text-secondary)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
          key={requestId}
          onClick={() => {
            onSelect(requestId);
          }}
          title={requestId}
          type="button"
        >
          {requestId}
        </button>
      ))}
    </div>
  );
}
