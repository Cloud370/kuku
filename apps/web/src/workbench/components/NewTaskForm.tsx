import { AlertCircle, LoaderCircle, RotateCcw, X } from 'lucide-react';
import { useState } from 'react';

import type { CreateTaskResponse, WorkspaceId } from '../../api/generated';
import type { PendingCommand, PendingCommandResult } from '../workbenchStore';

type PendingCreate = Extract<PendingCommand, { kind: 'create_task' }>;

export interface NewTaskFormProps {
  workspaceId: WorkspaceId;
  pending: PendingCreate | null;
  createTask: (workspaceId: WorkspaceId) => Promise<CreateTaskResponse>;
  retryPendingCommand: () => Promise<PendingCommandResult>;
  abandonConflictedCommand: () => void;
  onCreated: (taskId: string) => void;
  onReviewTasks: () => void;
  onCancel: () => void;
}

export function NewTaskForm({
  workspaceId,
  pending,
  createTask,
  retryPendingCommand,
  abandonConflictedCommand,
  onCreated,
  onReviewTasks,
  onCancel,
}: NewTaskFormProps) {
  const [phase, setPhase] = useState<'idle' | 'sending' | 'unknown'>('idle');

  if (pending?.status === 'conflicted') {
    return (
      <section
        className="border-y border-[var(--color-error-border)] py-4"
        aria-labelledby="new-task-title"
      >
        <div className="mb-3 flex items-center justify-between gap-3">
          <h2
            id="new-task-title"
            className="text-[var(--text-sm)] font-semibold text-[var(--color-text-primary)]"
          >
            New Task
          </h2>
          <CloseButton onClick={onCancel} />
        </div>
        <div role="alert" className="flex gap-2 text-[var(--text-sm)] text-[var(--color-error)]">
          <AlertCircle aria-hidden="true" className="mt-0.5 size-4 shrink-0" />
          <span>Task creation could not be reconciled.</span>
        </div>
        <div className="mt-4 flex flex-wrap gap-2">
          <TextButton type="button" onClick={onReviewTasks}>
            Review Tasks
          </TextButton>
          <TextButton
            type="button"
            quiet
            onClick={() => {
              abandonConflictedCommand();
              setPhase('idle');
            }}
          >
            Abandon this creation
          </TextButton>
        </div>
      </section>
    );
  }

  const retrying = phase === 'sending';
  return (
    <section
      className="border-y border-[var(--color-border)] py-4"
      aria-labelledby="new-task-title"
    >
      <div className="mb-3 flex items-center justify-between gap-3">
        <h2
          id="new-task-title"
          className="text-[var(--text-sm)] font-semibold text-[var(--color-text-primary)]"
        >
          New Task
        </h2>
        <CloseButton onClick={onCancel} />
      </div>
      {phase === 'unknown' ? (
        <div role="alert" className="mb-4 text-[var(--text-sm)] text-[var(--color-text-secondary)]">
          The server outcome is unknown. Retry the same creation request.
        </div>
      ) : null}
      <div className="flex flex-wrap gap-2">
        {phase === 'unknown' ? (
          <TextButton
            type="button"
            disabled={retrying}
            onClick={() => {
              setPhase('sending');
              void retryPendingCommand()
                .then((result) => {
                  if (isCreateTaskResponse(result)) {
                    onCreated(result.projection.task.task_id);
                    return;
                  }
                  setPhase('unknown');
                })
                .catch(() => {
                  setPhase('unknown');
                });
            }}
          >
            <RotateCcw aria-hidden="true" className="mr-1.5 size-4" />
            Retry Task creation
          </TextButton>
        ) : (
          <TextButton
            type="button"
            primary
            disabled={retrying}
            onClick={() => {
              setPhase('sending');
              void createTask(workspaceId)
                .then((result) => {
                  onCreated(result.projection.task.task_id);
                })
                .catch(() => {
                  setPhase('unknown');
                });
            }}
          >
            {retrying ? (
              <LoaderCircle
                aria-hidden="true"
                className="mr-1.5 size-4 animate-spin motion-reduce:animate-none"
              />
            ) : null}
            Create Task
          </TextButton>
        )}
      </div>
    </section>
  );
}

function TextButton({
  primary = false,
  quiet = false,
  className = '',
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & {
  primary?: boolean;
  quiet?: boolean;
}) {
  const palette = primary
    ? 'bg-[var(--color-accent)] text-white hover:opacity-90'
    : quiet
      ? 'text-[var(--color-text-secondary)] hover:bg-[var(--color-surface-hover)]'
      : 'border border-[var(--color-border)] bg-[var(--color-surface-raised)] text-[var(--color-text-primary)] hover:bg-[var(--color-surface-hover)]';
  return (
    <button
      className={`inline-flex min-h-8 items-center justify-center rounded-[var(--radius-sm)] px-3 py-1.5 text-[var(--text-xs)] font-medium focus-visible:outline focus-visible:outline-2 focus-visible:outline-[var(--color-accent)] disabled:opacity-40 ${palette} ${className}`}
      {...props}
    />
  );
}

function isCreateTaskResponse(result: PendingCommandResult): result is CreateTaskResponse {
  return result !== undefined && Object.hasOwn(result, 'projection');
}

function CloseButton({ onClick }: { onClick: () => void }) {
  return (
    <button
      type="button"
      aria-label="Cancel Task creation"
      title="Cancel Task creation"
      className="flex size-8 shrink-0 items-center justify-center rounded-[var(--radius-sm)] text-[var(--color-text-muted)] hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)] focus-visible:outline focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
      onClick={onClick}
    >
      <X aria-hidden="true" className="size-4" />
    </button>
  );
}
