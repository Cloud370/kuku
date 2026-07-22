import { useEffect, useState } from 'react';

import { webApi } from '../../api/client';
import type { ConversationId, RequestId, TaskId, WorkspaceId } from '../../api/generated';
import { ContextDetail } from './ContextDetail';
import styles from './ContextPanel.module.css';
import type { ContextSectionKey } from './contextSections';
import type { ContextPanelState } from './contextState';
import { toApiError } from './contextState';

export interface ContextPanelProps {
  taskId: TaskId;
  workspaceId: WorkspaceId;
  selectedRequestId: RequestId | null;
  stagedSkillIds: string[];
  openSections: ContextSectionKey[];
  onSelectRequest: (id: RequestId) => void;
  onOpenAgent: (conversationId: ConversationId) => void;
  onOpenFile: (workspaceId: WorkspaceId, relativePath: string) => void;
  onStageSkill: (skillId: string) => void;
  onUnstageSkill: (skillId: string) => void;
  onOpenSectionsChange: (keys: ContextSectionKey[]) => void;
  refreshRevision?: number;
}

function StateSummary({ label, detail }: { label: string; detail: string }) {
  return (
    <div className={styles.summary}>
      <p className="text-sm font-medium">{label}</p>
      <p className="mt-1 text-xs text-[var(--color-text-secondary)]">{detail}</p>
    </div>
  );
}

function announcement(state: ContextPanelState, loading: boolean): string {
  if (state.kind === 'loading') return 'Loading Context';
  if (loading) return 'Loading selected Context';
  if (state.kind === 'error') return 'Context unavailable';
  if (state.kind === 'empty') return 'Context is empty';
  return 'Context loaded';
}

export function ContextPanel({
  taskId,
  workspaceId,
  selectedRequestId,
  stagedSkillIds,
  openSections,
  onSelectRequest,
  onOpenAgent,
  onOpenFile,
  onStageSkill,
  onUnstageSkill,
  onOpenSectionsChange,
  refreshRevision = 0,
}: ContextPanelProps) {
  const [state, setState] = useState<ContextPanelState>({ kind: 'loading' });
  const [loading, setLoading] = useState(true);
  const [displayedRequestId, setDisplayedRequestId] = useState<RequestId | null>(null);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setState((current) => (current.kind === 'ready' ? current : { kind: 'loading' }));
    const snapshot =
      selectedRequestId === null
        ? webApi.context.current(taskId)
        : webApi.context.historical(taskId, selectedRequestId);
    void Promise.all([snapshot, webApi.catalog.workspace(workspaceId, { search: null })])
      .then(([nextSnapshot, catalog]) => {
        if (!active) return;
        if (nextSnapshot.selected_request === null) {
          setState({ kind: 'empty', reason: 'no_request' });
          setLoading(false);
          return;
        }
        setState({
          kind: 'ready',
          snapshot: nextSnapshot,
          catalog,
          mode: 'verified',
        });
        setDisplayedRequestId(selectedRequestId);
        setLoading(false);
      })
      .catch((error: unknown) => {
        if (active) {
          setState({ kind: 'error', error: toApiError(error) });
          setLoading(false);
        }
      });
    return () => {
      active = false;
    };
  }, [refreshRevision, selectedRequestId, taskId, workspaceId]);

  return (
    <section aria-label="Context" className={styles.panel}>
      <header className={styles.header}>
        <h2 className="text-sm font-semibold">Context</h2>
        {state.kind === 'ready' && stagedSkillIds.length > 0 ? (
          <span className="text-xs text-[var(--color-text-secondary)]">Next Request preview</span>
        ) : null}
      </header>
      <div aria-atomic="true" aria-live="polite" className="sr-only" role="status">
        {announcement(state, loading)}
      </div>
      {state.kind === 'ready' ? (
        <ContextDetail
          catalog={state.catalog}
          historical={displayedRequestId !== null}
          loading={loading}
          onOpenAgent={onOpenAgent}
          onOpenFile={onOpenFile}
          onOpenSectionsChange={onOpenSectionsChange}
          onSelectRequest={onSelectRequest}
          onStageSkill={onStageSkill}
          onUnstageSkill={onUnstageSkill}
          openSections={openSections}
          selectedRequestId={selectedRequestId}
          snapshot={state.snapshot}
          stagedSkillIds={stagedSkillIds}
          workspaceId={workspaceId}
        />
      ) : (
        <div className={styles.stateBody}>
          {state.kind === 'loading' ? (
            <StateSummary detail="Reading the latest immutable snapshot" label="Loading Context" />
          ) : state.kind === 'empty' ? (
            <StateSummary
              detail="No provider Request has a Context snapshot yet"
              label="No Context"
            />
          ) : (
            <StateSummary detail="The snapshot could not be loaded" label="Context unavailable" />
          )}
          <div className={styles.stateMessage}>
            {state.kind === 'loading' ? (
              <p>Loading Context</p>
            ) : state.kind === 'empty' ? (
              <p>Context will appear after the first provider Request.</p>
            ) : (
              <div>
                <p className="font-medium">{state.error.code}</p>
                <p className="mt-1 text-xs">{state.error.message}</p>
              </div>
            )}
          </div>
        </div>
      )}
    </section>
  );
}
