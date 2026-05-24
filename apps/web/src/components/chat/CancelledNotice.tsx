export type CancelledNoticeProps = {
  turnNumber?: number;
};

export function CancelledNotice({ turnNumber }: CancelledNoticeProps) {
  return (
    <div className="flex items-center gap-2 my-2 px-3 py-1.5 rounded-[var(--radius-md)] bg-[var(--color-surface)] border border-[var(--color-border)] text-[var(--text-xs)] text-[var(--color-text-muted)]">
      <span className="shrink-0">&#x26D4;</span>
      {turnNumber !== undefined ? (
        <span>
          Turn {turnNumber} cancelled
        </span>
      ) : (
        <span>Cancelled</span>
      )}
    </div>
  );
}
