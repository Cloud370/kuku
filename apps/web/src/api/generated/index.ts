/** Generated from the server-owned API schema. Do not edit. */

/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ApiVersion".
 */
export type ApiVersion = 1;
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ConversationId".
 */
export type ConversationId = string;
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "WorkspaceId".
 */
export type WorkspaceId = string;
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "Cursor".
 */
export type Cursor = number;
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "RequestId".
 */
export type RequestId = string;
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "MessageRole".
 */
export type MessageRole = "user" | "agent";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "DelegatedAgentStatus".
 */
export type DelegatedAgentStatus = "queued" | "running" | "completed" | "failed" | "interrupted";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TaskId".
 */
export type TaskId = string;
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TaskRevision".
 */
export type TaskRevision = number;
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "RevisionToken".
 */
export type RevisionToken = string;
/**
 * Identifies the content side anchored by a review annotation.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "AnnotationSide".
 */
export type AnnotationSide = "file" | "old" | "new";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ApiErrorCode".
 */
export type ApiErrorCode =
  | "invalid_request"
  | "auth_required"
  | "forbidden"
  | "origin_not_allowed"
  | "init_incomplete"
  | "stale_server_revision"
  | "workspace_not_found"
  | "workspace_in_use"
  | "workspace_unavailable"
  | "task_not_found"
  | "request_not_found"
  | "conversation_not_found"
  | "file_not_found"
  | "task_busy"
  | "stale_command"
  | "idempotency_conflict"
  | "run_not_active"
  | "interaction_not_pending"
  | "cursor_ahead"
  | "outdated"
  | "payload_too_large"
  | "unsupported_media_type"
  | "stream_limit"
  | "server_busy"
  | "provider_unavailable"
  | "storage_exhausted"
  | "internal";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "PageCursor".
 */
export type PageCursor = string;
/**
 * Ordered typed content sent to the provider.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ExactContentBlock".
 */
export type ExactContentBlock =
  | {
      kind: "text";
      /**
       * Exact text sent to the provider.
       */
      text: string;
      [k: string]: unknown;
    }
  | {
      kind: "thinking";
      /**
       * Exact thinking text.
       */
      text: string;
      [k: string]: unknown;
    }
  | {
      /**
       * Exact tool input value.
       */
      input: {
        [k: string]: unknown;
      };
      kind: "tool_use";
      /**
       * Registered tool name.
       */
      name: string;
      /**
       * Provider tool-call identity.
       */
      tool_call_id: string;
      [k: string]: unknown;
    }
  | {
      /**
       * Exact model-visible result content.
       */
      content: string;
      kind: "tool_result";
      /**
       * Tool execution status.
       */
      status: "completed" | "failed";
      /**
       * Optional structured result sent to the provider.
       */
      structured?: {
        [k: string]: unknown;
      };
      /**
       * Identity of the matching tool call.
       */
      tool_call_id: string;
      /**
       * Whether the result was truncated before transport.
       */
      truncated: boolean;
      [k: string]: unknown;
    };
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "Temperature".
 */
export type Temperature = number;
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ContextHealthLevel".
 */
export type ContextHealthLevel = "unavailable" | "healthy" | "notice" | "warning";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "WorkspaceRelativePath".
 */
export type WorkspaceRelativePath = string;
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "RunId".
 */
export type RunId = string;
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TurnId".
 */
export type TurnId = string;
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "RequestCause".
 */
export type RequestCause =
  | {
      kind: "user_submission";
      [k: string]: unknown;
    }
  | {
      kind: "tool_continuation";
      parent_request_id: RequestId;
      [k: string]: unknown;
    }
  | {
      interaction_id: InteractionId;
      kind: "interaction_resume";
      parent_request_id: RequestId;
      [k: string]: unknown;
    }
  | {
      kind: "delegated_agent";
      parent_request_id: RequestId;
      [k: string]: unknown;
    }
  | {
      kind: "review_submission";
      [k: string]: unknown;
    };
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "InteractionId".
 */
export type InteractionId = string;
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ProviderFact".
 */
export type ProviderFact =
  | {
      kind: "anthropic";
      [k: string]: unknown;
    }
  | {
      kind: "open_ai_compatible";
      [k: string]: unknown;
    }
  | {
      kind: "open_ai_responses";
      [k: string]: unknown;
    };
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "RequestStatus".
 */
export type RequestStatus = "started" | "completed" | "failed";
/**
 * Server-provided execution capability.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "CapabilityKind".
 */
export type CapabilityKind =
  | "file_read"
  | "file_write"
  | "command_execution"
  | "network_access"
  | "agent_delegation"
  | "skill_discovery"
  | "memory";
/**
 * Availability or permission state of a capability.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "CapabilityState".
 */
export type CapabilityState = "available" | "unavailable" | "requires_approval";
/**
 * Kind of instruction source included in a request.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "InstructionKind".
 */
export type InstructionKind = "system" | "project" | "workspace" | "agent";
/**
 * Scope of memory included in a request.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "MemoryKind".
 */
export type MemoryKind = "global" | "project";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ObservationDrift".
 */
export type ObservationDrift =
  | "present"
  | "changed_since_observation"
  | "no_longer_present"
  | "inaccessible"
  | "not_applicable";
/**
 * Kind and immutable metadata of a workspace observation.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ObservationKind".
 */
