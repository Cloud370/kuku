import { ChevronDown, History } from 'lucide-react';
import { useState } from 'react';

import type { ProviderFact, RequestId, RequestStatus, RequestSummary } from '../../api/generated';
import styles from './ContextPanel.module.css';

interface RequestHistoryProps {
  requests: RequestSummary[];
  selectedRequestId: RequestId | null;
  truncated: boolean;
  onSelect: (requestId: RequestId) => void;
}

const COLLAPSED_REQUEST_COUNT = 5;

const STATUS_LABELS: Record<RequestStatus, string> = {
  completed: 'Completed',
  failed: 'Failed',
  started: 'In progress',
};

const PROVIDER_LABELS: Record<ProviderFact['kind'], string> = {
  anthropic: 'Anthropic',
  open_ai_compatible: 'OpenAI compatible',
  open_ai_responses: 'OpenAI Responses',
};

function formatStartedAt(value: string): string | null {
  const startedAt = new Date(value);
  if (Number.isNaN(startedAt.getTime())) return null;
  return new Intl.DateTimeFormat('en-US', {
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
    month: 'short',
    timeZone: 'UTC',
  }).format(startedAt);
}

export function RequestHistory({
  requests,
  selectedRequestId,
  truncated,
  onSelect,
}: RequestHistoryProps) {
  const [expandedRequestIds, setExpandedRequestIds] = useState<RequestId[]>([]);
  const [showAll, setShowAll] = useState(false);
  const numberedRequests = requests
    .map((request, index) => ({ number: index + 1, request }))
    .reverse();
  let visibleRequests = numberedRequests;
  if (!showAll && numberedRequests.length > COLLAPSED_REQUEST_COUNT) {
    visibleRequests = numberedRequests.slice(0, COLLAPSED_REQUEST_COUNT);
    const selected = numberedRequests.find(
      ({ request }) => request.request_id === selectedRequestId,
    );
    if (
      selected !== undefined &&
      !visibleRequests.some(({ request }) => request.request_id === selected.request.request_id)
    ) {
      visibleRequests = [...visibleRequests.slice(0, COLLAPSED_REQUEST_COUNT - 1), selected].sort(
        (left, right) => right.number - left.number,
      );
    }
  }
  const requestCountLabel = `${String(requests.length)} ${requests.length === 1 ? 'Request' : 'Requests'}`;
  return (
    <section aria-label="Request history" className={styles.history}>
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <History aria-hidden="true" className="text-[var(--color-text-muted)]" size={14} />
          <h3 className="text-xs font-semibold">Request history</h3>
        </div>
        <span className="text-xs tabular-nums text-[var(--color-text-muted)]">
          {requestCountLabel}
        </span>
      </div>
      {truncated ? (
        <p className="mt-1 text-xs text-[var(--color-text-secondary)]">
          Earlier Requests are available from Chat
        </p>
      ) : null}
      <ol className="mt-2 divide-y divide-[var(--color-border)]">
        {visibleRequests.map(({ number, request }) => {
          const selected = request.request_id === selectedRequestId;
          const expanded = expandedRequestIds.includes(request.request_id);
          const requestNumber = String(number);
          const detailId = `request-${requestNumber}-details`;
          const startedAt = formatStartedAt(request.started_at);
          return (
            <li className="py-1.5" key={request.request_id}>
              <div
                className={`grid min-w-0 grid-cols-[minmax(0,1fr)_2rem] items-center rounded-[var(--radius-sm)] ${
                  selected ? 'bg-[var(--color-accent-muted)]' : ''
                }`}
              >
                <button
                  aria-label={`Select Request ${request.request_id}`}
                  aria-pressed={selected}
                  className="grid min-w-0 grid-cols-[auto_auto_minmax(0,1fr)] items-center gap-x-2 gap-y-0.5 px-2 py-1.5 text-left hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
                  onClick={() => {
                    onSelect(request.request_id);
                  }}
                  type="button"
                >
                  <span className="text-xs font-medium">Request {requestNumber}</span>
                  <span className="text-xs text-[var(--color-text-secondary)]">
                    {STATUS_LABELS[request.status]}
                  </span>
                  <span className="col-span-2 col-start-1 truncate text-xs text-[var(--color-text-muted)]">
                    {request.model}
                  </span>
                  {startedAt === null ? null : (
                    <time
                      className="col-start-3 row-start-2 truncate text-right text-xs tabular-nums text-[var(--color-text-muted)]"
                      dateTime={request.started_at}
                    >
                      {startedAt}
                    </time>
                  )}
                </button>
                <button
                  aria-controls={detailId}
                  aria-expanded={expanded}
                  aria-label={`${expanded ? 'Hide' : 'Show'} Request ${requestNumber} details`}
                  className="inline-flex size-8 items-center justify-center rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
                  onClick={() => {
                    setExpandedRequestIds((current) =>
                      current.includes(request.request_id)
                        ? current.filter((requestId) => requestId !== request.request_id)
                        : [...current, request.request_id],
                    );
                  }}
                  title={`${expanded ? 'Hide' : 'Show'} Request ${requestNumber} details`}
                  type="button"
                >
                  <ChevronDown
                    aria-hidden="true"
                    className={`transition-transform motion-reduce:transition-none ${expanded ? 'rotate-180' : ''}`}
                    size={15}
                  />
                </button>
              </div>
              {expanded ? (
                <dl
                  className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 px-2 py-2 text-xs"
                  id={detailId}
                >
                  <dt className="text-[var(--color-text-muted)]">Request ID</dt>
                  <dd className="break-all font-mono">{request.request_id}</dd>
                  <dt className="text-[var(--color-text-muted)]">Provider</dt>
                  <dd>{PROVIDER_LABELS[request.provider.kind]}</dd>
                </dl>
              ) : null}
            </li>
          );
        })}
      </ol>
      {requests.length > COLLAPSED_REQUEST_COUNT ? (
        <button
          aria-label={
            showAll
              ? `Show recent ${String(COLLAPSED_REQUEST_COUNT)} Requests`
              : `Show all ${String(requests.length)} Requests`
          }
          className="mt-2 inline-flex min-h-8 w-full items-center justify-center gap-1.5 rounded-[var(--radius-sm)] text-xs font-medium text-[var(--color-text-secondary)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
          onClick={() => {
            setShowAll((current) => !current);
          }}
          type="button"
        >
          <ChevronDown
            aria-hidden="true"
            className={`transition-transform motion-reduce:transition-none ${showAll ? 'rotate-180' : ''}`}
            size={14}
          />
          {showAll
            ? `Show recent ${String(COLLAPSED_REQUEST_COUNT)}`
            : `Show all ${String(requests.length)}`}
        </button>
      ) : null}
    </section>
  );
}
