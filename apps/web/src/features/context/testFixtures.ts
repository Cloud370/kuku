import type { AgentThread, ApiError, ContextCatalog, ContextSnapshot } from '../../api/generated';

export const taskId = 'tsk_000000000000000000000001';
export const workspaceId = 'wsp_000000000000000000000001';
export const requestOne = 'req_000000000000000000000001';
export const requestTwo = 'req_000000000000000000000002';
export const conversationId = 'con_000000000000000000000002';

const source = {
  id: 'source:project:rust-review',
  relative_path: 'skills/rust-review/SKILL.md',
  scope: 'project' as const,
};

export function contextFixture(overrides: Partial<ContextSnapshot> = {}): ContextSnapshot {
  return {
    api_version: 1,
    task_id: taskId,
    task_revision: 12,
    selected_request: {
      request_id: requestTwo,
      run_id: 'run_000000000000000000000002',
      turn_id: 'trn_000000000000000000000002',
      conversation_id: 'con_000000000000000000000001',
      status: 'completed',
      cause: { kind: 'user_submission' },
      provider: { kind: 'anthropic' },
      model: 'claude-fixture',
      started_at: '2026-07-21T00:01:00Z',
    },
    request_history: [
      {
        request_id: requestOne,
        run_id: 'run_000000000000000000000001',
        turn_id: 'trn_000000000000000000000001',
        conversation_id: 'con_000000000000000000000001',
        status: 'completed',
        cause: { kind: 'user_submission' },
        provider: { kind: 'anthropic' },
        model: 'claude-fixture',
        started_at: '2026-07-21T00:00:00Z',
      },
      {
        request_id: requestTwo,
        run_id: 'run_000000000000000000000002',
        turn_id: 'trn_000000000000000000000002',
        conversation_id: 'con_000000000000000000000001',
        status: 'completed',
        cause: { kind: 'user_submission' },
        provider: { kind: 'anthropic' },
        model: 'claude-fixture',
        started_at: '2026-07-21T00:01:00Z',
      },
    ],
    request_history_truncated: false,
    exact_payload_hash: 'sha256:exact-request',
    sections: {
      skills: [
        {
          skill_id: 'skill:project:rust-review',
          name: 'Rust review',
          description: 'Reviews Rust changes safely.',
          source,
          origin: 'you',
          content_hash: 'sha256:skill',
        },
      ],
      instructions: [
        {
          kind: 'project',
          source: {
            ...source,
            id: 'source:project:instructions',
            relative_path: 'AGENTS.md',
          },
          content_hash: 'sha256:instructions',
          label: 'Project instructions',
        },
      ],
      memory: [
        {
          kind: 'project',
          source: {
            ...source,
            id: 'source:project:memory',
            relative_path: '.kuku/memory.md',
          },
          content_hash: 'sha256:memory',
          label: 'Project memory',
        },
      ],
      conversation: {
        retained_turns: 4,
        handoff_boundaries: 1,
        history_summarized: true,
        delegated_results: [conversationId],
      },
      observations: [
        {
          request_id: requestTwo,
          tool_call_id: 'tool-read',
          kind: { kind: 'file_read' },
          relative_path: 'src/lib.rs',
          retention: 'retained',
          current_drift: 'changed_since_observation',
          summary: 'Read the library entry point.',
        },
        {
          request_id: requestTwo,
          tool_call_id: 'tool-unsafe',
          kind: { kind: 'file_read' },
          relative_path: '/etc/passwd',
          retention: 'truncated',
          current_drift: 'inaccessible',
          summary: 'Untrusted absolute path fact.',
        },
      ],
      agents: [
        {
          conversation_id: conversationId,
          agent: {
            agent_id: 'agent:project:research',
            name: 'Research Agent',
            description: 'Investigates implementation details.',
          },
          tier: {
            tier_id: 'tier:balanced',
            label: 'Balanced',
            purpose: 'General work',
            provider: 'anthropic',
            model: 'claude-fixture',
            think: null,
            is_default: true,
          },
          status: 'completed',
          result_in_main: true,
        },
      ],
      capabilities: [
        { kind: 'file_read', state: 'available' },
        { kind: 'command_execution', state: 'requires_approval' },
      ],
    },
    next_request_base: {
      skills: [],
      instructions: [],
      memory: [],
      conversation: {
        retained_turns: 4,
        handoff_boundaries: 1,
        history_summarized: true,
        delegated_results: [conversationId],
      },
      observations: [],
      delegated_results: [],
      capabilities: [],
      token_estimate: 2_048,
    },
    discoverable: {
      catalog_revision: 'a'.repeat(64),
      skill_count: 2,
      agent_count: 1,
      tool_count: 1,
    },
    usage: {
      this_request: {
        input_tokens: 100,
        output_tokens: 20,
        cached_input_tokens: 40,
        cache_creation_input_tokens: null,
        request_count: 1,
        elapsed_ms: 1_200,
        cost: { currency: 'USD', micros: 1_500 },
        cached_input_ratio: 0.4,
      },
      this_task: {
        input_tokens: 800,
        output_tokens: 160,
        cached_input_tokens: 200,
        cache_creation_input_tokens: 60,
        request_count: 4,
        elapsed_ms: 8_000,
        cost: { currency: 'USD', micros: 8_500 },
        cached_input_ratio: 0.25,
      },
    },
    health: {
      level: 'warning',
      context_tokens_used: 2_048,
      context_token_limit: 8_192,
      context_tokens_remaining: 6_144,
      summarized: true,
      source_drift_count: 1,
      truncated_observation_count: 1,
    },
    warnings: [
      {
        code: 'history_summarized',
        summary: 'Conversation history was summarized.',
        request_id: requestTwo,
        source: null,
      },
      {
        code: 'source_drift',
        summary: 'An observed source changed.',
        request_id: requestTwo,
        source: null,
      },
    ],
    exact_request: {
      messages: [
        { role: 'system', content: [{ kind: 'text', text: 'System prompt' }] },
        { role: 'user', content: [{ kind: 'text', text: 'Review this change' }] },
      ],
      tools: [
        {
          name: 'read_file',
          description: 'Read a workspace file.',
          input_schema: { type: 'object', properties: { path: { type: 'string' } } },
        },
      ],
      parameters: {
        model: 'claude-fixture',
        max_output_tokens: 2_000,
        temperature: null,
        stream: true,
        thinking: { kind: 'disabled' },
      },
    },
    ...overrides,
  };
}