export type ObservationKind =
  | {
      kind: "file_read";
      [k: string]: unknown;
    }
  | {
      kind: "file_list";
      [k: string]: unknown;
    }
  | {
      kind: "search";
      /**
       * Exact search query.
       */
      query: string;
      [k: string]: unknown;
    }
  | {
      /**
       * Exact command invocation.
       */
      command: string;
      /**
       * Process exit code when available.
       */
      exit_code: number | null;
      kind: "command";
      [k: string]: unknown;
    }
  | {
      kind: "tool";
      /**
       * Registered tool name.
       */
      name: string;
      [k: string]: unknown;
    };
/**
 * Request-time retention state of an observation.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ObservationRetention".
 */
export type ObservationRetention = "retained" | "summarized" | "truncated";
/**
 * Origin responsible for loading a Skill.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "SkillLoadOrigin".
 */
export type SkillLoadOrigin = "you" | "agent" | "bootstrap" | "project";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "CurrencyCode".
 */
export type CurrencyCode = "USD";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ContextWarningCode".
 */
export type ContextWarningCode =
  | "low_headroom"
  | "history_summarized"
  | "observation_truncated"
  | "source_drift"
  | "source_inaccessible";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ChangesAvailability".
 */
export type ChangesAvailability = "available" | "not_git_repository" | "git_unavailable" | "workspace_root_mismatch";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ChangeKind".
 */
export type ChangeKind =
  | "added"
  | "modified"
  | "deleted"
  | "untracked"
  | "renamed"
  | "copied"
  | "type_changed"
  | "conflicted";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "RunState".
 */
export type RunState =
  | "queued"
  | "running"
  | "needs_attention"
  | "stopping"
  | "completed"
  | "stopped"
  | "failed"
  | "interrupted";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ReviewSubmissionId".
 */
export type ReviewSubmissionId = string;
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TaskState".
 */
export type TaskState =
  | "draft"
  | "queued"
  | "running"
  | "needs_attention"
  | "stopping"
  | "completed"
  | "stopped"
  | "failed"
  | "interrupted";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TimelineItemProjection".
 */
export type TimelineItemProjection =
  | {
      item: MessageProjection;
      type: "message";
      [k: string]: unknown;
    }
  | {
      item: ActivityProjection;
      type: "activity";
      [k: string]: unknown;
    }
  | {
      item: InteractionProjection;
      type: "interaction";
      [k: string]: unknown;
    };
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ActivityKind".
 */
export type ActivityKind = "tool" | "delegated_agent" | "system";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ActivityStatus".
 */
export type ActivityStatus = "pending" | "running" | "completed" | "failed";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "InteractionStatus".
 */
export type InteractionStatus = "pending" | "resolved" | "cancelled";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "DiffLineKind".
 */
export type DiffLineKind = "context" | "addition" | "deletion" | "no_newline_marker";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "FileKind".
 */
export type FileKind = "file" | "directory";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "InitPhase".
 */
export type InitPhase =
  | "required"
  | "providers_ready"
  | "default_tier_ready"
  | "workspace_ready"
  | "probe_passed"
  | "complete";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "CredentialSource".
 */
export type CredentialSource = "direct_value" | "environment_reference";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "AuthMode".
 */
export type AuthMode = "loopback_trusted" | "bearer";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "RegistrationRootId".
 */
export type RegistrationRootId = string;
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "AnnotationStatus".
 */
export type AnnotationStatus = "current" | "outdated";
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TaskDelta".
 */
export type TaskDelta =
  | {
      projection: TaskProjection;
      type: "projection_replaced";
      [k: string]: unknown;
    }
  | {
      changes: TaskChange[];
      timeline_window: TimelineWindowDelta | null;
      type: "changes_applied";
      [k: string]: unknown;
    };
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TaskChange".
 */
export type TaskChange =
  | {
      item: TimelineItemProjection;
      type: "message_appended";
      [k: string]: unknown;
    }
  | {
      append_text: string;
      finalized: boolean;
      message_id: string;
      request_ids: RequestId[] | null;
      type: "message_patched";
      [k: string]: unknown;
    }
  | {
      activity: ActivityProjection;
      type: "activity_upserted";
      [k: string]: unknown;
    }
  | {
      interaction: InteractionProjection;
      type: "interaction_upserted";
      [k: string]: unknown;
    }
  | {
      active_run: RunProjection | null;
      latest_run: RunProjection | null;
      task: TaskSummary;
      type: "run_state_changed";
      [k: string]: unknown;
    }
  | {
      loaded_skills: LoadedSkillProjection[];
      selected_tier_id: string;
      type: "skills_changed";
      [k: string]: unknown;
    }
  | {
      context_summary: ContextSummary | null;
      type: "context_summary_changed";
      [k: string]: unknown;
    }
  | {
      change: ReviewSubmissionsChanged;
      type: "review_submissions_changed";
      [k: string]: unknown;
    };
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "CredentialInput".
 */
export type CredentialInput =
  | {
      source: "direct_value";
      value: string;
      [k: string]: unknown;
    }
  | {
      source: "environment_reference";
      value: string;
      [k: string]: unknown;
    };
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "WorkspaceAvailability".
 */
export type WorkspaceAvailability = "available" | "missing" | "inaccessible";
/**
 * Role of an exact provider message.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "MessageRole2".
 */
export type MessageRole2 = "system" | "user" | "assistant" | "tool";
/**
 * Scope that owns or supplied a context source.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "SourceScope".
 */
export type SourceScope = "system" | "user" | "project" | "workspace" | "agent";
/**
 * Provider thinking configuration preserved in the exact request.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ThinkingConfig".
 */
