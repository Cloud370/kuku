//! Durable, provider-independent facts describing one request's exact context.
#![deny(missing_docs)]

use std::borrow::Cow;
use std::fmt;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{
    ConversationId, ExecutionScope, ProviderFact, RequestCause, RequestId, RequestScope,
    RevisionToken,
};

/// Maximum serialized UTF-8 byte length of a workspace-relative path.
pub const MAX_WORKSPACE_RELATIVE_PATH_BYTES: usize = 4_096;

/// Failure to validate a canonical workspace-relative path.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceRelativePathError {
    /// The path is empty, absolute, non-normalized, or leaves the workspace.
    #[error("workspace-relative path must be non-empty, normalized, and contained")]
    Invalid,
    /// The UTF-8 representation exceeds the path byte limit.
    #[error("workspace-relative path exceeds {MAX_WORKSPACE_RELATIVE_PATH_BYTES} bytes")]
    TooLong,
}

/// A normalized, contained path using forward-slash wire separators.
///
/// Its schema uses `x-kuku-max-utf8-bytes` because JSON Schema's standard
/// `maxLength` keyword counts characters rather than UTF-8 bytes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspaceRelativePath(String);

impl WorkspaceRelativePath {
    /// Validates and constructs a canonical workspace-relative path.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, WorkspaceRelativePathError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(WorkspaceRelativePathError::Invalid);
        }
        if value.len() > MAX_WORKSPACE_RELATIVE_PATH_BYTES {
            return Err(WorkspaceRelativePathError::TooLong);
        }
        if value.contains('\\') {
            return Err(WorkspaceRelativePathError::Invalid);
        }
        if value.split('/').any(|segment| {
            segment.is_empty()
                || matches!(segment, "." | "..")
                || segment.contains(':')
                || segment.ends_with(['.', ' '])
                || is_windows_device_name(segment)
        }) {
            return Err(WorkspaceRelativePathError::Invalid);
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the canonical wire path.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_windows_device_name(segment: &str) -> bool {
    let stem = segment
        .split_once('.')
        .map_or(segment, |(stem, _)| stem)
        .trim_end_matches(['.', ' ']);
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || upper
            .strip_prefix("COM")
            .or_else(|| upper.strip_prefix("LPT"))
            .is_some_and(|number| {
                (number.len() == 1 && matches!(number.as_bytes()[0], b'1'..=b'9'))
                    || matches!(number, "¹" | "²" | "³")
            })
}

impl fmt::Display for WorkspaceRelativePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for WorkspaceRelativePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for WorkspaceRelativePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for WorkspaceRelativePath {
    fn schema_name() -> Cow<'static, str> {
        "WorkspaceRelativePath".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "minLength": 1,
            "maxLength": MAX_WORKSPACE_RELATIVE_PATH_BYTES,
            "pattern": r"^(?:[^./\\:]|\.[^./\\:]|\.\.[^/\\:])[^/\\:]*(?:/(?:[^./\\:]|\.[^./\\:]|\.\.[^/\\:])[^/\\:]*)*$",
            "allOf": [
                {
                    "not": {
                        "pattern": r"(?:^|/)[^/]*[. ](?:/|$)"
                    }
                },
                {
                    "not": {
                        "pattern": r"(?:^|/)(?:[Cc][Oo][Nn]|[Pp][Rr][Nn]|[Aa][Uu][Xx]|[Nn][Uu][Ll]|[Cc][Oo][Mm][1-9¹²³]|[Ll][Pp][Tt][1-9¹²³])[. ]*(?:\.[^/]*)?(?:/|$)"
                    }
                }
            ],
            "format": "workspace-relative-path",
            "x-kuku-max-utf8-bytes": MAX_WORKSPACE_RELATIVE_PATH_BYTES
        })
    }
}

/// Scope that owns or supplied a context source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceScope {
    /// Built-in runtime or host source.
    System,
    /// User-level source.
    User,
    /// Project-level source.
    Project,
    /// Registered workspace source.
    Workspace,
    /// Agent-specific source.
    Agent,
}

/// Stable identity and optional contained path for a context source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceFact {
    /// Ownership scope of the source.
    pub scope: SourceScope,
    /// Stable source identifier.
    pub id: String,
    /// Contained workspace-relative path when the source is file-backed.
    pub relative_path: Option<WorkspaceRelativePath>,
}

