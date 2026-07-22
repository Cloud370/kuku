import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useMemo, useRef, useState } from 'react';
import {
  Navigate,
  Route,
  Routes,
  useLocation,
  useNavigate,
  useParams,
  useSearchParams,
} from 'react-router-dom';

import { webApi } from './api/client';
import { AgentThreadDialog } from './features/context/AgentThreadDialog';
import { ContextPanel } from './features/context/ContextPanel';
import {
  resolveInitialOpenSections,
  type ContextSectionKey,
} from './features/context/contextSections';
import {
  createExperienceSlots,
  type ExperienceScope,
} from './features/experience/createExperienceSlots';
import {
  createPresentationStore,
  type ReviewFileRouteState,
  type TaskPresentationSnapshot,
} from './features/experience/presentationState';
import { GuideAction } from './features/guide/guideActions';
import type { FirstTaskPrefill } from './features/guide/guideActions';
import { GuideRoute } from './features/guide/GuideRoute';
import { AnnotationDrafts } from './features/review/AnnotationDrafts';
import { ReviewRoute } from './features/review/ReviewRoute';
import type { ReviewMode } from './features/review/ReviewModeTabs';
import { SubmittedReviewNotes } from './features/review/SubmittedReviewNotes';
import {
  classifySubmissionError,
  clearConfirmedDrafts,
  createPendingBatchForLocalNotes,
  makeLocalDraft,
  retryPendingBatch,
  type LocalReviewNote,
  type PendingReviewBatch,
} from './features/review/annotationState';
import { SettingsRoute, type SettingsInitialDialog } from './features/settings/SettingsRoute';
import type { WorkbenchControllerView } from './workbench/WorkbenchController';
import { WorkbenchEntry } from './workbench/WorkbenchEntry';
import type { LocalDraft } from './workbench/state';
import type { WorkbenchRoute } from './workbench/taskSelection';

const presentationStore = createPresentationStore();

function WorkbenchRouteView({ kind = 'latest' }: { kind?: WorkbenchRoute['kind'] }) {
  const navigate = useNavigate();
  const location = useLocation();
  const queryClient = useQueryClient();
  const { taskId } = useParams<{ taskId: string }>();
  const scopeRef = useRef<ExperienceScope | null>(null);
  const [contextRefreshRevision, setContextRefreshRevision] = useState(0);
  const initialDraft = useMemo(
    () => (kind === 'new' ? guideDraftFromLocationState(location.state) : undefined),
    [kind, location.state],
  );
  const route: WorkbenchRoute =
    kind === 'task' && taskId !== undefined
      ? { kind: 'task', taskId }
      : kind === 'new'
        ? { kind: 'new' }
        : { kind: 'latest' };
  const experience = useMemo(
    () =>
      createExperienceSlots({
        enterReview: (state) => {
          void navigate(
            `/tasks/${encodeURIComponent(state.returnTo.taskId)}/review?workspace=${encodeURIComponent(state.workspaceId)}&file=${encodeURIComponent(state.relativePath)}`,
            { state: { reviewRoute: state } },
          );
        },
        invalidateTaskQuery: (changedTaskId, query) => {
          void queryClient.invalidateQueries({ queryKey: [query, changedTaskId] });
          if (query === 'context') setContextRefreshRevision((revision) => revision + 1);
        },
        presentationStore,
        readPresentation: (selectedTaskId) => presentationStore.read(selectedTaskId),
        readScope: () => scopeRef.current,
        restorePresentation: (selectedTaskId, presentation) => {
          navigateToPresentation(navigate, selectedTaskId, presentation);
        },
      }),
    [navigate, queryClient],
  );

  return (
    <WorkbenchEntry
      context={<ContextUnavailable />}
      initialDraft={initialDraft}
      onNavigateTask={(selectedTaskId) => {
        void navigate(`/tasks/${encodeURIComponent(selectedTaskId)}`);
      }}
      onOpenAgentThread={(selectedTaskId, conversationId) => {
        void navigate(
          `/tasks/${encodeURIComponent(selectedTaskId)}?agent=${encodeURIComponent(conversationId)}`,
        );
      }}
      onOpenContext={() => {
        const params = new URLSearchParams(location.search);
        params.set('context', 'current');
        void navigate(`${location.pathname}?${params.toString()}`);
      }}
      onOpenFile={experience.chat.onOpenFile}
      onOpenLoadedSkills={(selectedTaskId) => {
        void navigate(`/tasks/${encodeURIComponent(selectedTaskId)}?context=skills`);
      }}
      onOpenRequestContext={(selectedTaskId, requestId) => {
        void navigate(
          `/tasks/${encodeURIComponent(selectedTaskId)}?request=${encodeURIComponent(requestId)}`,
        );
      }}
      onOpenReview={(selectedTaskId) => {
        void navigate(`/tasks/${encodeURIComponent(selectedTaskId)}/review`);
      }}
      onOpenSettings={() => {
        void navigate('/settings');
      }}
      onTaskDeltaCommitted={experience.onTaskDeltaCommitted}
      renderContext={(view) => {
        scopeRef.current =
          view.snapshot.selectedTaskId === null || view.workspace === null
            ? null
            : {
                taskId: view.snapshot.selectedTaskId,
                workspaceId: view.workspace.workspace_id,
              };
        return (
          <TaskContextSlot
            contextRefreshRevision={contextRefreshRevision}
            key={view.snapshot.selectedTaskId ?? 'no-task'}
            onOpenFile={experience.context.onOpenFile}
            onScopeChange={(scope) => {
              scopeRef.current = scope;
            }}
            view={view}
          />
        );
      }}
      route={route}
    />
  );
}

