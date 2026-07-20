import { Square } from 'lucide-react';

interface StopButtonProps {
  disabled?: boolean;
  onStop: () => void;
}

export function StopButton({ disabled = false, onStop }: StopButtonProps) {
  return (
    <button
      aria-label="Stop run"
      className="inline-flex size-9 shrink-0 items-center justify-center border border-[var(--color-error-border)] text-[var(--color-error)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)] disabled:opacity-40"
      disabled={disabled}
      onClick={onStop}
      title="Stop run"
      type="button"
    >
      <Square aria-hidden="true" fill="currentColor" size={14} />
    </button>
  );
}