/// Role of an exact provider message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// Provider system instruction.
    System,
    /// Human or tool-result input.
    User,
    /// Model output.
    Assistant,
    /// Provider-native tool message.
    Tool,
}

/// Terminal state of a tool result content block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultStatus {
    /// Tool execution completed successfully.
    Completed,
    /// Tool execution failed.
    Failed,
}

/// Ordered typed content sent to the provider.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExactContentBlock {
    /// Plain text content.
    Text {
        /// Exact text sent to the provider.
        text: String,
    },
    /// Model reasoning content retained by the provider protocol.
    Thinking {
        /// Exact thinking text.
        text: String,
    },
    /// Model-requested tool invocation.
    ToolUse {
        /// Provider tool-call identity.
        tool_call_id: String,
        /// Registered tool name.
        name: String,
        /// Exact tool input value.
        input: serde_json::Value,
    },
    /// Result returned for a tool invocation.
    ToolResult {
        /// Identity of the matching tool call.
        tool_call_id: String,
        /// Tool execution status.
        status: ToolResultStatus,
        /// Exact model-visible result content.
        content: String,
        /// Optional structured result sent to the provider.
        structured: Option<serde_json::Value>,
        /// Whether the result was truncated before transport.
        truncated: bool,
    },
}

impl fmt::Debug for ExactContentBlock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text { .. } => formatter.write_str("Text(<redacted>)"),
            Self::Thinking { .. } => formatter.write_str("Thinking(<redacted>)"),
            Self::ToolUse { .. } => formatter.write_str("ToolUse(<redacted>)"),
            Self::ToolResult { .. } => formatter.write_str("ToolResult(<redacted>)"),
        }
    }
}

/// One exact provider message with ordered content blocks.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExactMessage {
    /// Message role.
    pub role: MessageRole,
    /// Content blocks in provider order.
    pub content: Vec<ExactContentBlock>,
}

impl fmt::Debug for ExactMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExactMessage")
            .field("role", &self.role)
            .field("content", &"<redacted>")
            .finish()
    }
}

/// Exact provider-visible tool definition.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExactTool {
    /// Tool name.
    pub name: String,
    /// Tool description sent to the provider.
    pub description: String,
    /// Tool input JSON Schema.
    pub input_schema: serde_json::Value,
}

impl fmt::Debug for ExactTool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExactTool")
            .field("name", &self.name)
            .field("definition", &"<redacted>")
            .finish()
    }
}

/// Failure to construct a finite provider temperature.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("temperature must be finite")]
pub struct TemperatureError;

/// A finite provider temperature serialized as a JSON number.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Temperature(f32);

impl Temperature {
    /// Constructs a temperature, rejecting NaN and positive or negative infinity.
    pub fn try_new(value: f32) -> Result<Self, TemperatureError> {
        if value.is_finite() {
            Ok(Self(value))
        } else {
            Err(TemperatureError)
        }
    }

    /// Returns the finite floating-point value.
    pub fn get(self) -> f32 {
        self.0
    }
}

impl Serialize for Temperature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_f32(self.0)
    }
}

impl<'de> Deserialize<'de> for Temperature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::try_new(f32::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Temperature {
    fn schema_name() -> Cow<'static, str> {
        "Temperature".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({"type": "number", "format": "finite-float32"})
    }
}

/// Provider thinking configuration preserved in the exact request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ThinkingConfig {
    /// Thinking is disabled.
    Disabled,
    /// Provider-managed adaptive thinking is enabled.
    Adaptive,
    /// Thinking is enabled with an optional explicit budget.
    Enabled {
        /// Provider thinking-token budget when configured.
        #[serde(deserialize_with = "super::task::deserialize_optional_json_safe_u64")]
        #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
        budget_tokens: Option<u64>,
    },
}

/// Allowlisted non-secret parameters sent with an exact provider request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExactRequestParameters {
    /// Resolved provider model.
    pub model: String,
    /// Maximum output tokens when configured.
    #[serde(deserialize_with = "super::task::deserialize_optional_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub max_output_tokens: Option<u64>,
    /// Finite sampling temperature when configured.
    pub temperature: Option<Temperature>,
    /// Whether the provider response is streamed.
    pub stream: bool,
    /// Resolved thinking configuration.
    pub thinking: ThinkingConfig,
}

/// Ordered exact messages, tools, and allowlisted request parameters.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExactRequest {
    /// Messages in provider order, including system content.
    pub messages: Vec<ExactMessage>,
    /// Tool definitions in provider order.
    pub tools: Vec<ExactTool>,
    /// Non-secret provider parameters.
    pub parameters: ExactRequestParameters,
}

