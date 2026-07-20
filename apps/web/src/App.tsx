import { Navigate, Route, Routes, useLocation, useNavigate, useParams } from 'react-router-dom';

import { WorkbenchEntry } from './workbench/WorkbenchEntry';
import type { WorkbenchRoute } from './workbench/taskSelection';

function WorkbenchRouteView({ kind = 'latest' }: { kind?: WorkbenchRoute['kind'] }) {
  const navigate = useNavigate();
  const location = useLocation();
  const { taskId } = useParams<{ taskId: string }>();
  const route: WorkbenchRoute =
    kind === 'task' && taskId !== undefined
      ? { kind: 'task', taskId }
      : kind === 'new'
        ? { kind: 'new' }
        : { kind: 'latest' };

  return (
    <WorkbenchEntry
      context={
        <section
          aria-label="Context details"
          className="text-sm text-[var(--color-text-secondary)]"
        >
          <p>No Context selected.</p>
        </section>
      }
      onNavigateTask={(selectedTaskId) => {
        void navigate(`/tasks/${encodeURIComponent(selectedTaskId)}`);
      }}
      onOpenAgentThread={(selectedTaskId, conversationId) => {
        void navigate(
          `/tasks/${encodeURIComponent(selectedTaskId)}?agent=${encodeURIComponent(conversationId)}`,
        );
      }}
      onOpenContext={() => {
        void navigate(`${location.pathname}?context=current`);
      }}
      onOpenFile={(workspaceId, relativePath) => {
        if (taskId === undefined) return;
        void navigate(
          `/tasks/${encodeURIComponent(taskId)}/review?workspace=${encodeURIComponent(workspaceId)}&file=${encodeURIComponent(relativePath)}`,
        );
      }}
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
      route={route}
    />
  );
}

function App() {
  return (
    <Routes>
      <Route element={<WorkbenchRouteView />} path="/" />
      <Route element={<WorkbenchRouteView kind="new" />} path="/tasks/new" />
      <Route element={<WorkbenchRouteView kind="task" />} path="/tasks/:taskId" />
      <Route element={<WorkbenchRouteView kind="task" />} path="/tasks/:taskId/review" />
      <Route element={<Navigate replace to="/" />} path="*" />
    </Routes>
  );
}

export default App;
