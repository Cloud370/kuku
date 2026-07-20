import type { ReactNode } from 'react';

import type { WorkspaceSummary } from '../api/generated';
import { EntryGate } from './entry/EntryGate';
import type { WorkbenchSnapshot } from './state';
import { WorkbenchShell } from './components/WorkbenchShell';

export interface WorkbenchEntryProps {
  chat: (callbacks: WorkbenchChatCallbacks) => ReactNode;
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

export interface WorkbenchChatCallbacks {
  onOpenAgentThread: (taskId: string, conversationId: string) => void;
  onOpenReview: (taskId: string) => void;
}

export function WorkbenchEntry(props: WorkbenchEntryProps) {
  return (
    <EntryGate
      renderWorkbench={(platformStatus) => (
        <WorkbenchShell
          chat={props.chat({
            onOpenAgentThread: props.onOpenAgentThread,
            onOpenReview: props.onOpenReview,
          })}
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
