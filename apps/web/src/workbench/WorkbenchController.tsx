import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useStore } from 'zustand';

import type {
  ContextCatalog,
  PlatformStatus,
  TaskDelta,
  WorkspaceId,
  WorkspaceSummary,
} from '../api/generated';
import { webApi } from '../api/client';
import { selectTimelineItems } from './state';
import type { LocalDraft } from './state';
import { resolveInitialTask, type WorkbenchRoute } from './taskSelection';
import {
  createWorkbenchStore,
  type SubmitRunInput,
  type WorkbenchStoreState,
} from './workbenchStore';

export interface WorkbenchControllerView {
  catalog: ContextCatalog | null;
  catalogError: string | null;
  loadOlder: () => Promise<void>;
  onRetry: () => Promise<void>;
  retryCatalog: () => void;
  platform: PlatformStatus;
  selectTask: (taskId: string) => Promise<void>;
  selectWorkspace: (workspaceId: WorkspaceId) => void;
  searchCatalog: (query: string) => void;
  snapshot: WorkbenchStoreState['snapshot'];
  store: WorkbenchStoreState;
  timelineItems: ReturnType<typeof selectTimelineItems>;
  workspace: WorkspaceSummary | null;
}

interface WorkbenchControllerProps {
  api?: typeof webApi;
  children: (view: WorkbenchControllerView) => ReactNode;
  initialDraft?: LocalDraft;
  onTaskDeltaCommitted?: (taskId: string, delta: TaskDelta) => void;
  platform: PlatformStatus;
  route?: WorkbenchRoute;
}

