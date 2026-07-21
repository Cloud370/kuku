import { X } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';

import { webApi } from '../../api/client';
import type { AgentThread, ApiError, ConversationId, RequestId, TaskId } from '../../api/generated';
import { SafeMarkdown } from '../../components/content/SafeMarkdown';
import { activateFocusTrap } from '../../workbench/accessibility/focusTrap';
import { RequestLinks } from './RequestLinks';
import { selectAgentThread, type AgentThreadView } from './agentThreadSelectors';
import styles from './ContextPanel.module.css';
import { toApiError } from './contextState';

interface AgentThreadDialogProps {
  taskId: TaskId;
  conversationId: ConversationId;
  open: boolean;
  onClose: () => void;
  onSelectRequest: (requestId: RequestId) => void;
  thread?: AgentThread;
}

type ThreadState =
  | { kind: 'loading' }
  | { kind: 'error'; error: ApiError }
  | { kind: 'ready'; thread: AgentThread };

function ThreadMessages({
  view,
  onSelectRequest,
}: {
  view: AgentThreadView;
  onSelectRequest: (requestId: RequestId) => void;
}) {
  return (
    <div>
      {view.messagesTruncatedBefore ? (
        <p className="mb-3 border border-[var(--color-warning-border)] bg-[var(--color-warning)] p-2 text-xs">
          Earlier delegated messages are not shown
        </p>
      ) : null}
      {view.messages.length === 0 ? (
        <p className="text-sm text-[var(--color-text-secondary)]">No delegated messages.</p>
      ) : (
        <ol className="space-y-4">
          {view.messages.map((message) => (
            <li className="border-b border-[var(--color-border)] pb-4" key={message.message_id}>
              <p className="mb-2 text-xs font-medium text-[var(--color-text-muted)]">
                {message.role}
              </p>
              {message.role === 'agent' ? (
                <SafeMarkdown source={message.text} />
              ) : (
                <p className="whitespace-pre-wrap break-words text-sm">{message.text}</p>
              )}
              <div className="mt-2">
                <RequestLinks onSelect={onSelectRequest} requestIds={message.request_ids} />
              </div>
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}

export function AgentThreadDialog({
  taskId,
  conversationId,
  open,
  onClose,
  onSelectRequest,
  thread,
}: AgentThreadDialogProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const [state, setState] = useState<ThreadState>(
    thread === undefined ? { kind: 'loading' } : { kind: 'ready', thread },
  );

  useEffect(() => {
    if (!open || thread !== undefined) {
      if (thread !== undefined) setState({ kind: 'ready', thread });
      return;
    }
    let active = true;
    setState({ kind: 'loading' });
    void webApi.context
      .agentThread(taskId, conversationId)
      .then((nextThread) => {
        if (active) setState({ kind: 'ready', thread: nextThread });
      })
      .catch((error: unknown) => {
        if (active) setState({ kind: 'error', error: toApiError(error) });
      });
    return () => {
      active = false;
    };
  }, [conversationId, open, taskId, thread]);

  useEffect(() => {
    if (!open || dialogRef.current === null) return;
    const restoreTarget =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    return activateFocusTrap(dialogRef.current, onClose, restoreTarget);
  }, [onClose, open]);

  if (!open) return null;
  const view = state.kind === 'ready' ? selectAgentThread(state.thread) : null;
  return (
    <div className={styles.dialogBackdrop} onMouseDown={onClose}>
      <div
        aria-label="Agent thread"
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
            <h2 className="truncate text-sm font-semibold">{view?.identity ?? 'Agent thread'}</h2>
            <p className="text-xs text-[var(--color-text-muted)]">
              {view === null
                ? conversationId
                : `${view.status} · ${view.resultInMain ? 'Result in main' : 'Separate result'}`}
            </p>
          </div>
          <button
            aria-label="Close Agent thread"
            className="inline-flex size-8 shrink-0 items-center justify-center rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
            onClick={onClose}
            title="Close Agent thread"
            type="button"
          >
            <X aria-hidden="true" size={16} />
          </button>
        </header>
        <div className={styles.dialogBody}>
          {state.kind === 'loading' ? (
            <p className="text-sm text-[var(--color-text-secondary)]" role="status">
              Loading Agent thread
            </p>
          ) : state.kind === 'error' ? (
            <div role="alert">
              <p className="text-sm font-medium">{state.error.code}</p>
              <p className="mt-1 text-xs text-[var(--color-text-secondary)]">
                {state.error.message}
              </p>
            </div>
          ) : (
            <ThreadMessages
              onSelectRequest={onSelectRequest}
              view={selectAgentThread(state.thread)}
            />
          )}
        </div>
      </div>
    </div>
  );
}
