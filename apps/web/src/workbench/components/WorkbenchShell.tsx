import { PanelLeftClose, PanelRightClose } from 'lucide-react';
import { useEffect, useState, type ReactNode } from 'react';
import { Group, Panel, Separator, useDefaultLayout, usePanelRef } from 'react-resizable-panels';

import type { PlatformStatus, WorkspaceSummary } from '../../api/generated';
import type { WorkbenchSnapshot } from '../state';
import { TaskDrawer } from './TaskDrawer';
import { WorkbenchHeader } from './WorkbenchHeader';

export interface WorkbenchShellProps {
  chat: ReactNode;
  context: ReactNode;
  onOpenContext: () => void;
  onOpenSettings: () => void;
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
  const tasksPanelRef = usePanelRef();
  const contextPanelRef = usePanelRef();
  const layout = useDefaultLayout({
    groupId: 'workbench-columns',
    panelIds: ['tasks', 'chat', 'context'],
    storage: localStorage,
  });

  return (
    <div
      className="grid h-dvh min-h-0 min-w-0 grid-rows-[auto_minmax(0,1fr)] overflow-hidden bg-[var(--color-surface)]"
      data-testid="workbench-shell"
    >
      <div className="min-w-0">
        <WorkbenchHeader
          onOpenContext={
            narrow
              ? () => {
                  setContextDrawerOpen(true);
                  props.onOpenContext();
                }
              : () => {
                  contextPanelRef.current?.expand();
                  setContextCollapsed(false);
                }
          }
          onOpenSettings={props.onOpenSettings}
          onOpenTasks={
            narrow
              ? () => {
                  setTaskDrawerOpen(true);
                }
              : () => {
                  tasksPanelRef.current?.expand();
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
          <main aria-label="Chat" className="flex min-h-0 min-w-0 flex-col overflow-hidden">
            {props.chat}
          </main>
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
      ) : (
        <Group
          className="min-h-0 min-w-0"
          defaultLayout={layout.defaultLayout}
          id="workbench-columns"
          onLayoutChanged={layout.onLayoutChanged}
          orientation="horizontal"
        >
          <Panel
            collapsedSize={0}
            collapsible
            defaultSize="16rem"
            id="tasks"
            maxSize="32rem"
            minSize="13rem"
            onResize={(size) => {
              setTasksCollapsed(size.asPercentage === 0);
            }}
            panelRef={tasksPanelRef}
          >
            {tasksCollapsed ? null : (
              <nav
                aria-label="Tasks"
                className="flex h-full min-w-0 flex-col bg-[var(--color-surface-raised)]"
              >
                <div className="flex h-11 items-center justify-end border-b border-[var(--color-border)] px-2">
                  <IconButton
                    label="Collapse Tasks"
                    onClick={() => {
                      tasksPanelRef.current?.collapse();
                      setTasksCollapsed(true);
                    }}
                  >
                    <PanelLeftClose aria-hidden="true" size={18} />
                  </IconButton>
                </div>
                <div className="min-h-0 flex-1 overflow-hidden p-3">{props.taskNavigation}</div>
              </nav>
            )}
          </Panel>
          <Separator
            aria-label="Resize Tasks and Chat"
            className="group relative w-px bg-[var(--color-border)] outline-none focus-visible:bg-[var(--color-accent)]"
          >
            <span className="absolute inset-y-0 left-1/2 w-2 -translate-x-1/2 cursor-col-resize group-hover:bg-[var(--color-accent)]/20" />
          </Separator>
          <Panel id="chat" minSize="24rem">
            <main
              aria-label="Chat"
              className="flex h-full min-h-0 min-w-0 flex-col overflow-hidden"
            >
              {props.chat}
            </main>
          </Panel>
          <Separator
            aria-label="Resize Chat and Agent Context"
            className="group relative w-px bg-[var(--color-border)] outline-none focus-visible:bg-[var(--color-accent)]"
          >
            <span className="absolute inset-y-0 left-1/2 w-2 -translate-x-1/2 cursor-col-resize group-hover:bg-[var(--color-accent)]/20" />
          </Separator>
          <Panel
            collapsedSize={0}
            collapsible
            defaultSize="20rem"
            id="context"
            maxSize="40rem"
            minSize="18rem"
            onResize={(size) => {
              setContextCollapsed(size.asPercentage === 0);
            }}
            panelRef={contextPanelRef}
          >
            {contextCollapsed ? null : (
              <aside
                aria-label="Agent Context"
                className="flex h-full min-w-0 flex-col bg-[var(--color-surface-raised)]"
              >
                <div className="flex h-11 items-center justify-between border-b border-[var(--color-border)] px-2">
                  <span className="px-2 text-sm font-semibold">Agent Context</span>
                  <IconButton
                    label="Collapse Agent Context"
                    onClick={() => {
                      contextPanelRef.current?.collapse();
                      setContextCollapsed(true);
                    }}
                  >
                    <PanelRightClose aria-hidden="true" size={18} />
                  </IconButton>
                </div>
                <div className="min-h-0 flex-1 overflow-hidden">{props.context}</div>
              </aside>
            )}
          </Panel>
        </Group>
      )}
    </div>
  );
}