impl fmt::Debug for ExactRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExactRequest")
            .field(
                "messages",
                &format_args!("{} <redacted>", self.messages.len()),
            )
            .field("tools", &format_args!("{} <redacted>", self.tools.len()))
            .field("parameters", &"<redacted>")
            .finish()
    }
}

/// Origin responsible for loading a Skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillLoadOrigin {
    /// Loaded by the user.
    You,
    /// Loaded by an Agent.
    Agent,
    /// Loaded by workspace bootstrap.
    Bootstrap,
    /// Loaded by project configuration.
    Project,
}

/// Skill content included in a request context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillContextFact {
    /// Stable catalog Skill identifier.
    pub skill_id: String,
    /// Skill source.
    pub source: SourceFact,
    /// Origin that loaded the Skill.
    pub origin: SkillLoadOrigin,
    /// Hash of the exact loaded Skill content.
    pub content_hash: String,
}

/// Kind of instruction source included in a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstructionKind {
    /// Built-in system instruction.
    System,
    /// Project instruction.
    Project,
    /// Workspace instruction.
    Workspace,
    /// Agent identity instruction.
    Agent,
}

/// Instruction source included in a request context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstructionContextFact {
    /// Instruction category.
    pub kind: InstructionKind,
    /// Instruction source.
    pub source: SourceFact,
    /// Hash of the exact included content.
    pub content_hash: String,
}

/// Scope of memory included in a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// User-global memory.
    Global,
    /// Project memory.
    Project,
}

/// Memory source included in a request context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryContextFact {
    /// Memory category.
    pub kind: MemoryKind,
    /// Memory source.
    pub source: SourceFact,
    /// Hash of the exact included content.
    pub content_hash: String,
}

/// Conversation history composition included in a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationContextFact {
    /// Number of retained turns.
    #[serde(deserialize_with = "super::task::deserialize_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub retained_turns: u64,
    /// Number of handoff boundaries represented in the history.
    #[serde(deserialize_with = "super::task::deserialize_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub handoff_boundaries: u64,
    /// Whether earlier history has been summarized.
    pub history_summarized: bool,
    /// Delegated conversations whose results entered this history.
    pub delegated_results: Vec<ConversationId>,
}

/// Delegated Agent result included in the main request context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DelegatedResultFact {
    /// Delegated conversation identity.
    pub conversation_id: ConversationId,
    /// Stable Agent catalog identifier.
    pub agent_id: String,
    /// Hash of the included result content.
    pub content_hash: String,
}

/// Server-provided execution capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    /// Read workspace files.
    FileRead,
    /// Write workspace files.
    FileWrite,
    /// Execute workspace commands.
    CommandExecution,
    /// Access network resources.
    NetworkAccess,
    /// Delegate work to an Agent.
    AgentDelegation,
    /// Discover and load Skills.
    SkillDiscovery,
    /// Read or write memory.
    Memory,
}

/// Availability or permission state of a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    /// Capability is available without a new approval.
    Available,
    /// Capability is unavailable.
    Unavailable,
    /// Capability requires user approval.
    RequiresApproval,
}

/// Capability and its request-time state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityFact {
    /// Capability category.
    pub kind: CapabilityKind,
    /// Request-time capability state.
    pub state: CapabilityState,
}

/// Kind and immutable metadata of a workspace observation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObservationKind {
    /// File content was read.
    FileRead,
    /// A directory or file listing was observed.
    FileList,
    /// Text or path search results were observed.
    Search {
        /// Exact search query.
        query: String,
    },
    /// Command output was observed.
    Command {
        /// Exact command invocation.
        command: String,
        /// Process exit code when available.
        exit_code: Option<i32>,
    },
    /// Result from another named tool was observed.
    Tool {
        /// Registered tool name.
        name: String,
    },
}

impl fmt::Debug for ObservationKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileRead => formatter.write_str("FileRead"),
            Self::FileList => formatter.write_str("FileList"),
            Self::Search { .. } => formatter.write_str("Search(<redacted>)"),
            Self::Command { .. } => formatter.write_str("Command(<redacted>)"),
            Self::Tool { .. } => formatter.write_str("Tool(<redacted>)"),
        }
    }
}

