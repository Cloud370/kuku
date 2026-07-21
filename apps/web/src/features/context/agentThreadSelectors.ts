import type { AgentThread } from '../../api/generated';

export interface AgentThreadView {
  identity: string;
  status: AgentThread['status'];
  resultInMain: boolean;
  messages: AgentThread['messages'];
  messagesTruncatedBefore: boolean;
}

export function selectAgentThread(thread: AgentThread): AgentThreadView {
  return {
    identity: `${thread.agent.name} · ${thread.tier.label}`,
    status: thread.status,
    resultInMain: thread.result_in_main,
    messages: thread.messages,
    messagesTruncatedBefore: thread.messages_truncated_before,
  };
}
