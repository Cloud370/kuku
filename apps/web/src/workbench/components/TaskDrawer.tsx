import { useEffect, useRef, type ReactNode, type RefObject } from 'react';
import { X } from 'lucide-react';

import { activateFocusTrap } from '../accessibility/focusTrap';

interface TaskDrawerProps {
  children: ReactNode;
  contentRole?: 'complementary' | 'navigation';
  label?: string;
  open: boolean;
  onClose: () => void;
  returnFocusRef?: RefObject<HTMLElement | null>;
  side?: 'left' | 'right';
}

export function TaskDrawer({
  children,
  contentRole = 'navigation',
  label = 'Tasks',
  open,
  onClose,
  returnFocusRef,
  side = 'left',
}: TaskDrawerProps) {
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
        aria-label={label}
        aria-modal="true"
        className={`h-full w-[min(88vw,22rem)] border-[var(--color-border)] bg-[var(--color-surface-raised)] shadow-xl ${side === 'right' ? 'ml-auto border-l' : 'border-r'}`}
        onMouseDown={(event) => {
          event.stopPropagation();
        }}
        ref={drawerRef}
        role="dialog"
        data-reduced-motion={window.matchMedia('(prefers-reduced-motion: reduce)').matches}
      >
        <div className="flex h-12 items-center justify-between border-b border-[var(--color-border)] px-3">
          <h2 className="text-sm font-semibold">{label}</h2>
          <button
            aria-label={`Close ${label}`}
            className="inline-flex size-9 items-center justify-center rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
            onClick={onClose}
            title={`Close ${label}`}
            type="button"
          >
            <X aria-hidden="true" size={18} />
          </button>
        </div>
        {contentRole === 'navigation' ? (
          <nav aria-label={label} className="h-[calc(100%-3rem)] overflow-hidden p-3">
            {children}
          </nav>
        ) : (
          <aside aria-label={label} className="h-[calc(100%-3rem)] overflow-hidden p-3">
            {children}
          </aside>
        )}
      </div>
    </div>
  );
}
