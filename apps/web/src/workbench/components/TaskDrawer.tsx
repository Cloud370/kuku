import { useEffect, useRef, type ReactNode } from 'react';
import { X } from 'lucide-react';

interface TaskDrawerProps {
  children: ReactNode;
  open: boolean;
  onClose: () => void;
}

const FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function TaskDrawer({ children, open, onClose }: TaskDrawerProps) {
  const drawerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const restoreTarget =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const drawer = drawerRef.current;
    const focusable = () => Array.from(drawer?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []);
    focusable()[0]?.focus();

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== 'Tab') return;
      const controls = focusable();
      const first = controls[0];
      const last = controls.at(-1);
      if (first === undefined || last === undefined) return;
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }

    document.addEventListener('keydown', handleKeyDown);
    return () => {
      document.removeEventListener('keydown', handleKeyDown);
      restoreTarget?.focus();
    };
  }, [onClose, open]);

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
