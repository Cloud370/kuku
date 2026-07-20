import { useEffect, useRef, type ReactNode, type RefObject } from 'react';
import { X } from 'lucide-react';

import { activateFocusTrap } from '../accessibility/focusTrap';

interface TaskDrawerProps {
  children: ReactNode;
  open: boolean;
  onClose: () => void;
  returnFocusRef?: RefObject<HTMLElement | null>;
}

export function TaskDrawer({ children, open, onClose, returnFocusRef }: TaskDrawerProps) {
  const drawerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const drawer = drawerRef.current;
    if (drawer === null) return;
    const restoreTarget =
      returnFocusRef?.current ??
      (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    return activateFocusTrap(drawer, onClose, restoreTarget);
  }, [onClose, open, returnFocusRef]);

  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 bg-black/40" onMouseDown={onClose}>
      <div
        aria-label="Tasks"
        aria-modal="true"
        className="h-full w-[min(88vw,22rem)] border-r border-[var(--color-border)] bg-[var(--color-surface-raised)] shadow-xl"
        onMouseDown={(event) => {
          event.stopPropagation();
        }}
        ref={drawerRef}
        role="dialog"
        data-reduced-motion={window.matchMedia('(prefers-reduced-motion: reduce)').matches}
      >
        <div className="flex h-12 items-center justify-between border-b border-[var(--color-border)] px-3">
          <h2 className="text-sm font-semibold">Tasks</h2>
          <button
            aria-label="Close Tasks"
            className="inline-flex size-9 items-center justify-center rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
            onClick={onClose}
            title="Close Tasks"
            type="button"
          >
            <X aria-hidden="true" size={18} />
          </button>
        </div>
        <nav aria-label="Tasks" className="h-[calc(100%-3rem)] overflow-y-auto p-3">
          {children}
        </nav>
      </div>
    </div>
  );
}
