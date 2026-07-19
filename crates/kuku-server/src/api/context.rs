use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    ApiVersion, CapabilityKind, CapabilityState, ContextBreakdown, ConversationId, DecimalCost,
    ExactRequest, InstructionKind, MemoryKind, MessageProjection, ObservationKind,
    ObservationRetention, ProviderFact, RequestCause, RequestId, RevisionToken, RunId,
    SkillLoadOrigin, SourceFact, TaskId, TaskRevision, TurnId, WorkspaceRelativePath,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TierSummary {
    pub tier_id: String,
    pub purpose: String,
    pub provider: String,
    pub model: String,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub think: Option<String>,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentSummary {
    pub agent_id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillContextItem {
    pub skill_id: String,
    pub name: String,
    pub description: String,
    pub source: SourceFact,
    pub origin: SkillLoadOrigin,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstructionContextItem {
    pub kind: InstructionKind,
    pub source: SourceFact,
    pub content_hash: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryContextItem {
    pub kind: MemoryKind,
    pub source: SourceFact,
    pub content_hash: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationContext {
    #[serde(deserialize_with = "json_safe_u64")]
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub retained_turns: u64,
    #[serde(deserialize_with = "json_safe_u64")]
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub handoff_boundaries: u64,
    pub history_summarized: bool,
    pub delegated_results: Vec<ConversationId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ObservationContextItem {
    pub request_id: RequestId,
    pub tool_call_id: String,
    pub kind: ObservationKind,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub relative_path: Option<WorkspaceRelativePath>,
    pub retention: ObservationRetention,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityProjection {
    pub kind: CapabilityKind,
    pub state: CapabilityState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextSections {
    pub skills: Vec<SkillContextItem>,
    pub instructions: Vec<InstructionContextItem>,
    pub memory: Vec<MemoryContextItem>,
    pub conversation: ConversationContext,
    pub observations: Vec<ObservationContextItem>,
    pub agents: Vec<DelegatedAgentProjection>,
    pub capabilities: Vec<CapabilityProjection>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextBreakdownProjection {
    pub facts: ContextBreakdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiscoverableContext {
    pub catalog_revision: RevisionToken,
    #[serde(deserialize_with = "json_safe_u64")]
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub skill_count: u64,
    #[serde(deserialize_with = "json_safe_u64")]
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub agent_count: u64,
    #[serde(deserialize_with = "json_safe_u64")]
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub tool_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageSummary {
    #[schemars(required, range(max = 9_007_199_254_740_991_u64))]
    #[serde(deserialize_with = "required_nullable_json_safe_u64")]
    pub input_tokens: Option<u64>,
    #[schemars(required, range(max = 9_007_199_254_740_991_u64))]
    #[serde(deserialize_with = "required_nullable_json_safe_u64")]
    pub output_tokens: Option<u64>,
    #[schemars(required, range(max = 9_007_199_254_740_991_u64))]
    #[serde(deserialize_with = "required_nullable_json_safe_u64")]
    pub cached_input_tokens: Option<u64>,
    #[schemars(required, range(max = 9_007_199_254_740_991_u64))]
    #[serde(deserialize_with = "required_nullable_json_safe_u64")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(deserialize_with = "json_safe_u64")]
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub request_count: u64,
    #[schemars(required, range(max = 9_007_199_254_740_991_u64))]
    #[serde(deserialize_with = "required_nullable_json_safe_u64")]
    pub elapsed_ms: Option<u64>,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub cost: Option<DecimalCost>,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub cached_input_ratio: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextUsage {
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub this_request: Option<UsageSummary>,
    pub this_task: UsageSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextHealthLevel {
    Unavailable,
    Healthy,
    Notice,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextHealth {
    pub level: ContextHealthLevel,
    #[schemars(required, range(max = 9_007_199_254_740_991_u64))]
    #[serde(deserialize_with = "required_nullable_json_safe_u64")]
    pub context_tokens_used: Option<u64>,
    #[schemars(required, range(max = 9_007_199_254_740_991_u64))]
    #[serde(deserialize_with = "required_nullable_json_safe_u64")]
    pub context_token_limit: Option<u64>,
    #[schemars(required, range(max = 9_007_199_254_740_991_u64))]
    #[serde(deserialize_with = "required_nullable_json_safe_u64")]
    pub context_tokens_remaining: Option<u64>,
    pub summarized: bool,
    #[serde(deserialize_with = "json_safe_u64")]
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub source_drift_count: u64,
    #[serde(deserialize_with = "json_safe_u64")]
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub truncated_observation_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextWarningCode {
    LowHeadroom,
    HistorySummarized,
    ObservationTruncated,
    SourceDrift,
    SourceInaccessible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextWarning {
    pub code: ContextWarningCode,
    pub summary: String,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub request_id: Option<RequestId>,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub source: Option<SourceFact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RequestStatus {
    Started,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestSummary {
    pub request_id: RequestId,
    pub run_id: RunId,
    pub turn_id: TurnId,
    pub conversation_id: ConversationId,
    pub status: RequestStatus,
    pub cause: RequestCause,
    pub provider: ProviderFact,
    pub model: String,
    pub started_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextSnapshot {
    pub api_version: ApiVersion,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub selected_request: Option<RequestSummary>,
    pub request_history: Vec<RequestSummary>,
    pub request_history_truncated: bool,
    pub sections: ContextSections,
    pub next_request_base: ContextBreakdown,
    pub discoverable: DiscoverableContext,
    pub usage: ContextUsage,
    pub health: ContextHealth,
    pub warnings: Vec<ContextWarning>,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub exact_request: Option<ExactRequest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextSummary {
    pub level: ContextHealthLevel,
    pub loaded_skill_count: u32,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub latest_request_id: Option<RequestId>,
    pub usage: UsageSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TierCatalogEntry {
    pub tier: TierSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillCatalogEntry {
    pub skill_id: String,
    pub name: String,
    pub description: String,
    pub source: SourceFact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentCatalogEntry {
    pub agent: AgentSummary,
    pub tier_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolCatalogEntry {
    pub tool_id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextCatalog {
    pub api_version: ApiVersion,
    pub revision: RevisionToken,
    pub tiers: Vec<TierCatalogEntry>,
    pub skills: Vec<SkillCatalogEntry>,
    pub agents: Vec<AgentCatalogEntry>,
    pub tools: Vec<ToolCatalogEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CatalogQuery {
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub search: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DelegatedAgentStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DelegatedAgentProjection {
    pub conversation_id: ConversationId,
    pub agent: AgentSummary,
    pub tier: TierSummary,
    pub status: DelegatedAgentStatus,
    pub result_in_main: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentThread {
    pub api_version: ApiVersion,
    pub task_id: TaskId,
    pub conversation_id: ConversationId,
    pub agent: AgentSummary,
    pub tier: TierSummary,
    pub status: DelegatedAgentStatus,
    pub result_in_main: bool,
    pub messages: Vec<MessageProjection>,
    pub messages_truncated_before: bool,
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

fn json_safe_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value <= 9_007_199_254_740_991 {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "integer exceeds JavaScript safe maximum",
        ))
    }
}

fn required_nullable_json_safe_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<u64>::deserialize(deserializer)? {
        Some(value) if value > 9_007_199_254_740_991 => Err(serde::de::Error::custom(
            "integer exceeds JavaScript safe maximum",
        )),
        value => Ok(value),
    }
}