/// Request-time retention state of an observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationRetention {
    /// Full model-visible observation was retained.
    Retained,
    /// Observation was replaced by a summary.
    Summarized,
    /// Observation was truncated.
    Truncated,
}

/// One-based inclusive line range observed in a workspace file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ObservedRange {
    /// First observed line.
    #[serde(deserialize_with = "super::task::deserialize_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub start_line: u64,
    /// Last observed line.
    #[serde(deserialize_with = "super::task::deserialize_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub end_line: u64,
}

/// Immutable observation recorded for one provider request.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ObservationFact {
    /// Request that observed the value.
    pub scope: RequestScope,
    /// Tool-call identity that produced the observation.
    pub tool_call_id: String,
    /// Observation category and metadata.
    pub kind: ObservationKind,
    /// Contained workspace-relative path when applicable.
    pub relative_path: Option<WorkspaceRelativePath>,
    /// Hash of the observed content when available.
    pub observed_hash: Option<String>,
    /// Observed line range when applicable.
    pub range: Option<ObservedRange>,
    /// Request-time retention state.
    pub retention: ObservationRetention,
    /// Safe compact description of the observation.
    pub summary: String,
}

impl fmt::Debug for ObservationFact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObservationFact")
            .field("scope", &self.scope)
            .field("tool_call_id", &self.tool_call_id)
            .field("kind", &self.kind)
            .field("relative_path", &self.relative_path)
            .field("observed_hash", &self.observed_hash)
            .field("range", &self.range)
            .field("retention", &self.retention)
            .field("summary", &"<redacted>")
            .finish()
    }
}

/// Structured sources and capabilities included in a request context.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextBreakdown {
    /// Loaded Skills.
    pub skills: Vec<SkillContextFact>,
    /// Included instructions.
    pub instructions: Vec<InstructionContextFact>,
    /// Included memory sources.
    pub memory: Vec<MemoryContextFact>,
    /// Conversation-history composition.
    pub conversation: ConversationContextFact,
    /// Workspace observations available to the request.
    pub observations: Vec<ObservationFact>,
    /// Delegated results included in the request.
    pub delegated_results: Vec<DelegatedResultFact>,
    /// Effective execution capabilities.
    pub capabilities: Vec<CapabilityFact>,
    /// Server-provided input token estimate when available.
    #[serde(deserialize_with = "super::task::deserialize_optional_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub token_estimate: Option<u64>,
}

impl fmt::Debug for ContextBreakdown {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContextBreakdown")
            .field("skills", &self.skills.len())
            .field("instructions", &self.instructions.len())
            .field("memory", &self.memory.len())
            .field("conversation", &"<redacted>")
            .field("observations", &self.observations.len())
            .field("delegated_results", &self.delegated_results.len())
            .field("capabilities", &self.capabilities.len())
            .field("token_estimate", &self.token_estimate)
            .finish()
    }
}

/// Immutable exact provider request and its structured context evidence.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RequestSnapshot {
    /// Request identity and execution scope.
    pub scope: RequestScope,
    /// Cause of the provider request.
    pub cause: RequestCause,
    /// Resolved provider kind.
    pub provider: ProviderFact,
    /// Stable selected Tier identifier.
    pub tier_id: String,
    /// Exact provider-visible request.
    pub exact: ExactRequest,
    /// Structured request context breakdown.
    pub context: ContextBreakdown,
    /// Catalog revision used for the request.
    pub catalog_revision: RevisionToken,
    /// Canonical hash of the exact provider payload.
    pub exact_payload_hash: String,
}

impl fmt::Debug for RequestSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RequestSnapshot")
            .field("scope", &self.scope)
            .field("cause", &self.cause)
            .field("provider", &self.provider)
            .field("tier_id", &self.tier_id)
            .field("exact", &"<redacted>")
            .field("context", &"<redacted>")
            .field("catalog_revision", &self.catalog_revision)
            .field("exact_payload_hash", &self.exact_payload_hash)
            .finish()
    }
}

/// Durable record that a Skill entered an execution context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillLoadFact {
    /// Execution that loaded the Skill.
    pub execution: ExecutionScope,
    /// Provider request that caused the load, when Agent-initiated.
    pub caused_by_request_id: Option<RequestId>,
    /// Stable catalog Skill identifier.
    pub skill_id: String,
    /// Skill source.
    pub source: SourceFact,
    /// Origin responsible for the load.
    pub origin: SkillLoadOrigin,
    /// Hash of the exact loaded content.
    pub content_hash: String,
}
