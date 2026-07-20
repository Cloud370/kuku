interface RequestContextTriggerProps {
  index: number;
  requestId: string;
  taskId: string;
  total: number;
  onOpen: (taskId: string, requestId: string) => void;
}

export function RequestContextTrigger({
  index,
  requestId,
  taskId,
  total,
  onOpen,
}: RequestContextTriggerProps) {
  const label = `Inspect Request context ${String(index + 1)} of ${String(total)}`;
  return (
    <button
      aria-label={label}
      className="rounded-[var(--radius-sm)] border border-[var(--color-border)] px-2 py-1 font-mono text-xs text-[var(--color-text-secondary)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
      onClick={() => {
        onOpen(taskId, requestId);
      }}
      title={label}
      type="button"
    >
      Request {String(index + 1)}
    </button>
  );
}
