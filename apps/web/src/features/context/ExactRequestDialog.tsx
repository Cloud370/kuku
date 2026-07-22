import { ChevronDown, Check, Copy, X } from 'lucide-react';
import { useEffect, useRef, useState, type ReactNode, type RefObject } from 'react';

import type { ExactContentBlock, ExactRequest } from '../../api/generated';
import { SafeCodeBlock } from '../../components/content/SafeCodeBlock';
import { activateFocusTrap } from '../../workbench/accessibility/focusTrap';
import styles from './ContextPanel.module.css';

interface ExactRequestDialogProps {
  exactPayloadHash: string | null;
  exactRequest: ExactRequest;
  open: boolean;
  onClose: () => void;
  returnFocusRef?: RefObject<HTMLElement | null>;
}

function hasClipboard(value: unknown): value is Pick<Clipboard, 'writeText'> {
  return (
    typeof value === 'object' &&
    value !== null &&
    'writeText' in value &&
    typeof value.writeText === 'function'
  );
}

function formatThinking(request: ExactRequest): string {
  if (request.parameters.thinking.kind === 'disabled') return 'Thinking disabled';
  if (request.parameters.thinking.kind === 'adaptive') return 'Thinking adaptive';
  const budget = request.parameters.thinking.budget_tokens;
  return budget === null
    ? 'Thinking enabled'
    : `Thinking · ${budget.toLocaleString('en-US')} tokens`;
}

function ExactBlock({ block }: { block: ExactContentBlock }) {
  if (block.kind === 'text' || block.kind === 'thinking') {
    return <SafeCodeBlock code={block.text} />;
  }
  if (block.kind === 'tool_use') {
    return (
      <div className="mt-2">
        <div className="mb-1 flex items-center gap-2 text-xs">
          <span className="font-medium">{block.name}</span>
          <span className="text-[var(--color-text-muted)]">{block.tool_call_id}</span>
        </div>
        <SafeCodeBlock code={JSON.stringify(block.input, null, 2)} language="json" />
      </div>
    );
  }
  return (
    <div className="mt-2">
      <div className="mb-1 flex items-center gap-2 text-xs">
        <span className="font-medium">Tool result · {block.status}</span>
        {block.truncated ? <span className="text-yellow-300">Truncated</span> : null}
      </div>
      <SafeCodeBlock code={block.content} />
    </div>
  );
}

