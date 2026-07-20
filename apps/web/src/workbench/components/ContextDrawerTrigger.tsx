import { PanelRightOpen } from 'lucide-react';
import { forwardRef } from 'react';

interface ContextDrawerTriggerProps {
  onOpen: () => void;
}

export const ContextDrawerTrigger = forwardRef<HTMLButtonElement, ContextDrawerTriggerProps>(
  function ContextDrawerTrigger({ onOpen }, ref) {
    return (
      <button
        aria-label="Open Agent Context"
        className="inline-flex size-9 items-center justify-center rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
        onClick={onOpen}
        ref={ref}
        title="Open Agent Context"
        type="button"
      >
        <PanelRightOpen aria-hidden="true" size={18} />
      </button>
    );
  },
);
