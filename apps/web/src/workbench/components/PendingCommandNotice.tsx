import type { PendingCommand } from '../workbenchStore';

interface PendingCommandNoticeProps {
  command: PendingCommand;
  onRetry: () => void;
}

const labels: Record<PendingCommand['kind'], string> = {
  create_task: 'Task creation',
  respond: 'Interaction response',
  stop_run: 'Stop request',
  submit_run: 'Send request',
};

const retryLabels: Record<PendingCommand['kind'], string> = {
  create_task: 'Retry Task creation',
  respond: 'Retry response',
  stop_run: 'Retry stop',
  submit_run: 'Retry send',
};

export function PendingCommandNotice({ command, onRetry }: PendingCommandNoticeProps) {
  return (
    <div
      aria-label="Pending command"
      className="flex items-center gap-2 border-b border-[var(--color-border)] bg-[var(--color-surface-raised)] px-3 py-2 text-xs"
      role="status"
    >
      <span className="min-w-0 flex-1 truncate">
        {labels[command.kind]} outcome is unknown. The original request is preserved.
      </span>
      <button
        className="shrink-0 font-medium text-[var(--color-accent)] underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
        onClick={onRetry}
        type="button"
      >
        {retryLabels[command.kind]}
      </button>
    </div>
  );
}