function ContextUnavailable() {
  return (
    <section aria-label="Context details" className="text-sm text-[var(--color-text-secondary)]">
      <p>Select a Task to inspect Context.</p>
    </section>
  );
}

function TaskContextSlot({
  contextRefreshRevision,
  onOpenFile,
  onScopeChange,
  view,
}: {
  contextRefreshRevision: number;
  onOpenFile: (workspaceId: string, relativePath: string) => void;
  onScopeChange: (scope: ExperienceScope | null) => void;
  view: WorkbenchControllerView;
}) {
  const navigate = useNavigate();
  const location = useLocation();
  const [searchParams] = useSearchParams();
  const taskId = view.snapshot.selectedTaskId;
  const workspaceId = view.workspace?.workspace_id;
  const [openSections, setOpenSections] = useState<ContextSectionKey[]>(() =>
    taskId === null
      ? []
      : resolveInitialOpenSections(presentationStore.read(taskId).openContextSections),
  );
  const selectedRequestId = searchParams.get('request');
  const selectedAgentId = searchParams.get('agent');

  useEffect(() => {
    const scope = taskId === null || workspaceId === undefined ? null : { taskId, workspaceId };
    onScopeChange(scope);
    return () => {
      onScopeChange(null);
    };
  }, [onScopeChange, taskId, workspaceId]);

  useEffect(() => {
    if (taskId === null) return;
    presentationStore.save(taskId, {
      ...presentationStore.read(taskId),
      openContextSections: openSections,
      selectedRequestId,
    });
  }, [openSections, selectedRequestId, taskId]);

  if (taskId === null || workspaceId === undefined) return <ContextUnavailable />;
  const updateParams = (next: URLSearchParams) => {
    const query = next.toString();
    void navigate(query.length === 0 ? location.pathname : `${location.pathname}?${query}`);
  };
  return (
    <>
      <ContextPanel
        onOpenAgent={(conversationId) => {
          const next = new URLSearchParams(location.search);
          next.set('agent', conversationId);
          updateParams(next);
        }}
        onOpenFile={onOpenFile}
        onOpenSectionsChange={setOpenSections}
        onSelectRequest={(requestId) => {
          const next = new URLSearchParams(location.search);
          next.set('request', requestId);
          updateParams(next);
        }}
        onStageSkill={(skillId) => {
          if (view.snapshot.localDraft.skillIds.includes(skillId)) return;
          view.store.setDraft({
            ...view.snapshot.localDraft,
            skillIds: [...view.snapshot.localDraft.skillIds, skillId],
          });
        }}
        onUnstageSkill={(skillId) => {
          view.store.setDraft({
            ...view.snapshot.localDraft,
            skillIds: view.snapshot.localDraft.skillIds.filter((value) => value !== skillId),
          });
        }}
        openSections={openSections}
        refreshRevision={contextRefreshRevision}
        selectedRequestId={selectedRequestId}
        stagedSkillIds={view.snapshot.localDraft.skillIds}
        taskId={taskId}
        workspaceId={workspaceId}
      />
      {selectedAgentId === null ? null : (
        <AgentThreadDialog
          conversationId={selectedAgentId}
          onClose={() => {
            const next = new URLSearchParams(location.search);
            next.delete('agent');
            updateParams(next);
          }}
          onSelectRequest={(requestId) => {
            const next = new URLSearchParams(location.search);
            next.delete('agent');
            next.set('request', requestId);
            updateParams(next);
          }}
          open
          taskId={taskId}
        />
      )}
    </>
  );
}