function CollapsibleSection({
  children,
  label,
  count,
}: {
  children: ReactNode;
  label: string;
  count: number;
}) {
  const [open, setOpen] = useState(true);
  return (
    <section
      aria-label={label}
      className="border-b border-[var(--color-border)] pb-3 last:border-b-0"
      role="group"
    >
      <button
        aria-expanded={open}
        aria-label={`${open ? 'Collapse' : 'Expand'} ${label}`}
        className="flex min-h-9 w-full items-center justify-between gap-2 text-left hover:text-[var(--color-accent)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
        onClick={() => {
          setOpen((current) => !current);
        }}
        type="button"
      >
        <span className="flex min-w-0 items-center gap-2 text-xs font-semibold">
          <span>{label}</span>
          <span className="font-normal tabular-nums text-[var(--color-text-muted)]">{count}</span>
        </span>
        <ChevronDown
          aria-hidden="true"
          className={`shrink-0 transition-transform motion-reduce:transition-none ${open ? 'rotate-180' : ''}`}
          size={15}
        />
      </button>
      {open ? <div className="pt-1">{children}</div> : null}
    </section>
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
  const [copyStatus, setCopyStatus] = useState<'idle' | 'copied' | 'unavailable'>('idle');
  const exactRequestJson = JSON.stringify(exactRequest, null, 2);

  async function copyRequest() {
    const clipboardValue = (navigator as unknown as { clipboard?: unknown }).clipboard;
    if (!hasClipboard(clipboardValue)) {
      setCopyStatus('unavailable');
      return;
    }
    try {
      await clipboardValue.writeText(exactRequestJson);
      setCopyStatus('copied');
    } catch {
      setCopyStatus('unavailable');
    }
  }

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
          <div className="flex shrink-0 items-center gap-1">
            <button
              aria-label="Copy request JSON"
              className="inline-flex size-8 items-center justify-center rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
              onClick={() => void copyRequest()}
              title="Copy request JSON"
              type="button"
            >
              {copyStatus === 'copied' ? (
                <Check aria-hidden="true" size={15} />
              ) : (
                <Copy aria-hidden="true" size={15} />
              )}
            </button>
            <button
              aria-label="Close Exact Request"
              className="inline-flex size-8 items-center justify-center rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
              onClick={onClose}
              title="Close Exact Request"
              type="button"
            >
              <X aria-hidden="true" size={16} />
            </button>
          </div>
        </header>
        <div className={styles.dialogBody}>
          <div aria-label="Request metadata" className="mb-4 grid grid-cols-2 gap-2 sm:grid-cols-3">
            <span className="rounded-[var(--radius-sm)] bg-[var(--color-surface)] px-2 py-1.5 text-xs">
              <span className="block text-[var(--color-text-muted)]">Model</span>
              <span className="mt-0.5 block truncate font-medium">
                {exactRequest.parameters.model}
              </span>
            </span>
            <span className="rounded-[var(--radius-sm)] bg-[var(--color-surface)] px-2 py-1.5 text-xs">
              <span className="block text-[var(--color-text-muted)]">Mode</span>
              <span className="mt-0.5 block font-medium">
                {exactRequest.parameters.stream ? 'Streaming' : 'Non-streaming'}
              </span>
            </span>
            <span className="rounded-[var(--radius-sm)] bg-[var(--color-surface)] px-2 py-1.5 text-xs">
              <span className="block text-[var(--color-text-muted)]">Temperature</span>
              <span className="mt-0.5 block font-medium">
                {exactRequest.parameters.temperature === null
                  ? 'Auto'
                  : String(exactRequest.parameters.temperature)}
              </span>
            </span>
            <span className="rounded-[var(--radius-sm)] bg-[var(--color-surface)] px-2 py-1.5 text-xs">
              <span className="block text-[var(--color-text-muted)]">Thinking</span>
              <span className="mt-0.5 block truncate font-medium">
                {formatThinking(exactRequest)}
              </span>
            </span>
            <span className="rounded-[var(--radius-sm)] bg-[var(--color-surface)] px-2 py-1.5 text-xs">
              <span className="block text-[var(--color-text-muted)]">Messages</span>
              <span className="mt-0.5 block font-medium">
                {exactRequest.messages.length} messages
              </span>
            </span>
            <span className="rounded-[var(--radius-sm)] bg-[var(--color-surface)] px-2 py-1.5 text-xs">
              <span className="block text-[var(--color-text-muted)]">Tools</span>
              <span className="mt-0.5 block font-medium">
                {exactRequest.tools.length} {exactRequest.tools.length === 1 ? 'tool' : 'tools'}
              </span>
            </span>
          </div>
          <div className="space-y-3">
            <CollapsibleSection count={1} label="Parameters">
              <SafeCodeBlock
                code={JSON.stringify(exactRequest.parameters, null, 2)}
                language="json"
              />
            </CollapsibleSection>
            <CollapsibleSection count={exactRequest.messages.length} label="Messages">
              {exactRequest.messages.map((message, messageIndex) => (
                <article
                  className="mt-3 border-l-2 border-[var(--color-accent)] pl-3 first:mt-0"
                  key={`${message.role}:${String(messageIndex)}`}
                >
                  <div className="flex items-center gap-2 text-xs">
                    <span className="font-semibold uppercase tracking-wide">{message.role}</span>
                    <span className="text-[var(--color-text-muted)]">
                      {message.content.length} {message.content.length === 1 ? 'block' : 'blocks'}
                    </span>
                  </div>
                  {message.content.map((block, blockIndex) => (
                    <div className="mt-2" key={`${block.kind}:${String(blockIndex)}`}>
                      <span className="text-[10px] font-medium uppercase tracking-wide text-[var(--color-text-muted)]">
                        {block.kind}
                      </span>
                      <ExactBlock block={block} />
                    </div>
                  ))}
                </article>
              ))}
            </CollapsibleSection>
            <CollapsibleSection count={exactRequest.tools.length} label="Tools">
              {exactRequest.tools.length === 0 ? (
                <p className="text-xs text-[var(--color-text-muted)]">No tools</p>
              ) : (
                exactRequest.tools.map((tool) => (
                  <article className="mt-3 first:mt-0" key={tool.name}>
                    <div className="flex items-baseline justify-between gap-2">
                      <h4 className="text-xs font-medium">{tool.name}</h4>
                      <span className="text-[10px] text-[var(--color-text-muted)]">
                        input schema
                      </span>
                    </div>
                    <p className="mt-1 text-xs text-[var(--color-text-secondary)]">
                      {tool.description}
                    </p>
                    <SafeCodeBlock
                      code={JSON.stringify(tool.input_schema, null, 2)}
                      language="json"
                    />
                  </article>
                ))
              )}
            </CollapsibleSection>
          </div>
          {copyStatus !== 'idle' ? (
            <span className="sr-only" role="status">
              {copyStatus === 'copied' ? 'Request JSON copied' : 'Request JSON copy unavailable'}
            </span>
          ) : null}
        </div>
      </div>
    </div>
  );
}
