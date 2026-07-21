import { X } from 'lucide-react';
import { useEffect, useRef, type RefObject } from 'react';

import type { ExactContentBlock, ExactRequest } from '../../api/generated';
import { activateFocusTrap } from '../../workbench/accessibility/focusTrap';
import styles from './ContextPanel.module.css';

interface ExactRequestDialogProps {
  exactPayloadHash: string | null;
  exactRequest: ExactRequest;
  open: boolean;
  onClose: () => void;
  returnFocusRef?: RefObject<HTMLElement | null>;
}

function ExactBlock({ block }: { block: ExactContentBlock }) {
  if (block.kind === 'text' || block.kind === 'thinking') {
    return (
      <pre className="mt-2 overflow-x-auto whitespace-pre-wrap break-words border border-[var(--color-border)] bg-[var(--color-surface)] p-3 text-xs">
        {block.text}
      </pre>
    );
  }
  if (block.kind === 'tool_use') {
    return (
      <div className="mt-2 border border-[var(--color-border)] bg-[var(--color-surface)] p-3 text-xs">
        <p className="font-medium">{block.name}</p>
        <pre className="mt-2 overflow-x-auto whitespace-pre-wrap break-words">
          {JSON.stringify(block.input, null, 2)}
        </pre>
      </div>
    );
  }
  return (
    <div className="mt-2 border border-[var(--color-border)] bg-[var(--color-surface)] p-3 text-xs">
      <p className="font-medium">Tool result · {block.status}</p>
      <pre className="mt-2 overflow-x-auto whitespace-pre-wrap break-words">{block.content}</pre>
    </div>
  );
}

export function ExactRequestDialog({
  exactPayloadHash,
  exactRequest,
  open,
  onClose,
  returnFocusRef,
}: ExactRequestDialogProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open || dialogRef.current === null) return;
    const restoreTarget =
      returnFocusRef?.current ??
      (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    return activateFocusTrap(dialogRef.current, onClose, restoreTarget);
  }, [onClose, open, returnFocusRef]);

  if (!open) return null;
  return (
    <div className={styles.dialogBackdrop} onMouseDown={onClose}>
      <div
        aria-label="Exact Request"
        aria-modal="true"
        className={styles.dialog}
        onMouseDown={(event) => {
          event.stopPropagation();
        }}
        ref={dialogRef}
        role="dialog"
      >
        <header className={styles.header}>
          <div className="min-w-0">
            <h2 className="text-sm font-semibold">Exact Request</h2>
            {exactPayloadHash === null ? null : (
              <p className="truncate font-mono text-xs text-[var(--color-text-muted)]">
                {exactPayloadHash}
              </p>
            )}
          </div>
          <button
            aria-label="Close Exact Request"
            className="inline-flex size-8 shrink-0 items-center justify-center rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
            onClick={onClose}
            title="Close Exact Request"
            type="button"
          >
            <X aria-hidden="true" size={16} />
          </button>
        </header>
        <div className={styles.dialogBody}>
          <section>
            <h3 className="text-xs font-semibold">Parameters</h3>
            <pre className="mt-2 overflow-x-auto whitespace-pre-wrap break-words border border-[var(--color-border)] bg-[var(--color-surface)] p-3 text-xs">
              {JSON.stringify(exactRequest.parameters, null, 2)}
            </pre>
          </section>
          <section className="mt-4">
            <h3 className="text-xs font-semibold">Messages</h3>
            {exactRequest.messages.map((message, messageIndex) => (
              <article className="mt-3" key={`${message.role}:${String(messageIndex)}`}>
                <h4 className="text-xs font-medium text-[var(--color-text-secondary)]">
                  {message.role}
                </h4>
                {message.content.map((block, blockIndex) => (
                  <ExactBlock block={block} key={`${block.kind}:${String(blockIndex)}`} />
                ))}
              </article>
            ))}
          </section>
          <section className="mt-4">
            <h3 className="text-xs font-semibold">Tools</h3>
            {exactRequest.tools.length === 0 ? (
              <p className="mt-2 text-xs text-[var(--color-text-muted)]">No tools</p>
            ) : (
              exactRequest.tools.map((tool) => (
                <article className="mt-3 border border-[var(--color-border)] p-3" key={tool.name}>
                  <h4 className="text-xs font-medium">{tool.name}</h4>
                  <p className="mt-1 text-xs text-[var(--color-text-secondary)]">
                    {tool.description}
                  </p>
                  <pre className="mt-2 overflow-x-auto whitespace-pre-wrap break-words text-xs">
                    {JSON.stringify(tool.input_schema, null, 2)}
                  </pre>
                </article>
              ))
            )}
          </section>
        </div>
      </div>
    </div>
  );
}