function ReviewRouteView() {
  const navigate = useNavigate();
  const location = useLocation();
  const { taskId } = useParams<{ taskId: string }>();
  const [searchParams] = useSearchParams();
  const projection = useQuery({
    enabled: taskId !== undefined,
    queryKey: ['task-review', taskId],
    queryFn: () => webApi.tasks.get(taskId ?? ''),
  });
  const [notes, setNotes] = useState<LocalReviewNote[]>([]);
  const [submissionRefresh, setSubmissionRefresh] = useState(0);
  const pendingBatch = useRef<PendingReviewBatch | null>(null);
  if (taskId === undefined) return <Navigate replace to="/" />;
  const workspaceId = searchParams.get('workspace') ?? projection.data?.task.workspace_id ?? null;
  if (workspaceId === null) return <main role="status">Loading Review</main>;
  const initialPath = searchParams.get('file');
  const mode: ReviewMode = initialPath === null ? 'changes' : 'files';
  const returnState = reviewRouteState(location.state);
  const submit = async () => {
    const revision = projection.data?.task_revision;
    if (revision === undefined || notes.length === 0) return;
    const pending =
      pendingBatch.current ??
      createPendingBatchForLocalNotes({ taskId, taskRevision: revision }, notes);
    pendingBatch.current = pending;
    const selected = new Set(pending.localIds);
    setNotes((current) =>
      current.map((note) => (selected.has(note.localId) ? { ...note, state: 'submitting' } : note)),
    );
    try {
      await retryPendingBatch(webApi.review, pending);
      setNotes((current) => clearConfirmedDrafts(current, pending));
      pendingBatch.current = null;
      setSubmissionRefresh((value) => value + 1);
    } catch (error) {
      const action = classifySubmissionError(error);
      setNotes((current) =>
        current.map((note) =>
          selected.has(note.localId)
            ? { ...note, state: action === 'mark_outdated' ? 'outdated' : 'error' }
            : note,
        ),
      );
      if (action === 'abandon_batch') pendingBatch.current = null;
      if (action === 'refresh_task') void projection.refetch();
    }
  };
  return (
    <main className="grid h-dvh min-h-0 grid-rows-[minmax(0,1fr)_auto] bg-[var(--color-surface)] lg:grid-cols-[minmax(0,1fr)_22rem] lg:grid-rows-1">
      <ReviewRoute
        initialPath={initialPath}
        mode={mode}
        onDraftRange={(draft) => {
          pendingBatch.current = null;
          setNotes((current) => [...current, makeLocalDraft(draft)]);
        }}
        onLeave={() => {
          if (returnState === null) {
            void navigate(`/tasks/${encodeURIComponent(taskId)}`);
            return;
          }
          presentationStore.save(taskId, returnState.returnTo.presentation);
          navigateToPresentation(navigate, taskId, returnState.returnTo.presentation);
        }}
        presentation="embedded"
        taskId={taskId}
        workspaceId={workspaceId}
      />
      <aside
        aria-label="Review notes"
        className="min-h-0 overflow-auto border-t border-[var(--color-border)] p-3 lg:border-t-0 lg:border-l"
      >
        <AnnotationDrafts
          notes={notes}
          onRemove={(localId) => {
            pendingBatch.current = null;
            setNotes((current) => current.filter((note) => note.localId !== localId));
          }}
          onSubmit={() => {
            void submit();
          }}
          onUpdateComment={(localId, comment) => {
            pendingBatch.current = null;
            setNotes((current) =>
              current.map((note) =>
                note.localId === localId
                  ? { ...note, note: { ...note.note, comment }, state: 'draft' }
                  : note,
              ),
            );
          }}
        />
        <div className="mt-4">
          <SubmittedReviewNotes refreshKey={submissionRefresh} taskId={taskId} />
        </div>
      </aside>
    </main>
  );
}

