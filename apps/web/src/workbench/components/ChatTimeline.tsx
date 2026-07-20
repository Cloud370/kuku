import type { TaskProjection, TimelineItemProjection } from '../../api/generated';
import type { TimelineHistory } from '../state';
import { ActivityItem } from './ActivityItem';
import { ChatMessage } from './ChatMessage';
import { InteractionCard } from './InteractionCard';
import { RunStatusBanner } from './RunStatusBanner';
import { VirtualTimeline } from './VirtualTimeline';

export interface ChatTimelineProps {
  loadOlder: () => Promise<void> | void;
  onOpenAgentThread: (taskId: string, conversationId: string) => void;
  onOpenFile: (workspaceId: string, relativePath: string) => void;
  onOpenRequestContext: (taskId: string, requestId: string) => void;
  onOpenReview: (taskId: string) => void;
  onRespond: (taskId: string, interactionId: string, choiceId: string) => void;
  onReturnToRecent: () => void;
  projection: TaskProjection | null;
  timelineHistory: TimelineHistory;
  timelineItems: TimelineItemProjection[];
}

function itemId(item: TimelineItemProjection): string {
  if (item.type === 'message') return item.item.message_id;
  if (item.type === 'activity') return item.item.activity_id;
  return item.item.interaction_id;
}

export function ChatTimeline({
  loadOlder,
  onOpenAgentThread,
  onOpenFile,
  onOpenRequestContext,
  onOpenReview,
  onRespond,
  onReturnToRecent,
  projection,
  timelineHistory,
  timelineItems,
}: ChatTimelineProps) {
  if (projection === null) {
    return (
      <section aria-label="Conversation" className="grid min-h-full place-items-center p-6">
        <p className="text-sm text-[var(--color-text-secondary)]">
          Choose a Task to start chatting.
        </p>
      </section>
    );
  }
  const taskId = projection.task.task_id;
  const run = projection.active_run ?? projection.latest_run;

  return (
    <section aria-label="Conversation" className="min-w-0">
      <RunStatusBanner
        onOpenReview={onOpenReview}
        run={run}
        taskId={taskId}
        taskState={projection.task.state}
      />
      {timelineHistory.nextCursor !== null ? (
        <div className="px-4 pt-4 text-center">
          <button
            aria-label="Load earlier messages"
            className="rounded-[var(--radius-sm)] border border-[var(--color-border)] px-3 py-1.5 text-xs font-medium hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)] disabled:opacity-40"
            disabled={timelineHistory.phase === 'loading'}
            onClick={() => void loadOlder()}
            type="button"
          >
            {timelineHistory.phase === 'loading'
              ? 'Loading earlier messages'
              : 'Load earlier messages'}
          </button>
        </div>
      ) : null}
      {timelineHistory.phase === 'error' ? (
        <p className="px-4 pt-3 text-center text-sm text-[var(--color-error)]" role="alert">
          Earlier messages could not be loaded. Try again.
        </p>
      ) : null}
      {timelineItems.length === 0 ? (
        <p className="p-6 text-center text-sm text-[var(--color-text-secondary)]">
          No messages yet.
        </p>
      ) : (
        <VirtualTimeline
          gapAfter={timelineHistory.gapAfter}
          gapAfterIndex={timelineHistory.items.length}
          getItemId={itemId}
          items={timelineItems}
          maxMountedRows={120}
          onReturnToRecent={onReturnToRecent}
          renderItem={(item) => {
            if (item.type === 'message') {
              return (
                <ChatMessage
                  message={item.item}
                  onOpenFile={onOpenFile}
                  onOpenRequestContext={onOpenRequestContext}
                  taskId={taskId}
                />
              );
            }
            if (item.type === 'activity') {
              return (
                <ActivityItem
                  activity={item.item}
                  onOpenAgentThread={onOpenAgentThread}
                  onOpenFile={onOpenFile}
                  taskId={taskId}
                />
              );
            }
            return (
              <InteractionCard interaction={item.item} onRespond={onRespond} taskId={taskId} />
            );
          }}
        />
      )}
    </section>
  );
}