export function WorkbenchController({
  api = webApi,
  children,
  initialDraft,
  onTaskDeltaCommitted,
  platform,
  route,
}: WorkbenchControllerProps) {
  const storeRef = useRef<ReturnType<typeof createWorkbenchStore> | undefined>(undefined);
  if (storeRef.current === undefined) storeRef.current = createWorkbenchStore(api);
  const store = storeRef.current;
  const state = useStore(store);
  const incomingRouteKind = route?.kind ?? 'latest';
  const incomingRouteTaskId = route?.kind === 'task' ? route.taskId : null;
  const [requestedRoute, setRequestedRoute] = useState<WorkbenchRoute>(route ?? { kind: 'latest' });
  const [workspaces, setWorkspaces] = useState<WorkspaceSummary[]>([]);
  const [workspaceId, setWorkspaceId] = useState<WorkspaceId | null>(null);
  const [catalog, setCatalog] = useState<ContextCatalog | null>(null);
  const [catalogError, setCatalogError] = useState<string | null>(null);
  const catalogSearchGeneration = useRef(0);
  const [retryVersion, setRetryVersion] = useState(0);
  const [resolvedRoute, setResolvedRoute] = useState<WorkbenchRoute | null>(
    incomingRouteTaskId === null ? null : { kind: 'task', taskId: incomingRouteTaskId },
  );
  const workspace = useMemo(
    () => workspaces.find((entry) => entry.workspace_id === workspaceId) ?? null,
    [workspaceId, workspaces],
  );

  useEffect(() => {
    setRequestedRoute(
      incomingRouteTaskId === null
        ? { kind: incomingRouteKind as 'latest' | 'new' }
        : { kind: 'task', taskId: incomingRouteTaskId },
    );
  }, [incomingRouteKind, incomingRouteTaskId]);

  useEffect(() => {
    let current = true;
    void api.workspaces
      .list()
      .then((page) => {
        if (!current) return;
        setWorkspaces(page.items);
        setWorkspaceId(
          page.items.find((entry) => entry.is_default)?.workspace_id ??
            page.items[0]?.workspace_id ??
            null,
        );
      })
      .catch((error: unknown) => {
        if (current) store.getState().setTaskError(errorMessage(error, 'Workspaces unavailable'));
      });
    return () => {
      current = false;
    };
  }, [api.workspaces, retryVersion, store]);

  useEffect(() => {
    if (workspaceId === null) return;
    let current = true;
    const generation = ++catalogSearchGeneration.current;
    setCatalog(null);
    setCatalogError(null);
    void api.catalog
      .workspace(workspaceId, { search: null })
      .then((value) => {
        if (current && generation === catalogSearchGeneration.current) setCatalog(value);
      })
      .catch((error: unknown) => {
        if (current && generation === catalogSearchGeneration.current) {
          setCatalogError(errorMessage(error, 'Catalog unavailable'));
        }
      });
    return () => {
      current = false;
    };
  }, [api.catalog, retryVersion, workspaceId]);

  useEffect(() => {
    if (workspaceId === null) return;
    let current = true;
    const resolve = async () => {
      const next =
        requestedRoute.kind === 'latest'
          ? await resolveInitialTask(api, workspaceId)
          : requestedRoute.kind === 'new'
            ? { kind: 'new' as const }
            : requestedRoute;
      if (!current) return;
      setResolvedRoute(next);
    };
    void resolve().catch((error: unknown) => {
      if (current) store.getState().setTaskError(errorMessage(error, 'Tasks unavailable'));
    });
    return () => {
      current = false;
    };
  }, [api, requestedRoute, retryVersion, store, workspaceId]);

  useEffect(() => {
    if (resolvedRoute?.kind !== 'task') {
      if (resolvedRoute?.kind === 'new') {
        store.getState().clearTask();
        if (initialDraft !== undefined) store.getState().setDraft(initialDraft);
      }
      return;
    }
    const taskId = resolvedRoute.taskId;
    let current = true;
    let unsubscribe: (() => void) | undefined;
    void store
      .getState()
      .loadTask(taskId)
      .then(() => {
        if (current) unsubscribe = store.getState().subscribeTask(taskId);
      })
      .catch((error: unknown) => {
        if (current) store.getState().setTaskError(errorMessage(error, 'Task unavailable'));
      });
    return () => {
      current = false;
      unsubscribe?.();
    };
  }, [initialDraft, resolvedRoute, retryVersion, store]);

  useEffect(() => {
    store.getState().setTaskDeltaObserver(onTaskDeltaCommitted ?? null);
    return () => {
      store.getState().setTaskDeltaObserver(null);
    };
  }, [onTaskDeltaCommitted, store]);

  const view: WorkbenchControllerView = {
    catalog,
    catalogError,
    loadOlder: () => store.getState().loadOlder(),
    onRetry: () => {
      if (store.getState().snapshot.projection === null) {
        setRetryVersion((value) => value + 1);
        return Promise.resolve();
      }
      return store.getState().reconnectFromCursor();
    },
    platform,
    retryCatalog: () => {
      setRetryVersion((value) => value + 1);
    },
    selectTask: async (taskId) => {
      try {
        await store.getState().loadTask(taskId);
        store.getState().subscribeTask(taskId);
      } catch (error) {
        store.getState().setTaskError(errorMessage(error, 'Task unavailable'));
      }
    },
    selectWorkspace: (selectedWorkspaceId) => {
      store.getState().clearTask();
      setResolvedRoute(null);
      setRequestedRoute({ kind: 'latest' });
      setWorkspaceId(selectedWorkspaceId);
    },
    searchCatalog: (query) => {
      if (workspaceId === null) return;
      const generation = ++catalogSearchGeneration.current;
      const selectedWorkspaceId = workspaceId;
      setCatalogError(null);
      void api.catalog
        .workspace(selectedWorkspaceId, { search: query.trim().length === 0 ? null : query.trim() })
        .then((value) => {
          if (generation === catalogSearchGeneration.current) setCatalog(value);
        })
        .catch((error: unknown) => {
          if (generation === catalogSearchGeneration.current) {
            setCatalogError(errorMessage(error, 'Catalog unavailable'));
          }
        });
    },
    snapshot: state.snapshot,
    store: state,
    timelineItems: selectTimelineItems(state.snapshot),
    workspace,
  };
  return <>{children(view)}</>;
}

export type ComposerSubmit = SubmitRunInput;

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback;
}
