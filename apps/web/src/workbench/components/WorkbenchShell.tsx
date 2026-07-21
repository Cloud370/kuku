import { PanelLeftClose, PanelRightClose } from 'lucide-react';
import { useEffect, useState, type ReactNode } from 'react';

import type { PlatformStatus, WorkspaceSummary } from '../../api/generated';
import type { WorkbenchSnapshot } from '../state';
import { TaskDrawer } from './TaskDrawer';
import { WorkbenchHeader } from './WorkbenchHeader';

export interface WorkbenchShellProps {
  chat: ReactNode;
  context: ReactNode;
  onOpenContext: () => void;
  onStop: () => void;
  platformStatus: PlatformStatus;
  stagedSkillCount: number;
  state: WorkbenchSnapshot;
  taskNavigation: ReactNode;
  workspace: WorkspaceSummary | null;
}

function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(() => window.matchMedia(query).matches);
  useEffect(() => {
    const media = window.matchMedia(query);
    function update(event: MediaQueryListEvent) {
      setMatches(event.matches);
    }
    setMatches(media.matches);
    media.addEventListener('change', update);
    return () => {
      media.removeEventListener('change', update);
    };
  }, [query]);
  return matches;
}

function IconButton({
  label,
  onClick,
  children,
}: {
  children: ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      aria-label={label}
      className="inline-flex size-9 shrink-0 items-center justify-center rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
      onClick={onClick}
      title={label}
      type="button"
    >
      {children}
    </button>
  );
}

export function WorkbenchShell(props: WorkbenchShellProps) {
  const narrow = useMediaQuery('(max-width: 767px)');
  const [taskDrawerOpen, setTaskDrawerOpen] = useState(false);
  const [contextDrawerOpen, setContextDrawerOpen] = useState(false);
  const [tasksCollapsed, setTasksCollapsed] = useState(false);
  const [contextCollapsed, setContextCollapsed] = useState(false);

  return (
    <div
      className="grid h-dvh min-h-0 min-w-0 grid-cols-[auto_minmax(0,1fr)_auto] grid-rows-[auto_minmax(0,1fr)] overflow-hidden bg-[var(--color-surface)]"
      data-testid="workbench-shell"
    >
      <div className="col-span-3 min-w-0">
        <WorkbenchHeader
          onOpenContext={
            narrow
              ? () => {
                  setContextDrawerOpen(true);
                  props.onOpenContext();
                }
              : () => {
                  setContextCollapsed(false);
                }
          }
          onOpenTasks={
            narrow
              ? () => {
                  setTaskDrawerOpen(true);
                }
              : () => {
                  setTasksCollapsed(false);
                }
          }
          onStop={props.onStop}
          platformStatus={props.platformStatus}
          showContextTrigger={narrow || contextCollapsed}
          showTasksTrigger={narrow || tasksCollapsed}
          stagedSkillCount={props.stagedSkillCount}
          state={props.state}
          workspace={props.workspace}
        />
      </div>
      {narrow ? (
        <>
          <TaskDrawer
            open={taskDrawerOpen}
            onClose={() => {
              setTaskDrawerOpen(false);
            }}
          >
            {props.taskNavigation}
          </TaskDrawer>
          <TaskDrawer
            contentRole="complementary"
            label="Agent Context"
            open={contextDrawerOpen}
            onClose={() => {
              setContextDrawerOpen(false);
            }}
            side="right"
          >
            {props.context}
          </TaskDrawer>
        </>
      ) : tasksCollapsed ? null : (
        <nav
          aria-label="Tasks"
          className="col-start-1 row-start-2 flex h-full w-[var(--workbench-tasks-track)] min-w-0 flex-col border-r border-[var(--color-border)] bg-[var(--color-surface-raised)]"
        >
          <div className="flex h-11 items-center justify-end border-b border-[var(--color-border)] px-2">
            <IconButton
              label="Collapse Tasks"
              onClick={() => {
                setTasksCollapsed(true);
              }}
            >
              <PanelLeftClose aria-hidden="true" size={18} />
            </IconButton>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto p-3">{props.taskNavigation}</div>
        </nav>
      )}
      <main aria-label="Chat" className="col-start-2 row-start-2 min-h-0 min-w-0 overflow-y-auto">
        {props.chat}
      </main>
      {narrow ? null : contextCollapsed ? null : (
        <aside
          aria-label="Agent Context"
          className="col-start-3 row-start-2 flex h-full w-[var(--workbench-context-track)] min-w-0 flex-col border-l border-[var(--color-border)] bg-[var(--color-surface-raised)]"
        >
          <div className="flex h-11 items-center justify-between border-b border-[var(--color-border)] px-2">
            <span className="px-2 text-sm font-semibold">Agent Context</span>
            <IconButton
              label="Collapse Agent Context"
              onClick={() => {
                setContextCollapsed(true);
              }}
            >
              <PanelRightClose aria-hidden="true" size={18} />
            </IconButton>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto p-3">{props.context}</div>
        </aside>
      )}
    </div>
  );
}
