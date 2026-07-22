import { FileText } from 'lucide-react';

import type { MessageProjection } from '../../api/generated';
import { SafeMarkdown } from '../../components/content/SafeMarkdown';
import { RequestContextTrigger } from './RequestContextTrigger';

interface ChatMessageProps {
  message: MessageProjection;
  taskId: string;
  onOpenFile: (workspaceId: string, relativePath: string) => void;
  onOpenRequestContext: (taskId: string, requestId: string) => void;
}

export function ChatMessage({
  message,
  taskId,
  onOpenFile,
  onOpenRequestContext,
}: ChatMessageProps) {
  const user = message.role === 'user';
  return (
    <article className={user ? 'ml-auto max-w-[min(85%,42rem)]' : 'w-full'}>
      <div
        className={
          user
            ? 'rounded-[var(--radius-md)] bg-[var(--color-surface-raised)] px-4 py-3 text-[15px] leading-7 text-[var(--color-text-primary)]'
            : 'min-w-0 text-[15px] leading-7 text-[var(--color-text-primary)]'
        }
      >
        {user ? (
          <p className="whitespace-pre-wrap break-words">{message.text}</p>
        ) : (
          <SafeMarkdown source={message.text} />
        )}
      </div>
      {message.request_ids.length > 0 ? (
        <div className="mt-2 flex flex-wrap gap-2">
          {message.request_ids.map((requestId, index) => (
            <RequestContextTrigger
              index={index}
              key={requestId}
              onOpen={onOpenRequestContext}
              requestId={requestId}
              taskId={taskId}
              total={message.request_ids.length}
            />
          ))}
        </div>
      ) : null}
      {message.file_references.length > 0 ? (
        <div className="mt-2 flex flex-wrap gap-2">
          {message.file_references.map((reference) => (
            <button
              aria-label={`Open ${reference.label}`}
              className="inline-flex items-center gap-1 rounded-[var(--radius-sm)] border border-[var(--color-border)] px-2 py-1 text-xs hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
              key={`${reference.workspace_id}:${reference.relative_path}`}
              onClick={() => {
                onOpenFile(reference.workspace_id, reference.relative_path);
              }}
              title={`Open ${reference.label}`}
              type="button"
            >
              <FileText aria-hidden="true" size={13} />
              {reference.label}
            </button>
          ))}
        </div>
      ) : null}
    </article>
  );
}
