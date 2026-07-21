import { GitBranch, PanelLeftOpen, Square, Workflow } from 'lucide-react';

import type { PlatformStatus, WorkspaceSummary } from '../../api/generated';
import type { WorkbenchSnapshot } from '../state';
import { ContextDrawerTrigger } from './ContextDrawerTrigger';

export interface WorkbenchHeaderProps {
  onOpenContext: () => void;
  onOpenTasks: () => void;
  onStop: () => void;
  platformStatus: PlatformStatus;
  showContextTrigger?: boolean;
  showTasksTrigger?: boolean;
  stagedSkillCount: number;
  state: WorkbenchSnapshot;
  workspace: WorkspaceSummary | null;
}

function stateLabel(state: string): string {
  return state
    .split('_')
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(' ');
}

export function WorkbenchHeader({
  onOpenContext,
  onOpenTasks,
  onStop,
  platformStatus,
  showContextTrigger = false,
  showTasksTrigger = false,
  stagedSkillCount,
  state,
  workspace,
}: WorkbenchHeaderProps) {
  const projection = state.projection;
  const activeRun = projection?.active_run ?? null;
  const loadedSkillCount = projection?.context_summary?.loaded_skill_count ?? 0;
  const currentState = activeRun?.state ?? projection?.task.state ?? state.connection;

  return (
    <header
      className="flex min-h-14 min-w-0 items-center gap-2 border-b border-[var(--color-border)] bg-[var(--color-surface-raised)] px-3"
      role="banner"
    >
      {showTasksTrigger ? (
        <button
          aria-label="Open Tasks"
          className="inline-flex size-9 shrink-0 items-center justify-center rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
          onClick={onOpenTasks}
          title="Open Tasks"
          type="button"
        >
          <PanelLeftOpen aria-hidden="true" size={18} />
        </button>
      ) : null}
      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 items-center gap-2 text-xs text-[var(--color-text-secondary)]">
          <span className="truncate" data-testid="connection-display-name">
            {platformStatus.connection.display_name}
          </span>
          {workspace !== null ? (
            <>
              <span aria-hidden="true">/</span>
              <span className="truncate">{workspace.label}</span>
            </>
          ) : null}
          {workspace?.branch !== null && workspace?.branch !== undefined ? (
            <span
              className="hidden min-w-0 items-center gap-1 truncate sm:inline-flex"
              data-testid="workspace-branch"
            >
              <GitBranch aria-hidden="true" className="shrink-0" size={13} />
              {workspace.branch}
            </span>
          ) : null}
        </div>
        <div className="mt-0.5 truncate text-sm font-semibold">
          {projection?.task.title ?? 'New task'}
        </div>
      </div>
      <div className="hidden shrink-0 items-center gap-3 text-xs text-[var(--color-text-secondary)] md:flex">
        <span>{loadedSkillCount} loaded</span>
        <span>{stagedSkillCount} staged</span>
        <span className="inline-flex items-center gap-1 text-[var(--color-text-primary)]">
          <Workflow aria-hidden="true" size={14} />
          {stateLabel(currentState)}
        </span>
      </div>
      {activeRun !== null ? (
        <button
          aria-label="Stop run"
          className="inline-flex size-9 shrink-0 items-center justify-center rounded-[var(--radius-sm)] border border-[var(--color-error-border)] text-[var(--color-error)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
          onClick={onStop}
          title="Stop run"
          type="button"
        >
          <Square aria-hidden="true" fill="currentColor" size={14} />
        </button>
      ) : null}
      {showContextTrigger ? <ContextDrawerTrigger onOpen={onOpenContext} /> : null}
    </header>
  );
}
