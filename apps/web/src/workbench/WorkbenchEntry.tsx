import type { ReactNode } from 'react';

import type { WorkspaceSummary } from '../api/generated';
import { EntryGate } from './entry/EntryGate';
import type { WorkbenchSnapshot } from './state';
import { WorkbenchShell } from './components/WorkbenchShell';

export interface WorkbenchEntryProps {
  chat: ReactNode;
  context: ReactNode;
  onOpenAgentThread: (taskId: string, conversationId: string) => void;
  onOpenContext: () => void;
  onOpenReview: (taskId: string) => void;
  onStop: () => void;
  stagedSkillCount: number;
  state: WorkbenchSnapshot;
  taskNavigation: ReactNode;
  workspace: WorkspaceSummary | null;
}

export function WorkbenchEntry(props: WorkbenchEntryProps) {
  return (
    <EntryGate
      renderWorkbench={(platformStatus) => (
        <WorkbenchShell
          chat={props.chat}
          context={props.context}
          onOpenContext={props.onOpenContext}
          onStop={props.onStop}
          platformStatus={platformStatus}
          stagedSkillCount={props.stagedSkillCount}
          state={props.state}
          taskNavigation={props.taskNavigation}
          workspace={props.workspace}
        />
      )}
    />
  );
}
