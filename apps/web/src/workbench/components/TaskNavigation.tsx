import { AlertCircle, ChevronRight, LoaderCircle, Plus, RotateCcw } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';

import type {
  CreateTaskResponse,
  PageCursor,
  TaskState,
  TaskSummary,
  WorkspaceId,
  WorkspaceSummary,
} from '../../api/generated';
import { webApi } from '../../api/client';
import type { PendingCommand, PendingCommandResult } from '../workbenchStore';
import { NewTaskForm } from './NewTaskForm';
import { TaskFilter } from './TaskFilter';

type WebApi = typeof webApi;

const groups: ReadonlyArray<readonly [string, readonly TaskState[]]> = [
  ['Needs Attention', ['needs_attention']],
  ['Running / Queued', ['running', 'queued', 'stopping']],
  ['Recent', ['draft', 'completed', 'stopped', 'failed', 'interrupted']],
];

export interface TaskNavigationCommands {
  createTask: (workspaceId: WorkspaceId) => Promise<CreateTaskResponse>;
  retryPendingCommand: () => Promise<PendingCommandResult>;
  abandonConflictedCommand: () => void;
}

export interface TaskNavigationProps {
  api?: WebApi;
  commands: TaskNavigationCommands;
  initialWorkspaceId: WorkspaceId | null;
  selectedTaskId: string | null;
  pendingCommand: PendingCommand | null;
  onSelectTask: (taskId: string) => void;
  onWorkspaceChange: (workspaceId: WorkspaceId) => void;
}

