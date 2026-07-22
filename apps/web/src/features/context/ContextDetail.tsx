import { Code2 } from 'lucide-react';
import { useMemo, useRef, useState } from 'react';

import type {
  ContextCatalog,
  ContextSnapshot,
  ConversationId,
  RequestId,
  WorkspaceId,
} from '../../api/generated';
import { ContextAccordion, type ContextAccordionItem } from './ContextAccordion';
import { ExactRequestDialog } from './ExactRequestDialog';
import { RequestHistory } from './RequestHistory';
import { ContextSectionContent } from './ContextSectionContent';
import { ContextSummary } from './ContextSummary';
import type { ContextSectionKey } from './contextSections';
import styles from './ContextPanel.module.css';
import { selectContextView } from './contextSelectors';

interface ContextDetailProps {
  catalog: ContextCatalog;
  snapshot: ContextSnapshot;
  workspaceId: WorkspaceId;
  stagedSkillIds: string[];
  openSections: ContextSectionKey[];
  historical?: boolean;
  onSelectRequest: (id: RequestId) => void;
  onOpenAgent: (conversationId: ConversationId) => void;
  onOpenFile: (workspaceId: WorkspaceId, relativePath: string) => void;
  onStageSkill: (skillId: string) => void;
  onUnstageSkill: (skillId: string) => void;
  onOpenSectionsChange: (keys: ContextSectionKey[]) => void;
}

const LABELS: Record<ContextSectionKey, string> = {
  staged: 'Staged',
  skills: 'Skills',
  instructions: 'Instructions',
  memory: 'Memory',
  conversation: 'Conversation',
  observations: 'Workspace observations',
  agents: 'Agents',
  discoverable: 'Can discover',
  capabilities: 'Capabilities',
  usage: 'Usage and performance',
  health: 'Context health',
};

export function ContextDetail({
  catalog,
  snapshot,
  workspaceId,
  stagedSkillIds,
  openSections,
  historical = false,
  onSelectRequest,
  onOpenAgent,
  onOpenFile,
  onStageSkill,
  onUnstageSkill,
  onOpenSectionsChange,
}: ContextDetailProps) {
  const [exactOpen, setExactOpen] = useState(false);
  const exactTriggerRef = useRef<HTMLButtonElement>(null);
  const view = useMemo(
    () => selectContextView(snapshot, catalog, stagedSkillIds),
    [catalog, snapshot, stagedSkillIds],
  );
  const sectionKeys: ContextSectionKey[] = [
    ...(view.staged.length > 0 ? (['staged'] as const) : []),
    'skills',
    'instructions',
    'memory',
    'conversation',
    'observations',
    'agents',
    'discoverable',
    'capabilities',
    'usage',
    'health',
  ];
  const counts: Partial<Record<ContextSectionKey, number>> = {
    staged: view.staged.length,
    skills: view.sections.skills.length,
    instructions: view.sections.instructions.length,
    memory: view.sections.memory.length,
    observations: view.sections.observations.length,
    agents: view.sections.agents.length,
    discoverable: view.discoverable.skills.length,
    capabilities: view.sections.capabilities.length,
  };
  const items: ContextAccordionItem[] = sectionKeys.map((section) => ({
    key: section,
    label: LABELS[section],
    count: counts[section],
    warning:
      (section === 'health' && view.warnings.length > 0) ||
      (section === 'observations' && view.health.source_drift_count > 0),
    content: (
      <ContextSectionContent
        onOpenAgent={onOpenAgent}
        onOpenFile={onOpenFile}
        onStageSkill={onStageSkill}
        onUnstageSkill={onUnstageSkill}
        section={section}
        view={view}
        workspaceId={workspaceId}
      />
    ),
  }));

  return (
    <div className={styles.detail}>
      <div className="relative">
        <ContextSummary
          health={view.health}
          historical={historical}
          selectedRequest={view.selectedRequest}
          usage={view.usage.thisTask}
        />
        {view.exactRequest === null ? null : (
          <button
            aria-label="View exact Request"
            className="absolute bottom-2 right-3 inline-flex size-8 items-center justify-center rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
            onClick={() => {
              setExactOpen(true);
            }}
            ref={exactTriggerRef}
            title="View exact Request"
            type="button"
          >
            <Code2 aria-hidden="true" size={15} />
          </button>
        )}
      </div>
      <RequestHistory
        onSelect={onSelectRequest}
        requests={view.requestHistory}
        selectedRequestId={view.selectedRequest?.request_id ?? null}
        truncated={view.requestHistoryTruncated}
      />
      <ContextAccordion
        items={items}
        onOpenSectionsChange={onOpenSectionsChange}
        openSections={openSections}
      />
      {view.exactRequest === null ? null : (
        <ExactRequestDialog
          exactPayloadHash={view.exactPayloadHash}
          exactRequest={view.exactRequest}
          onClose={() => {
            setExactOpen(false);
          }}
          open={exactOpen}
          returnFocusRef={exactTriggerRef}
        />
      )}
    </div>
  );
}
