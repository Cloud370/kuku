import { useRef, type ReactNode } from 'react';

import type { PlatformStatus, TaskDelta } from '../api/generated';
import { webApi } from '../api/client';
import type { LocalDraft } from './state';
import { EntryGate } from './entry/EntryGate';
import { WorkbenchController, type WorkbenchControllerView } from './WorkbenchController';
import type { WorkbenchRoute } from './taskSelection';
import { StateBoundary } from './components/StateBoundary';
import { ChatTimeline } from './components/ChatTimeline';
import { Composer } from './components/Composer';
import { RunLiveRegion } from './components/RunLiveRegion';
import { TaskNavigation } from './components/TaskNavigation';
import { WorkbenchShell } from './components/WorkbenchShell';

export interface WorkbenchEntryProps {
  api?: typeof webApi;
  context: ReactNode;
  initialDraft?: LocalDraft;
  renderContext?: (view: WorkbenchControllerView) => ReactNode;
  onOpenAgentThread: (taskId: string, conversationId: string) => void;
  onOpenContext: () => void;
  onOpenFile: (workspaceId: string, relativePath: string) => void;
  onOpenLoadedSkills: (taskId: string) => void;
  onOpenRequestContext: (taskId: string, requestId: string) => void;
  onOpenReview: (taskId: string) => void;
  onOpenSettings: () => void;
  onNavigateTask?: (taskId: string) => void;
  onTaskDeltaCommitted?: (taskId: string, delta: TaskDelta) => void;
  onStop?: () => void;
  route?: WorkbenchRoute;
}

export function WorkbenchEntry({
  api = webApi,
  context,
  initialDraft,
  renderContext,
  onOpenAgentThread,
  onOpenContext,
  onOpenFile,
  onOpenLoadedSkills,
  onOpenRequestContext,
  onOpenReview,
  onOpenSettings,
  onNavigateTask,
  onStop,
  onTaskDeltaCommitted,
  route,
}: WorkbenchEntryProps) {
  void onOpenAgentThread;
  return (
    <EntryGate
      renderWorkbench={(platformStatus) => (
        <WorkbenchController
          api={api}
          initialDraft={initialDraft}
          onTaskDeltaCommitted={onTaskDeltaCommitted}
          platform={platformStatus}
          route={route}
        >
          {(view) => (
            <WorkbenchView
              api={api}
              context={context}
              onNavigateTask={onNavigateTask}
              onOpenContext={onOpenContext}
              onOpenFile={onOpenFile}
              onOpenLoadedSkills={onOpenLoadedSkills}
              onOpenRequestContext={onOpenRequestContext}
              onOpenReview={onOpenReview}
              onOpenSettings={onOpenSettings}
              onStop={onStop}
              platformStatus={platformStatus}
              renderContext={renderContext}
              view={view}
            />
          )}
        </WorkbenchController>
      )}
    />
  );
}

interface WorkbenchViewProps {
  api: typeof webApi;
  context: ReactNode;
  onNavigateTask?: (taskId: string) => void;
  onOpenContext: () => void;
  onOpenFile: (workspaceId: string, relativePath: string) => void;
  onOpenLoadedSkills: (taskId: string) => void;
  onOpenRequestContext: (taskId: string, requestId: string) => void;
  onOpenReview: (taskId: string) => void;
  onOpenSettings: () => void;
  onStop?: () => void;
  platformStatus: PlatformStatus;
  renderContext?: (view: WorkbenchControllerView) => ReactNode;
  view: WorkbenchControllerView;
}