export function TaskNavigation({
  api = webApi,
  commands,
  initialWorkspaceId,
  selectedTaskId,
  pendingCommand,
  onSelectTask,
  onWorkspaceChange,
}: TaskNavigationProps) {
  const [workspaces, setWorkspaces] = useState<WorkspaceSummary[]>([]);
  const [workspaceId, setWorkspaceId] = useState<WorkspaceId | null>(initialWorkspaceId);
  const [workspaceError, setWorkspaceError] = useState(false);
  const [workspacesLoaded, setWorkspacesLoaded] = useState(false);
  const [tasks, setTasks] = useState<TaskSummary[]>([]);
  const [nextCursor, setNextCursor] = useState<PageCursor | null>(null);
  const [query, setQuery] = useState('');
  const [debouncedQuery, setDebouncedQuery] = useState('');
  const [phase, setPhase] = useState<'loading' | 'ready' | 'error'>('loading');
  const [loadingMore, setLoadingMore] = useState(false);
  const [creating, setCreating] = useState(pendingCommand?.kind === 'create_task');
  const [retryVersion, setRetryVersion] = useState(0);
  const listGeneration = useRef(0);

  useEffect(() => {
    let current = true;
    setWorkspaceError(false);
    setWorkspacesLoaded(false);
    void api.workspaces
      .list()
      .then((page) => {
        if (!current) return;
        setWorkspaces(page.items);
        setWorkspacesLoaded(true);
        if (workspaceId === null) {
          const selected = page.items.find((workspace) => workspace.is_default) ?? page.items[0];
          if (selected !== undefined) setWorkspaceId(selected.workspace_id);
        }
      })
      .catch(() => {
        if (current) {
          setWorkspaceError(true);
          setWorkspacesLoaded(true);
        }
      });
    return () => {
      current = false;
    };
  }, [api.workspaces, retryVersion, workspaceId]);

  useEffect(() => {
    const timeout = window.setTimeout(() => {
      setDebouncedQuery(query.trim());
    }, 200);
    return () => {
      window.clearTimeout(timeout);
    };
  }, [query]);

  useEffect(() => {
    if (workspaceId === null) return;
    const generation = ++listGeneration.current;
    setPhase('loading');
    setTasks([]);
    setNextCursor(null);
    void api.tasks
      .list({
        workspace_id: workspaceId,
        search: debouncedQuery.length === 0 ? null : debouncedQuery,
        limit: 100,
        cursor: null,
      })
      .then((page) => {
        if (listGeneration.current !== generation) return;
        setTasks(page.items);
        setNextCursor(page.next_cursor);
        setPhase('ready');
      })
      .catch(() => {
        if (listGeneration.current === generation) setPhase('error');
      });
  }, [api.tasks, debouncedQuery, retryVersion, workspaceId]);

  const groupedTasks = useMemo(
    () =>
      groups.map(([label, states]) => ({
        label,
        tasks: tasks.filter((task) => states.includes(task.state)),
      })),
    [tasks],
  );
  const pendingCreate = pendingCommand?.kind === 'create_task' ? pendingCommand : null;
  const showingCreation = creating;
  const noWorkspaces = workspacesLoaded && workspaces.length === 0;

  const loadMore = async (): Promise<void> => {
    if (workspaceId === null || nextCursor === null || loadingMore) return;
    const requestedWorkspace = workspaceId;
    const requestedCursor = nextCursor;
    const requestedGeneration = listGeneration.current;
    setLoadingMore(true);
    try {
      const page = await api.tasks.list({
        workspace_id: requestedWorkspace,
        search: debouncedQuery.length === 0 ? null : debouncedQuery,
        limit: 100,
        cursor: requestedCursor,
      });
      if (listGeneration.current !== requestedGeneration || workspaceId !== requestedWorkspace) {
        return;
      }
      setTasks((current) => appendUniqueTasks(current, page.items));
      setNextCursor(page.next_cursor);
    } catch {
      setPhase('error');
    } finally {
      setLoadingMore(false);
    }
  };

  return (
    <aside aria-label="Task navigation" className="flex min-h-0 flex-col gap-3">
      <div className="flex items-center gap-2">
        <label className="sr-only" htmlFor="workbench-workspace">
          Workspace
        </label>
        <select
          id="workbench-workspace"
          aria-label="Workspace"
          disabled={noWorkspaces}
          value={workspaceId ?? ''}
          className="h-9 min-w-0 flex-1 rounded-[var(--radius-md)] border border-[var(--color-border)] bg-[var(--color-surface)] px-2 text-[var(--text-sm)] text-[var(--color-text-primary)] outline-none focus-visible:ring-2 focus-visible:ring-[var(--color-accent)]"
          onChange={(event) => {
            const nextWorkspace = event.currentTarget.value;
            listGeneration.current += 1;
            setWorkspaceId(nextWorkspace);
            setQuery('');
            setDebouncedQuery('');
            setCreating(false);
            onWorkspaceChange(nextWorkspace);
          }}
        >
          {workspaceId !== null &&
          !workspaces.some((workspace) => workspace.workspace_id === workspaceId) ? (
            <option value={workspaceId}>{workspaceId}</option>
          ) : null}
          {workspaces.map((workspace) => (
            <option key={workspace.workspace_id} value={workspace.workspace_id}>
              {workspace.label}
            </option>
          ))}
        </select>
        <button
          type="button"
          disabled={noWorkspaces}
          className="inline-flex h-9 shrink-0 items-center justify-center rounded-[var(--radius-sm)] bg-[var(--color-accent)] px-3 text-[var(--text-xs)] font-medium text-white hover:opacity-90 focus-visible:outline focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
          onClick={() => {
            setCreating(true);
          }}
        >
          <Plus aria-hidden="true" className="mr-1.5 size-4" />
          New Task
        </button>
      </div>

      {workspaceError ? (
        <div
          role="alert"
          className="flex items-center justify-between gap-2 text-[var(--text-sm)] text-[var(--color-error)]"
        >
          <span className="flex items-center gap-2">
            <AlertCircle aria-hidden="true" className="size-4" />
            Workspaces unavailable
          </span>
          <RetryButton
            label="Retry Workspaces"
            onClick={() => {
              setRetryVersion((value) => value + 1);
            }}
          />
        </div>
      ) : null}

      {noWorkspaces ? (
        <p role="status" className="py-4 text-[var(--text-sm)] text-[var(--color-text-muted)]">
          No workspaces available
        </p>
      ) : showingCreation && workspaceId !== null ? (
        <NewTaskForm
          workspaceId={workspaceId}
          pending={pendingCreate}
          createTask={commands.createTask}
          retryPendingCommand={commands.retryPendingCommand}
          abandonConflictedCommand={commands.abandonConflictedCommand}
          onCreated={(createdTaskId) => {
            setCreating(false);
            onSelectTask(createdTaskId);
          }}
          onReviewTasks={() => {
            setCreating(false);
            setRetryVersion((value) => value + 1);
          }}
          onCancel={() => {
            setCreating(false);
          }}
        />
      ) : (
        <>
          <TaskFilter value={query} loading={phase === 'loading'} onChange={setQuery} />
          <div className="min-h-0 flex-1 overflow-y-auto" aria-busy={phase === 'loading'}>
            {phase === 'loading' ? (
              <div
                role="status"
                className="flex items-center gap-2 py-4 text-[var(--text-sm)] text-[var(--color-text-muted)]"
              >
                <LoaderCircle
                  aria-hidden="true"
                  className="size-4 animate-spin motion-reduce:animate-none"
                />
                Loading Tasks
              </div>
            ) : null}
            {phase === 'error' ? (
              <div
                role="alert"
                className="flex items-center justify-between gap-2 py-3 text-[var(--text-sm)] text-[var(--color-error)]"
              >
                <span>Tasks unavailable</span>
                <RetryButton
                  label="Retry Tasks"
                  onClick={() => {
                    setRetryVersion((value) => value + 1);
                  }}
                />
              </div>
            ) : null}
            {phase === 'ready' && tasks.length === 0 ? (
              <p className="py-4 text-[var(--text-sm)] text-[var(--color-text-muted)]">
                {debouncedQuery.length === 0 ? 'No Tasks yet' : 'No Tasks match this search'}
              </p>
            ) : null}
            {phase === 'ready'
              ? groupedTasks.map((group) =>
                  group.tasks.length === 0 ? null : (
                    <section
                      key={group.label}
                      className="mb-4"
                      aria-labelledby={`task-group-${group.label.replaceAll(' ', '-').toLowerCase()}`}
                    >
                      <h2
                        id={`task-group-${group.label.replaceAll(' ', '-').toLowerCase()}`}
                        className="mb-1 px-1 text-[var(--text-xs)] font-semibold text-[var(--color-text-muted)]"
                      >
                        {group.label}
                      </h2>
                      <div className="space-y-0.5">
                        {group.tasks.map((task) => (
                          <button
                            key={task.task_id}
                            type="button"
                            aria-current={task.task_id === selectedTaskId ? 'page' : undefined}
                            className="flex min-h-9 w-full items-center gap-2 rounded-[var(--radius-sm)] px-2 py-1.5 text-left text-[var(--text-sm)] text-[var(--color-text-secondary)] hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)] focus-visible:outline focus-visible:outline-2 focus-visible:outline-[var(--color-accent)] aria-[current=page]:bg-[var(--color-surface-raised)] aria-[current=page]:text-[var(--color-text-primary)]"
                            onClick={() => {
                              onSelectTask(task.task_id);
                            }}
                          >
                            <span className="min-w-0 flex-1 truncate">{task.title}</span>
                            <ChevronRight aria-hidden="true" className="size-4 shrink-0" />
                          </button>
                        ))}
                      </div>
                    </section>
                  ),
                )
              : null}
            {phase === 'ready' && nextCursor !== null ? (
              <button
                type="button"
                className="inline-flex min-h-8 w-full items-center justify-center rounded-[var(--radius-sm)] px-3 py-1.5 text-[var(--text-xs)] font-medium text-[var(--color-text-secondary)] hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)] focus-visible:outline focus-visible:outline-2 focus-visible:outline-[var(--color-accent)] disabled:opacity-40"
                disabled={loadingMore}
                onClick={() => {
                  void loadMore();
                }}
              >
                {loadingMore ? (
                  <LoaderCircle
                    aria-hidden="true"
                    className="mr-1.5 size-4 animate-spin motion-reduce:animate-none"
                  />
                ) : null}
                Load more Tasks
              </button>
            ) : null}
          </div>
        </>
      )}
    </aside>
  );
}

function appendUniqueTasks(current: TaskSummary[], next: TaskSummary[]): TaskSummary[] {
  const items = new Map(current.map((task) => [task.task_id, task]));
  for (const task of next) items.set(task.task_id, task);
  return [...items.values()];
}

function RetryButton({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      className="flex size-8 shrink-0 items-center justify-center rounded-[var(--radius-sm)] text-[var(--color-text-muted)] hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)] focus-visible:outline focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
      onClick={onClick}
    >
      <RotateCcw aria-hidden="true" className="size-4" />
    </button>
  );
}
