import { createPortal } from 'react-dom';
import { useEffect, useRef } from 'react';

import type { InteractionProjection } from '../../api/generated';
import { activateFocusTrap } from '../accessibility/focusTrap';
import { InteractionCard } from './InteractionCard';

interface InteractionDialogProps {
  interaction: InteractionProjection;
  taskId: string;
  onRespond: (taskId: string, interactionId: string, choiceId: string) => Promise<void> | void;
}

export function InteractionDialog({
  interaction,
  taskId,
  onRespond,
}: InteractionDialogProps) {
  const dialogRef = useRef<HTMLElement>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog === null) return;
    const restoreTarget =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    const releaseFocus = activateFocusTrap(dialog, () => undefined, restoreTarget);
    return () => {
      document.body.style.overflow = previousOverflow;
      releaseFocus();
    };
  }, [interaction.interaction_id]);

  return createPortal(
    <div className="fixed inset-0 z-[70] grid place-items-center bg-black/60 p-4">
      <section
        aria-label="Permission required"
        aria-modal="true"
        className="w-full max-w-lg border border-[var(--color-border-strong)] bg-[var(--color-surface-raised)] p-5 shadow-[var(--shadow-elevated)]"
        ref={dialogRef}
        role="dialog"
      >
        <h2 className="mb-4 text-sm font-semibold">Permission required</h2>
        <InteractionCard
          framed={false}
          interaction={interaction}
          onRespond={onRespond}
          taskId={taskId}
        />
      </section>
    </div>,
    document.body,
  );
}