export type ThinkingConfig =
  | {
      kind: "disabled";
      [k: string]: unknown;
    }
  | {
      kind: "adaptive";
      [k: string]: unknown;
    }
  | {
      /**
       * Provider thinking-token budget when configured.
       */
      budget_tokens: number | null;
      kind: "enabled";
      [k: string]: unknown;
    };
/**
 * Terminal state of a tool result content block.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ToolResultStatus".
 */
export type ToolResultStatus = "completed" | "failed";

export interface WebApiContract {
  agent_thread: AgentThread;
  annotation_batch: AnnotationBatch;
  api_error: ApiError;
  catalog_query: CatalogQuery;
  changes_query: ChangesQuery;
  command_accepted: CommandAccepted;
  complete_init: CompleteInitRequest;
  connection_info: ConnectionInfo;
  context_snapshot: ContextSnapshot;
  create_task: CreateTaskRequest;
  create_task_result: CreateTaskResponse;
  diff_document: DiffDocument;
  diff_query: DiffQuery;
  file_content: FileContent;
  file_content_query: FileContentQuery;
  file_page: FilePage;
  file_search_page: FileSearchPage;
  file_search_query: FileSearchQuery;
  file_tree_query: FileTreeQuery;
  init_status: InitStatus;
  interaction_response: InteractionResponseRequest;
  list_tasks: ListTasksQuery;
  platform_catalog: PlatformCatalog;
  platform_status: PlatformStatus;
  register_initial_workspace: RegisterInitialWorkspaceRequest;
  register_workspace: RegisterWorkspaceRequest;
  registration_root_page: RegistrationRootPage;
  remove_workspace: RemoveWorkspaceRequest;
  review_snapshot: ReviewSnapshot;
  review_submission_page: ReviewSubmissionPage;
  review_submission_query: ReviewSubmissionQuery;
  review_submission_result: ReviewSubmissionResult;
  settings: SettingsSnapshot;
  stop_run: StopRunRequest;
  submit_run: SubmitRunRequest;
  submit_run_result: SubmitRunResponse;
  task_page: TaskPage;
  task_projection: TaskProjection;
  task_stream_event: TaskStreamEvent;
  task_stream_query: TaskStreamQuery;
  test_provider: TestProviderRequest;
  test_provider_result: TestProviderResult;
  timeline_page: TimelinePage;
  timeline_query: TimelineQuery;
  update_default_tier: UpdateDefaultTierRequest;
  update_providers: UpdateProvidersRequest;
  update_settings: UpdateSettingsRequest;
  workspace_catalog: ContextCatalog;
  workspace_page: WorkspacePage;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "AgentThread".
 */
export interface AgentThread {
  agent: AgentSummary;
  api_version: ApiVersion;
  conversation_id: ConversationId;
  messages: MessageProjection[];
  messages_truncated_before: boolean;
  result_in_main: boolean;
  status: DelegatedAgentStatus;
  task_id: TaskId;
  tier: TierSummary;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "AgentSummary".
 */
export interface AgentSummary {
  agent_id: string;
  description: string;
  name: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "MessageProjection".
 */
export interface MessageProjection {
  file_references: FileReferenceProjection[];
  finalized: boolean;
  message_id: string;
  order_key: Cursor;
  request_ids: RequestId[];
  role: MessageRole;
  text: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "FileReferenceProjection".
 */
export interface FileReferenceProjection {
  label: string;
  relative_path: string;
  workspace_id: WorkspaceId;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TierSummary".
 */
export interface TierSummary {
  is_default: boolean;
  label: string;
  model: string;
  provider: string;
  purpose: string;
  think: string | null;
  tier_id: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "AnnotationBatch".
 */
export interface AnnotationBatch {
  expected_task_revision: TaskRevision;
  idempotency_key: string;
  notes: AnnotationDraft[];
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "AnnotationDraft".
 */
export interface AnnotationDraft {
  comment: string;
  end_line: number;
  excerpt: string;
  path: string;
  revision: RevisionToken;
  side: AnnotationSide;
  start_line: number;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ApiError".
 */
export interface ApiError {
  api_version: ApiVersion;
  code: ApiErrorCode;
  details: unknown;
  message: string;
  trace_id: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "CatalogQuery".
 */
export interface CatalogQuery {
  search: string | null;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ChangesQuery".
 */
export interface ChangesQuery {
  cursor: PageCursor | null;
  limit: number;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "CommandAccepted".
 */
export interface CommandAccepted {
  api_version: ApiVersion;
  replayed: boolean;
  task_id: TaskId;
  task_revision: TaskRevision;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "CompleteInitRequest".
 */
export interface CompleteInitRequest {
  expected_revision: RevisionToken;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ConnectionInfo".
 */
export interface ConnectionInfo {
  display_name: string;
  lan_url: string | null;
  local_url: string;
  plaintext: boolean;
  preferred_origin: string;
  server_id: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ContextSnapshot".
 */
export interface ContextSnapshot {
  api_version: ApiVersion;
  discoverable: DiscoverableContext;
  exact_payload_hash: string | null;
  exact_request: ExactRequest | null;
  health: ContextHealth;
  next_request_base: ContextBreakdown;
  request_history: RequestSummary[];
  request_history_truncated: boolean;
  sections: ContextSections;
  selected_request: RequestSummary | null;
  task_id: TaskId;
  task_revision: TaskRevision;
  usage: ContextUsage;
  warnings: ContextWarning[];
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "DiscoverableContext".
 */
export interface DiscoverableContext {
  agent_count: number;
  catalog_revision: RevisionToken;
  skill_count: number;
  tool_count: number;
  [k: string]: unknown;
}
/**
 * Ordered exact messages, tools, and allowlisted request parameters.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ExactRequest".
 */
export interface ExactRequest {
  /**
   * Messages in provider order, including system content.
   */
  messages: ExactMessage[];
  parameters: ExactRequestParameters;
  /**
   * Tool definitions in provider order.
   */
  tools: ExactTool[];
  [k: string]: unknown;
}
/**
 * One exact provider message with ordered content blocks.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ExactMessage".
 */
export interface ExactMessage {
  /**
   * Content blocks in provider order.
   */
  content: ExactContentBlock[];
  /**
   * Message role.
   */
  role: "system" | "user" | "assistant" | "tool";
  [k: string]: unknown;
}
/**
 * Non-secret provider parameters.
 */
export interface ExactRequestParameters {
  /**
   * Maximum output tokens when configured.
   */
  max_output_tokens: number | null;
  /**
   * Resolved provider model.
   */
  model: string;
  /**
   * Whether the provider response is streamed.
   */
  stream: boolean;
  /**
   * Finite sampling temperature when configured.
   */
  temperature: Temperature | null;
  /**
   * Resolved thinking configuration.
   */
  thinking:
    | {
        kind: "disabled";
        [k: string]: unknown;
      }
    | {
        kind: "adaptive";
        [k: string]: unknown;
      }
    | {
        /**
         * Provider thinking-token budget when configured.
         */
        budget_tokens: number | null;
        kind: "enabled";
        [k: string]: unknown;
      };
  [k: string]: unknown;
}
/**
 * Exact provider-visible tool definition.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ExactTool".
 */
export interface ExactTool {
  /**
   * Tool description sent to the provider.
   */
  description: string;
  /**
   * Tool input JSON Schema.
   */
  input_schema: {
    [k: string]: unknown;
  };
  /**
   * Tool name.
   */
  name: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ContextHealth".
 */
export interface ContextHealth {
  context_token_limit: number | null;
  context_tokens_remaining: number | null;
  context_tokens_used: number | null;
  level: ContextHealthLevel;
  source_drift_count: number;
  summarized: boolean;
  truncated_observation_count: number;
  [k: string]: unknown;
}
/**
 * Structured sources and capabilities included in a request context.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ContextBreakdown".
 */
export interface ContextBreakdown {
  /**
   * Effective execution capabilities.
   */
  capabilities: CapabilityFact[];
  conversation: ConversationContextFact;
  /**
   * Delegated results included in the request.
   */
  delegated_results: DelegatedResultFact[];
  /**
   * Included instructions.
   */
  instructions: InstructionContextFact[];
  /**
   * Included memory sources.
   */
  memory: MemoryContextFact[];
  /**
   * Workspace observations available to the request.
   */
  observations: ObservationFact[];
  /**
   * Loaded Skills.
   */
  skills: SkillContextFact[];
  /**
   * Server-provided input token estimate when available.
   */
  token_estimate: number | null;
  [k: string]: unknown;
}
/**
 * Capability and its request-time state.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "CapabilityFact".
 */
export interface CapabilityFact {
  /**
   * Capability category.
   */
  kind:
    | "file_read"
    | "file_write"
    | "command_execution"
    | "network_access"
    | "agent_delegation"
    | "skill_discovery"
    | "memory";
  /**
   * Request-time capability state.
   */
  state: "available" | "unavailable" | "requires_approval";
  [k: string]: unknown;
}
/**
 * Conversation-history composition.
 */
export interface ConversationContextFact {
  /**
   * Delegated conversations whose results entered this history.
   */
  delegated_results: ConversationId[];
  /**
   * Number of handoff boundaries represented in the history.
   */
  handoff_boundaries: number;
  /**
   * Whether earlier history has been summarized.
   */
  history_summarized: boolean;
  /**
   * Number of retained turns.
   */
  retained_turns: number;
  [k: string]: unknown;
}
/**
 * Delegated Agent result included in the main request context.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "DelegatedResultFact".
 */
export interface DelegatedResultFact {
  /**
   * Stable Agent catalog identifier.
   */
  agent_id: string;
  /**
   * Hash of the included result content.
   */
  content_hash: string;
  /**
   * Delegated conversation identity.
   */
  conversation_id: string;
  [k: string]: unknown;
}
/**
 * Instruction source included in a request context.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "InstructionContextFact".
 */
export interface InstructionContextFact {
  /**
   * Hash of the exact included content.
   */
  content_hash: string;
  /**
   * Instruction category.
   */
  kind: "system" | "project" | "workspace" | "agent";
  source: SourceFact;
  [k: string]: unknown;
}
/**
 * Instruction source.
 */
export interface SourceFact {
  /**
   * Stable source identifier.
   */
  id: string;
  /**
   * Contained workspace-relative path when the source is file-backed.
   */
  relative_path: WorkspaceRelativePath | null;
  /**
   * Ownership scope of the source.
   */
  scope: "system" | "user" | "project" | "workspace" | "agent";
  [k: string]: unknown;
}
/**
 * Memory source included in a request context.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "MemoryContextFact".
 */
export interface MemoryContextFact {
  /**
   * Hash of the exact included content.
   */
  content_hash: string;
  /**
   * Memory category.
   */
  kind: "global" | "project";
  source: SourceFact1;
  [k: string]: unknown;
}
/**
 * Memory source.
 */
export interface SourceFact1 {
  /**
   * Stable source identifier.
   */
  id: string;
  /**
   * Contained workspace-relative path when the source is file-backed.
   */
  relative_path: WorkspaceRelativePath | null;
  /**
   * Ownership scope of the source.
   */
  scope: "system" | "user" | "project" | "workspace" | "agent";
  [k: string]: unknown;
}
/**
 * Immutable observation recorded for one provider request.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ObservationFact".
 */
export interface ObservationFact {
  /**
   * Observation category and metadata.
   */
  kind:
    | {
        kind: "file_read";
        [k: string]: unknown;
      }
    | {
        kind: "file_list";
        [k: string]: unknown;
      }
    | {
        kind: "search";
        /**
         * Exact search query.
         */
        query: string;
        [k: string]: unknown;
      }
    | {
        /**
         * Exact command invocation.
         */
        command: string;
        /**
         * Process exit code when available.
         */
        exit_code: number | null;
        kind: "command";
        [k: string]: unknown;
      }
    | {
        kind: "tool";
        /**
         * Registered tool name.
         */
        name: string;
        [k: string]: unknown;
      };
  /**
   * Hash of the observed content when available.
   */
  observed_hash: string | null;
  /**
   * Observed line range when applicable.
   */
  range: ObservedRange | null;
  /**
   * Contained workspace-relative path when applicable.
   */
  relative_path: WorkspaceRelativePath | null;
  /**
   * Request-time retention state.
   */
  retention: "retained" | "summarized" | "truncated";
  scope: RequestScope;
  /**
   * Safe compact description of the observation.
   */
  summary: string;
  /**
   * Tool-call identity that produced the observation.
   */
  tool_call_id: string;
  [k: string]: unknown;
}
/**
 * One-based inclusive line range observed in a workspace file.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ObservedRange".
 */
export interface ObservedRange {
  /**
   * Last observed line.
   */
  end_line: number;
  /**
   * First observed line.
   */
  start_line: number;
  [k: string]: unknown;
}
/**
 * Request that observed the value.
 */
export interface RequestScope {
  execution: ExecutionScope;
  request_id: RequestId;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ExecutionScope".
 */
export interface ExecutionScope {
  conversation_id: ConversationId;
  run_id: RunId;
  task_id: TaskId;
  turn_id: TurnId;
  turn_index: number;
  workspace_id: WorkspaceId;
  [k: string]: unknown;
}
/**
 * Skill content included in a request context.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "SkillContextFact".
 */
export interface SkillContextFact {
  /**
   * Hash of the exact loaded Skill content.
   */
  content_hash: string;
  /**
   * Origin that loaded the Skill.
   */
  origin: "you" | "agent" | "bootstrap" | "project";
  /**
   * Stable catalog Skill identifier.
   */
  skill_id: string;
  source: SourceFact2;
  [k: string]: unknown;
}
/**
 * Skill source.
 */
export interface SourceFact2 {
  /**
   * Stable source identifier.
   */
  id: string;
  /**
   * Contained workspace-relative path when the source is file-backed.
   */
  relative_path: WorkspaceRelativePath | null;
  /**
   * Ownership scope of the source.
   */
  scope: "system" | "user" | "project" | "workspace" | "agent";
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "RequestSummary".
 */
export interface RequestSummary {
  cause: RequestCause;
  conversation_id: ConversationId;
  model: string;
  provider: ProviderFact;
  request_id: RequestId;
  run_id: RunId;
  started_at: string;
  status: RequestStatus;
  turn_id: TurnId;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ContextSections".
 */
export interface ContextSections {
  agents: DelegatedAgentProjection[];
  capabilities: CapabilityProjection[];
  conversation: ConversationContext;
  instructions: InstructionContextItem[];
  memory: MemoryContextItem[];
  observations: ObservationContextItem[];
  skills: SkillContextItem[];
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "DelegatedAgentProjection".
 */
export interface DelegatedAgentProjection {
  agent: AgentSummary;
  conversation_id: ConversationId;
  result_in_main: boolean;
  status: DelegatedAgentStatus;
  tier: TierSummary;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "CapabilityProjection".
 */
export interface CapabilityProjection {
  kind: CapabilityKind;
  state: CapabilityState;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ConversationContext".
 */
export interface ConversationContext {
  delegated_results: ConversationId[];
  handoff_boundaries: number;
  history_summarized: boolean;
  retained_turns: number;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "InstructionContextItem".
 */
export interface InstructionContextItem {
  content_hash: string;
  kind: InstructionKind;
  label: string;
  source: SourceFact3;
  [k: string]: unknown;
}
/**
 * Stable identity and optional contained path for a context source.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "SourceFact".
 */
export interface SourceFact3 {
  /**
   * Stable source identifier.
   */
  id: string;
  /**
   * Contained workspace-relative path when the source is file-backed.
   */
  relative_path: WorkspaceRelativePath | null;
  /**
   * Ownership scope of the source.
   */
  scope: "system" | "user" | "project" | "workspace" | "agent";
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "MemoryContextItem".
 */
export interface MemoryContextItem {
  content_hash: string;
  kind: MemoryKind;
  label: string;
  source: SourceFact3;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ObservationContextItem".
 */
export interface ObservationContextItem {
  current_drift: ObservationDrift;
  kind: ObservationKind;
  relative_path: WorkspaceRelativePath | null;
  request_id: RequestId;
  retention: ObservationRetention;
  summary: string;
  tool_call_id: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "SkillContextItem".
 */
export interface SkillContextItem {
  content_hash: string;
  description: string;
  name: string;
  origin: SkillLoadOrigin;
  skill_id: string;
  source: SourceFact3;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ContextUsage".
 */
export interface ContextUsage {
  this_request: UsageSummary | null;
  this_task: UsageSummary;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "UsageSummary".
 */
export interface UsageSummary {
  cache_creation_input_tokens: number | null;
  cached_input_ratio: number | null;
  cached_input_tokens: number | null;
  cost: DecimalCost | null;
  elapsed_ms: number | null;
  input_tokens: number | null;
  output_tokens: number | null;
  request_count: number;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "DecimalCost".
 */
export interface DecimalCost {
  currency: CurrencyCode;
  micros: number;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ContextWarning".
 */
export interface ContextWarning {
  code: ContextWarningCode;
  request_id: RequestId | null;
  source: SourceFact3 | null;
  summary: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "CreateTaskRequest".
 */
export interface CreateTaskRequest {
  idempotency_key: string;
  workspace_id: WorkspaceId;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "CreateTaskResponse".
 */
export interface CreateTaskResponse {
  api_version: ApiVersion;
  projection: TaskProjection;
  replayed: boolean;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TaskProjection".
 */
export interface TaskProjection {
  active_run: RunProjection | null;
  api_version: ApiVersion;
  context_summary: ContextSummary | null;
  cursor: Cursor;
  latest_run: RunProjection | null;
  loaded_skills: LoadedSkillProjection[];
  review_summary: ReviewSummaryProjection;
  selected_tier_id: string;
  task: TaskSummary;
  task_revision: TaskRevision;
  timeline: TimelineItemProjection[];
  timeline_next_cursor: PageCursor | null;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "RunProjection".
 */
export interface RunProjection {
  completion: CompletionProjection | null;
  finished_at: string | null;
  run_id: RunId;
  started_at: string;
  state: RunState;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "CompletionProjection".
 */
export interface CompletionProjection {
  checks: CheckProjection[] | null;
  metrics: MetricProjection[] | null;
  summary: string;
  warnings: string[];
  workspace_changes: ReviewSnapshot | null;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "CheckProjection".
 */
export interface CheckProjection {
  detail: string | null;
  name: string;
  passed: boolean;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "MetricProjection".
 */
export interface MetricProjection {
  name: string;
  unit: string | null;
  value: number;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ReviewSnapshot".
 */
export interface ReviewSnapshot {
  api_version: ApiVersion;
  availability: ChangesAvailability;
  entries: ChangeEntry[];
  next_cursor: PageCursor | null;
  revision: RevisionToken;
  workspace_id: WorkspaceId;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ChangeEntry".
 */
export interface ChangeEntry {
  additions: number | null;
  binary: boolean;
  deletions: number | null;
  kind: ChangeKind;
  old_path: string | null;
  path: string;
  revision: RevisionToken;
  staged: boolean;
  worktree: boolean;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ContextSummary".
 */
export interface ContextSummary {
  latest_request_id: RequestId | null;
  level: ContextHealthLevel;
  loaded_skill_count: number;
  usage: UsageSummary;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "LoadedSkillProjection".
 */
export interface LoadedSkillProjection {
  description: string;
  loaded_by: string;
  name: string;
  skill_id: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ReviewSummaryProjection".
 */
export interface ReviewSummaryProjection {
  latest_submission_id: ReviewSubmissionId | null;
  total_submissions: number;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TaskSummary".
 */
export interface TaskSummary {
  active_run_id: RunId | null;
  latest_run_id: RunId | null;
  state: TaskState;
  task_id: TaskId;
  title: string;
  updated_at: string;
  workspace_id: WorkspaceId;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ActivityProjection".
 */
export interface ActivityProjection {
  activity_id: string;
  detail: string | null;
  file_references: FileReferenceProjection[];
  kind: ActivityKind;
  order_key: Cursor;
  status: ActivityStatus;
  title: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "InteractionProjection".
 */
export interface InteractionProjection {
  choices: InteractionChoiceProjection[];
  interaction_id: InteractionId;
  order_key: Cursor;
  prompt: string;
  selected_choice_id: string | null;
  status: InteractionStatus;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "InteractionChoiceProjection".
 */
export interface InteractionChoiceProjection {
  choice_id: string;
  label: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "DiffDocument".
 */
export interface DiffDocument {
  api_version: ApiVersion;
  binary: boolean;
  hunks: DiffHunk[];
  next_cursor: PageCursor | null;
  old_path: string | null;
  path: string;
  revision: RevisionToken;
  truncated: boolean;
  workspace_id: WorkspaceId;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "DiffHunk".
 */
export interface DiffHunk {
  lines: DiffLine[];
  new_lines: number;
  new_start: number;
  old_lines: number;
  old_start: number;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "DiffLine".
 */
export interface DiffLine {
  kind: DiffLineKind;
  new_line: number | null;
  old_line: number | null;
  text: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "DiffQuery".
 */
export interface DiffQuery {
  cursor: PageCursor | null;
  limit: number;
  path: string;
  revision: RevisionToken;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "FileContent".
 */
export interface FileContent {
  api_version: ApiVersion;
  binary: boolean;
  end_line: number;
  next_start_line: number | null;
  path: string;
  revision: RevisionToken;
  start_line: number;
  text: string | null;
  total_lines: number | null;
  truncated: boolean;
  workspace_id: WorkspaceId;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "FileContentQuery".
 */
export interface FileContentQuery {
  end_line: number;
  path: string;
  start_line: number;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "FilePage".
 */
export interface FilePage {
  api_version: ApiVersion;
  entries: FileEntry[];
  next_cursor: PageCursor | null;
  revision: RevisionToken;
  workspace_id: WorkspaceId;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "FileEntry".
 */
export interface FileEntry {
  binary: boolean;
  change: ChangeKind | null;
  kind: FileKind;
  name: string;
  path: string;
  revision: RevisionToken | null;
  size_bytes: number | null;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "FileSearchPage".
 */
export interface FileSearchPage {
  api_version: ApiVersion;
  matches: SearchMatch[];
  next_cursor: PageCursor | null;
  revision: RevisionToken;
  workspace_id: WorkspaceId;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "SearchMatch".
 */
export interface SearchMatch {
  entry: FileEntry;
  path_match_ranges: TextRange[];
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TextRange".
 */
export interface TextRange {
  end: number;
  start: number;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "FileSearchQuery".
 */
export interface FileSearchQuery {
  cursor: PageCursor | null;
  limit: number;
  prefix: string;
  query: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "FileTreeQuery".
 */
export interface FileTreeQuery {
  cursor: PageCursor | null;
  limit: number;
  prefix: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "InitStatus".
 */
export interface InitStatus {
  api_version: ApiVersion;
  complete: boolean;
  default_tier_configured: boolean;
  phase: InitPhase;
  provider_test_passed: boolean;
  providers_configured: boolean;
  server_revision: RevisionToken;
  workspace_registered: boolean;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "InteractionResponseRequest".
 */
export interface InteractionResponseRequest {
  choice_id: string;
  expected_task_revision: TaskRevision;
  idempotency_key: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ListTasksQuery".
 */
export interface ListTasksQuery {
  cursor: PageCursor | null;
  limit: number;
  search: string | null;
  workspace_id: WorkspaceId;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "PlatformCatalog".
 */
export interface PlatformCatalog {
  api_version: ApiVersion;
  credentials: CredentialStatus[];
  default_tier: TierSummary;
  revision: RevisionToken;
  tiers: TierSummary[];
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "CredentialStatus".
 */
export interface CredentialStatus {
  environment_reference: string | null;
  present: boolean;
  provider_id: string;
  source: CredentialSource | null;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "PlatformStatus".
 */
export interface PlatformStatus {
  api_version: ApiVersion;
  auth: AuthStatus;
  connection: ConnectionInfo;
  init: InitStatus;
  ready: boolean;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "AuthStatus".
 */
export interface AuthStatus {
  authenticated: boolean;
  mode: AuthMode;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "RegisterInitialWorkspaceRequest".
 */
export interface RegisterInitialWorkspaceRequest {
  workspace: RegisterWorkspaceRequest;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "RegisterWorkspaceRequest".
 */
export interface RegisterWorkspaceRequest {
  expected_revision: RevisionToken;
  label: string;
  relative_path: string;
  root_id: RegistrationRootId;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "RegistrationRootPage".
 */
export interface RegistrationRootPage {
  api_version: ApiVersion;
  items: RegistrationRootSummary[];
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "RegistrationRootSummary".
 */
export interface RegistrationRootSummary {
  label: string;
  root_id: RegistrationRootId;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "RemoveWorkspaceRequest".
 */
export interface RemoveWorkspaceRequest {
  expected_revision: RevisionToken;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ReviewSubmissionPage".
 */
export interface ReviewSubmissionPage {
  api_version: ApiVersion;
  items: ReviewSubmissionProjection[];
  next_cursor: PageCursor | null;
  task_id: TaskId;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ReviewSubmissionProjection".
 */
export interface ReviewSubmissionProjection {
  notes: SubmittedReviewNote[];
  run_id: RunId;
  submission_id: ReviewSubmissionId;
  submitted_at: string;
  task_id: TaskId;
  task_revision: TaskRevision;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "SubmittedReviewNote".
 */
export interface SubmittedReviewNote {
  comment: string;
  end_line: number;
  excerpt: string;
  path: string;
  revision: RevisionToken;
  side: AnnotationSide;
  start_line: number;
  status: AnnotationStatus;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ReviewSubmissionQuery".
 */
export interface ReviewSubmissionQuery {
  cursor: PageCursor | null;
  limit: number;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ReviewSubmissionResult".
 */
export interface ReviewSubmissionResult {
  api_version: ApiVersion;
  replayed: boolean;
  submission: ReviewSubmissionProjection;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "SettingsSnapshot".
 */
export interface SettingsSnapshot {
  api_version: ApiVersion;
  credentials: CredentialStatus[];
  default_tier: string;
  default_workspace_id: WorkspaceId | null;
  discovery: DiscoverySettings;
  max_concurrent_runs: number;
  server_revision: RevisionToken;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "DiscoverySettings".
 */
export interface DiscoverySettings {
  auto_discover: boolean;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "StopRunRequest".
 */
export interface StopRunRequest {
  expected_task_revision: TaskRevision;
  idempotency_key: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "SubmitRunRequest".
 */
export interface SubmitRunRequest {
  expected_task_revision: TaskRevision;
  idempotency_key: string;
  message: string;
  skill_ids: string[];
  tier_id: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "SubmitRunResponse".
 */
export interface SubmitRunResponse {
  api_version: ApiVersion;
  replayed: boolean;
  run_id: RunId;
  task_id: TaskId;
  task_revision: TaskRevision;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TaskPage".
 */
export interface TaskPage {
  api_version: ApiVersion;
  items: TaskSummary[];
  next_cursor: PageCursor | null;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TaskStreamEvent".
 */
export interface TaskStreamEvent {
  api_version: ApiVersion;
  cursor: Cursor;
  event: TaskDelta;
  task_id: TaskId;
  task_revision: TaskRevision;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ReviewSubmissionsChanged".
 */
export interface ReviewSubmissionsChanged {
  submission: ReviewSubmissionProjection;
  total_submissions: number;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TimelineWindowDelta".
 */
export interface TimelineWindowDelta {
  evicted_items: TimelineItemProjection[];
  next_cursor: PageCursor | null;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TaskStreamQuery".
 */
export interface TaskStreamQuery {
  after: Cursor | null;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TestProviderRequest".
 */
export interface TestProviderRequest {
  expected_revision: RevisionToken;
  tier_id: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TestProviderResult".
 */
export interface TestProviderResult {
  api_version: ApiVersion;
  message: string | null;
  model: string;
  provider: string;
  reachable: boolean;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TimelinePage".
 */
export interface TimelinePage {
  api_version: ApiVersion;
  items: TimelineItemProjection[];
  next_cursor: PageCursor | null;
  task_id: TaskId;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TimelineQuery".
 */
export interface TimelineQuery {
  before: PageCursor | null;
  limit: number;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "UpdateDefaultTierRequest".
 */
export interface UpdateDefaultTierRequest {
  expected_revision: RevisionToken;
  tier_id: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "UpdateProvidersRequest".
 */
export interface UpdateProvidersRequest {
  expected_revision: RevisionToken;
  providers: ProviderDraft[];
  tiers: TierDraft[];
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ProviderDraft".
 */
export interface ProviderDraft {
  base_url: string;
  credential: CredentialInput;
  format: string;
  provider_id: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TierDraft".
 */
export interface TierDraft {
  model: string;
  provider_id: string;
  purpose: string;
  think: string | null;
  tier_id: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "UpdateSettingsRequest".
 */
export interface UpdateSettingsRequest {
  expected_revision: RevisionToken;
  patch: SettingsPatch;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "SettingsPatch".
 */
export interface SettingsPatch {
  default_tier: string | null;
  default_workspace_id: WorkspaceId | null;
  discovery: DiscoverySettings | null;
  max_concurrent_runs: number | null;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ContextCatalog".
 */
export interface ContextCatalog {
  agents: AgentCatalogEntry[];
  api_version: ApiVersion;
  revision: RevisionToken;
  skills: SkillCatalogEntry[];
  tiers: TierCatalogEntry[];
  tools: ToolCatalogEntry[];
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "AgentCatalogEntry".
 */
export interface AgentCatalogEntry {
  agent: AgentSummary;
  tier_id: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "SkillCatalogEntry".
 */
export interface SkillCatalogEntry {
  description: string;
  name: string;
  skill_id: string;
  source: SourceFact3;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "TierCatalogEntry".
 */
export interface TierCatalogEntry {
  tier: TierSummary;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ToolCatalogEntry".
 */
export interface ToolCatalogEntry {
  description: string;
  name: string;
  tool_id: string;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "WorkspacePage".
 */
export interface WorkspacePage {
  api_version: ApiVersion;
  items: WorkspaceSummary[];
  server_revision: RevisionToken;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "WorkspaceSummary".
 */
export interface WorkspaceSummary {
  availability: WorkspaceAvailability;
  branch: string | null;
  is_default: boolean;
  label: string;
  workspace_id: WorkspaceId;
  [k: string]: unknown;
}
/**
 * Conversation history composition included in a request.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ConversationContextFact".
 */
export interface ConversationContextFact1 {
  /**
   * Delegated conversations whose results entered this history.
   */
  delegated_results: ConversationId[];
  /**
   * Number of handoff boundaries represented in the history.
   */
  handoff_boundaries: number;
  /**
   * Whether earlier history has been summarized.
   */
  history_summarized: boolean;
  /**
   * Number of retained turns.
   */
  retained_turns: number;
  [k: string]: unknown;
}
/**
 * Allowlisted non-secret parameters sent with an exact provider request.
 *
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "ExactRequestParameters".
 */
export interface ExactRequestParameters1 {
  /**
   * Maximum output tokens when configured.
   */
  max_output_tokens: number | null;
  /**
   * Resolved provider model.
   */
  model: string;
  /**
   * Whether the provider response is streamed.
   */
  stream: boolean;
  /**
   * Finite sampling temperature when configured.
   */
  temperature: Temperature | null;
  /**
   * Resolved thinking configuration.
   */
  thinking:
    | {
        kind: "disabled";
        [k: string]: unknown;
      }
    | {
        kind: "adaptive";
        [k: string]: unknown;
      }
    | {
        /**
         * Provider thinking-token budget when configured.
         */
        budget_tokens: number | null;
        kind: "enabled";
        [k: string]: unknown;
      };
  [k: string]: unknown;
}
/**
 * This interface was referenced by `WebApiContract`'s JSON-Schema
 * via the `definition` "RequestScope".
 */
export interface RequestScope1 {
  execution: ExecutionScope;
  request_id: RequestId;
  [k: string]: unknown;
}
