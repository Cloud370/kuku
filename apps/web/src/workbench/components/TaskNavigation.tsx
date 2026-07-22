import {
  AlertCircle,
  Check,
  ChevronRight,
  ChevronsUpDown,
  FolderGit2,
  GitBranch,
  LoaderCircle,
  Plus,
  RotateCcw,
} from 'lucide-react';
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
  createTask: (workspaceId: WorkspaceId) => Promise<CreateTaskResponse | undefined>;
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
  const [dismissedCreateId, setDismissedCreateId] = useState<number | null>(null);
  const [retryVersion, setRetryVersion] = useState(0);
  const [workspaceMenuOpen, setWorkspaceMenuOpen] = useState(false);
  const listGeneration = useRef(0);
  const previousInitialWorkspaceId = useRef(initialWorkspaceId);
  const workspaceIdRef = useRef(workspaceId);
  workspaceIdRef.current = workspaceId;
  const workspaceMenu = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (previousInitialWorkspaceId.current === initialWorkspaceId) return;
    previousInitialWorkspaceId.current = initialWorkspaceId;
    if (workspaceIdRef.current === initialWorkspaceId) return;
    listGeneration.current += 1;
    setLoadingMore(false);
    setWorkspaceId(initialWorkspaceId);
    setQuery('');
    setDebouncedQuery('');
    setCreating(false);
    setWorkspaceMenuOpen(false);
  }, [initialWorkspaceId]);

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
  const pendingCreateId = pendingCreate?.commandId ?? -1;
  const hasNonCreatePending = pendingCommand !== null && pendingCreate === null;
  const showingCreation =
    !hasNonCreatePending &&
    (creating || (pendingCreate !== null && pendingCreateId !== dismissedCreateId));
  const noWorkspaces = workspacesLoaded && workspaces.length === 0;
  const selectedWorkspace = workspaces.find((workspace) => workspace.workspace_id === workspaceId);

  useEffect(() => {
    if (!workspaceMenuOpen) return;
    const close = (event: MouseEvent) => {
      if (!workspaceMenu.current?.contains(event.target as Node)) setWorkspaceMenuOpen(false);
    };
    document.addEventListener('mousedown', close);
    return () => {
      document.removeEventListener('mousedown', close);
    };
  }, [workspaceMenuOpen]);

  const selectWorkspace = (nextWorkspace: WorkspaceId) => {
    listGeneration.current += 1;
    setLoadingMore(false);
    setWorkspaceId(nextWorkspace);
    setQuery('');
    setDebouncedQuery('');
    setCreating(false);
    setWorkspaceMenuOpen(false);
    onWorkspaceChange(nextWorkspace);
  };

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
      if (listGeneration.current === requestedGeneration && workspaceId === requestedWorkspace) {
        setPhase('error');
      }
    } finally {
      if (listGeneration.current === requestedGeneration && workspaceId === requestedWorkspace) {
        setLoadingMore(false);
      }
    }
  };

  return (
    <aside aria-label="Task navigation" className="flex min-h-0 flex-col gap-3">
      <div className="flex items-center gap-2">
        <div className="relative min-w-0 flex-1" ref={workspaceMenu}>
          <button
            aria-controls="workbench-workspace-options"
            aria-expanded={workspaceMenuOpen}
            aria-haspopup="listbox"
            aria-label="Workspace"
            className="flex min-h-10 w-full min-w-0 items-center gap-2 border border-[var(--color-border)] bg-[var(--color-surface)] px-2.5 text-left outline-none hover:bg-[var(--color-surface-hover)] focus-visible:ring-2 focus-visible:ring-[var(--color-accent)] disabled:opacity-40"
            disabled={noWorkspaces || hasNonCreatePending}
            onClick={() => {
              setWorkspaceMenuOpen((open) => !open);
            }}
            onKeyDown={(event) => {
              if (event.key === 'Escape') setWorkspaceMenuOpen(false);
            }}
            role="combobox"
            type="button"
          >
            <FolderGit2 aria-hidden="true" className="shrink-0" size={16} />
            <span className="min-w-0 flex-1">
              <span className="block truncate text-sm font-medium">
                {selectedWorkspace?.label ?? workspaceId ?? 'Select workspace'}
              </span>
              <span className="flex min-w-0 items-center gap-1 text-xs text-[var(--color-text-muted)]">
                {selectedWorkspace?.branch === null || selectedWorkspace?.branch === undefined ? (
                  <span className="truncate">No branch</span>
                ) : (
                  <>
                    <GitBranch aria-hidden="true" className="shrink-0" size={12} />
                    <span className="truncate">{selectedWorkspace.branch}</span>
                  </>
                )}
              </span>
            </span>
            <ChevronsUpDown aria-hidden="true" className="shrink-0" size={15} />
          </button>
          {workspaceMenuOpen ? (
            <div
              aria-label="Workspace options"
              className="absolute left-0 right-0 z-30 mt-1 max-h-72 overflow-y-auto border border-[var(--color-border-strong)] bg-[var(--color-surface-raised)] p-1 shadow-[var(--shadow-elevated)]"
              id="workbench-workspace-options"
              role="listbox"
            >
              {workspaces.map((workspace) => {
                const selected = workspace.workspace_id === workspaceId;
                return (
                  <button
                    aria-selected={selected}
                    className="flex w-full min-w-0 items-start gap-2 px-2 py-2 text-left hover:bg-[var(--color-surface-hover)] focus-visible:bg-[var(--color-surface-hover)] focus-visible:outline-none"
                    key={workspace.workspace_id}
                    onClick={() => {
                      selectWorkspace(workspace.workspace_id);
                    }}
                    role="option"
                    type="button"
                  >
                    <span className="mt-0.5 flex size-4 shrink-0 items-center justify-center">
                      {selected ? <Check aria-hidden="true" size={14} /> : null}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="flex items-center gap-2">
                        <span className="min-w-0 flex-1 break-words text-sm font-medium">
                          {workspace.label}
                        </span>
                        {workspace.is_default ? (
                          <span className="shrink-0 text-xs text-[var(--color-accent)]">
                            Default
                          </span>
                        ) : null}
                      </span>
                      <span className="mt-0.5 block break-all text-xs text-[var(--color-text-muted)]">
                        {workspace.branch ?? 'No branch'} · {workspace.availability} ·{' '}
                        {workspace.workspace_id}
                      </span>
                    </span>
                  </button>
                );
              })}
            </div>
          ) : null}
        </div>
        <button
          aria-label="New Task"
          type="button"
          disabled={noWorkspaces || hasNonCreatePending}
          className="inline-flex size-10 shrink-0 items-center justify-center rounded-[var(--radius-sm)] bg-[var(--color-accent)] text-white hover:opacity-90 focus-visible:outline focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
          onClick={() => {
            setDismissedCreateId(null);
            setCreating(true);
          }}
          title="New Task"
        >
          <Plus aria-hidden="true" className="size-4" />
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
          workspaceId={pendingCreate?.body.workspace_id ?? workspaceId}
          pending={pendingCreate}
          createTask={commands.createTask}
          retryPendingCommand={commands.retryPendingCommand}
          abandonConflictedCommand={commands.abandonConflictedCommand}
          onCreated={(createdTaskId) => {
            setCreating(false);
            setDismissedCreateId(null);
            onSelectTask(createdTaskId);
          }}
          onReviewTasks={() => {
            setCreating(false);
            setDismissedCreateId(pendingCreateId);
            setRetryVersion((value) => value + 1);
          }}
          onCancel={() => {
            setCreating(false);
          }}
        />
      ) : (
        <>
          <TaskFilter
            value={query}
            loading={phase === 'loading'}
            onChange={(value) => {
              listGeneration.current += 1;
              setLoadingMore(false);
              setQuery(value);
            }}
          />
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