function reviewRouteState(value: unknown): ReviewFileRouteState | null {
  if (value === null || typeof value !== 'object' || !('reviewRoute' in value)) return null;
  const route: unknown = value.reviewRoute;
  if (route === null || typeof route !== 'object') return null;
  if (
    !('mode' in route) ||
    route.mode !== 'files' ||
    !('workspaceId' in route) ||
    typeof route.workspaceId !== 'string' ||
    !('relativePath' in route) ||
    typeof route.relativePath !== 'string' ||
    !('returnTo' in route) ||
    route.returnTo === null ||
    typeof route.returnTo !== 'object' ||
    !('taskId' in route.returnTo) ||
    typeof route.returnTo.taskId !== 'string' ||
    !('presentation' in route.returnTo)
  ) {
    return null;
  }
  return route as ReviewFileRouteState;
}

function navigateToPresentation(
  navigate: ReturnType<typeof useNavigate>,
  taskId: string,
  presentation: TaskPresentationSnapshot,
) {
  const query = new URLSearchParams();
  if (presentation.selectedRequestId !== null) {
    query.set('request', presentation.selectedRequestId);
  }
  const suffix = query.size === 0 ? '' : `?${query.toString()}`;
  void navigate(`/tasks/${encodeURIComponent(taskId)}${suffix}`);
}

function SettingsRouteView() {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const initialDialog: SettingsInitialDialog | null =
    searchParams.get('connection') === 'qr' ? 'connection_qr' : null;
  return (
    <SettingsRoute initialDialog={initialDialog} onOpenGuide={() => void navigate('/guide')} />
  );
}

function GuideRouteView() {
  const navigate = useNavigate();
  return (
    <GuideRoute
      onAction={(action, route) => {
        if (action === GuideAction.FirstTask) return;
        void navigate(route);
      }}
      onPrefillFirstTask={(prefill) => {
        void navigate('/tasks/new', { state: { firstTaskPrefill: prefill } });
      }}
    />
  );
}

function guideDraftFromLocationState(value: unknown): LocalDraft | undefined {
  if (value === null || typeof value !== 'object' || !('firstTaskPrefill' in value)) {
    return undefined;
  }
  const prefill: unknown = value.firstTaskPrefill;
  if (
    prefill === null ||
    typeof prefill !== 'object' ||
    !('editable' in prefill) ||
    prefill.editable !== true ||
    !('message' in prefill) ||
    typeof prefill.message !== 'string'
  ) {
    return undefined;
  }
  const accepted = prefill as FirstTaskPrefill;
  return { text: accepted.message, skillIds: [], tierId: null };
}

function App() {
  return (
    <Routes>
      <Route element={<WorkbenchRouteView />} path="/" />
      <Route element={<WorkbenchRouteView />} path="/auth" />
      <Route element={<WorkbenchRouteView />} path="/init" />
      <Route element={<WorkbenchRouteView kind="new" />} path="/tasks/new" />
      <Route element={<WorkbenchRouteView kind="task" />} path="/tasks/:taskId" />
      <Route element={<ReviewRouteView />} path="/tasks/:taskId/review" />
      <Route element={<SettingsRouteView />} path="/settings" />
      <Route element={<GuideRouteView />} path="/guide" />
      <Route element={<Navigate replace to="/" />} path="*" />
    </Routes>
  );
}

export default App;