export function WorkbenchView({
  api,
  context,
  onNavigateTask,
  onOpenContext,
  onOpenFile,
  onOpenLoadedSkills,
  onOpenRequestContext,
  onOpenReview,
  onOpenSettings,
  onStop,
  platformStatus,
  renderContext,
  view,
}: WorkbenchViewProps) {
  const retained = useRef<Pick<WorkbenchControllerView, 'snapshot' | 'timelineItems'> | undefined>(
    undefined,
  );
  const projection = view.snapshot.projection;
  if (projection !== null) {
    retained.current = { snapshot: view.snapshot, timelineItems: view.timelineItems };
  }
  const transitioning =
    projection === null &&
    view.snapshot.connection === 'loading' &&
    retained.current !== undefined &&
    retained.current.snapshot.selectedTaskId !== view.snapshot.selectedTaskId;
  const display = transitioning ? retained.current : undefined;
  const displaySnapshot = display?.snapshot ?? view.snapshot;
  const displayTimelineItems = display?.timelineItems ?? view.timelineItems;
  const displayView =
    display === undefined
      ? view
      : { ...view, snapshot: displaySnapshot, timelineItems: displayTimelineItems };
  const displayProjection = displaySnapshot.projection;
  const tiers = view.catalog?.tiers ?? [];
  const skills = view.catalog?.skills ?? [];
  const defaultTierId =
    projection?.selected_tier_id ??
    tiers.find(({ tier }) => tier.is_default)?.tier.tier_id ??
    tiers[0]?.tier.tier_id ??
    '';
  const taskId = view.snapshot.selectedTaskId;

  return (
    <WorkbenchShell
      chat={
        <StateBoundary
          onRetry={() => void view.onRetry()}
          retainContentWhileLoading={transitioning}
          snapshot={view.snapshot}
        >
          {view.catalogError === null ? null : (
            <div
              className="flex items-center justify-between gap-3 border-b border-[var(--color-border)] bg-[var(--color-surface-raised)] px-4 py-2 text-xs"
              role="alert"
            >
              <span>{view.catalogError}</span>
              <button
                className="shrink-0 font-medium text-[var(--color-accent)]"
                onClick={() => {
                  view.retryCatalog();
                }}
                type="button"
              >
                Retry catalog
              </button>
            </div>
          )}
          <div
            aria-busy={transitioning}
            className={`min-h-0 flex-1 overflow-y-auto transition-opacity duration-150 ${transitioning ? 'opacity-60' : 'opacity-100'}`}
            data-chat-scroll
            key={displayProjection?.task.task_id ?? 'no-task'}
          >
            {transitioning ? (
              <div
                aria-label="Loading selected Task"
                className="sticky top-0 z-20 h-0.5 overflow-hidden bg-[var(--color-accent-muted)]"
                role="status"
              >
                <span className="block h-full w-full animate-pulse bg-[var(--color-accent)]" />
              </div>
            ) : null}
            <ChatTimeline
              loadOlder={view.loadOlder}
              onOpenFile={onOpenFile}
              onOpenRequestContext={onOpenRequestContext}
              onOpenReview={onOpenReview}
              onRespond={(_selectedTaskId, interactionId, choiceId) => {
                return view.store.respond(interactionId, choiceId).then(() => undefined);
              }}
              onReturnToRecent={() => {
                view.store.returnToRecent();
              }}
              projection={displayProjection}
              timelineHistory={displaySnapshot.timelineHistory}
              timelineItems={displayTimelineItems}
            />
          </div>
          <Composer
            activeRunId={projection?.active_run?.run_id ?? null}
            catalogReady={
              projection !== null && view.catalog !== null && view.catalogError === null
            }
            defaultTierId={defaultTierId}
            draft={view.snapshot.localDraft}
            loadedSkillCount={projection?.context_summary?.loaded_skill_count ?? 0}
            onDraftChange={(draft) => {
              view.store.setDraft(draft);
            }}
            onOpenLoadedSkills={() => {
              if (taskId !== null) onOpenLoadedSkills(taskId);
            }}
            onRetryPendingCommand={() => {
              void view.store.retryPendingCommand();
            }}
            onSearchSkills={(query) => {
              view.searchCatalog(query);
            }}
            onStop={() => {
              void view.store.stopRun();
              onStop?.();
            }}
            onSubmit={async (input) => {
              await view.store.submitRun(input);
            }}
            pendingCommand={view.store.pendingCommand}
            skills={skills}
            taskId={taskId}
            tiers={tiers}
          />
          <RunLiveRegion projection={projection} />
        </StateBoundary>
      }
      context={
        <div
          aria-busy={transitioning}
          className={`h-full min-h-0 transition-opacity duration-150 ${transitioning ? 'pointer-events-none opacity-60' : 'opacity-100'}`}
        >
          {renderContext?.(displayView) ?? context}
        </div>
      }
      onOpenContext={onOpenContext}
      onOpenSettings={onOpenSettings}
      onStop={() => {
        void view.store.stopRun();
        onStop?.();
      }}
      platformStatus={platformStatus}
      stagedSkillCount={displaySnapshot.localDraft.skillIds.length}
      state={displaySnapshot}
      taskNavigation={
        <TaskNavigation
          api={api}
          commands={{
            abandonConflictedCommand: () => {
              view.store.abandonConflictedCommand();
            },
            createTask: (workspaceId) => view.store.createTask(workspaceId),
            retryPendingCommand: () => view.store.retryPendingCommand(),
          }}
          initialWorkspaceId={view.workspace?.workspace_id ?? null}
          onSelectTask={(selectedTaskId) => {
            if (onNavigateTask === undefined) {
              void view.selectTask(selectedTaskId);
            } else {
              onNavigateTask(selectedTaskId);
            }
          }}
          onWorkspaceChange={(workspaceId) => {
            view.selectWorkspace(workspaceId);
          }}
          pendingCommand={view.store.pendingCommand}
          selectedTaskId={view.snapshot.selectedTaskId}
        />
      }
      workspace={view.workspace}
    />
  );
}