export function catalogFixture(): ContextCatalog {
  return {
    api_version: 1,
    revision: 'a'.repeat(64),
    tiers: [],
    agents: [],
    tools: [
      { tool_id: 'tool:read_file', name: 'read_file', description: 'Read a workspace file.' },
    ],
    skills: [
      {
        skill_id: 'skill:project:rust-review',
        name: 'Rust review',
        description: 'Reviews Rust changes safely.',
        source,
      },
      {
        skill_id: 'skill:project:docs',
        name: 'Documentation guide with an exceptionally long canonical name',
        description: 'Keeps documentation accurate.',
        source: { ...source, id: 'source:project:docs' },
      },
    ],
  };
}

export function agentThreadFixture(overrides: Partial<AgentThread> = {}): AgentThread {
  return {
    api_version: 1,
    task_id: taskId,
    conversation_id: conversationId,
    agent: {
      agent_id: 'agent:project:research',
      name: 'Research Agent',
      description: 'Investigates implementation details.',
    },
    tier: {
      tier_id: 'tier:balanced',
      label: 'Balanced',
      purpose: 'General work',
      provider: 'anthropic',
      model: 'claude-fixture',
      think: null,
      is_default: true,
    },
    status: 'completed',
    result_in_main: true,
    messages: [
      {
        message_id: 'msg-1',
        role: 'agent',
        text: 'Research result',
        finalized: true,
        request_ids: [requestOne, requestTwo],
        file_references: [],
        order_key: 1,
      },
    ],
    messages_truncated_before: false,
    ...overrides,
  };
}

export function apiErrorFixture(): ApiError {
  return {
    api_version: 1,
    code: 'request_not_found',
    message: 'Request not found',
    trace_id: 'context-read',
    details: null,
  };
}
