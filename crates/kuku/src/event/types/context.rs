use std::borrow::Cow;
use std::fmt;
use std::path::{Component, Path};

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{
    ConversationId, ExecutionScope, ProviderFact, RequestCause, RequestId, RequestScope,
    RevisionToken,
};

pub const MAX_WORKSPACE_RELATIVE_PATH_BYTES: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceRelativePathError {
    #[error("workspace-relative path must be non-empty, normalized, and contained")]
    Invalid,
    #[error("workspace-relative path exceeds {MAX_WORKSPACE_RELATIVE_PATH_BYTES} bytes")]
    TooLong,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspaceRelativePath(String);

impl WorkspaceRelativePath {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, WorkspaceRelativePathError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(WorkspaceRelativePathError::Invalid);
        }
        if value.len() > MAX_WORKSPACE_RELATIVE_PATH_BYTES {
            return Err(WorkspaceRelativePathError::TooLong);
        }
        if value.contains(['\\', ':']) {
            return Err(WorkspaceRelativePathError::Invalid);
        }

        let path = Path::new(value);
        if path.is_absolute() {
            return Err(WorkspaceRelativePathError::Invalid);
        }

        let mut normalized = String::new();
        for component in path.components() {
            let Component::Normal(segment) = component else {
                return Err(WorkspaceRelativePathError::Invalid);
            };
            let segment = segment
                .to_str()
                .ok_or(WorkspaceRelativePathError::Invalid)?;
            if !normalized.is_empty() {
                normalized.push('/');
            }
            normalized.push_str(segment);
        }

        if normalized != value {
            return Err(WorkspaceRelativePathError::Invalid);
        }
        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
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
            "pattern": "^(?!/)(?!.*(?:^|/)\\.\\.?(?:/|$))(?!.*[\\\\:]).+$"
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceScope {
    System,
    User,
    Project,
    Workspace,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceFact {
    pub scope: SourceScope,
    pub id: String,
    pub relative_path: Option<WorkspaceRelativePath>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultStatus {
    Completed,
    Failed,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExactContentBlock {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolUse {
        tool_call_id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_call_id: String,
        status: ToolResultStatus,
        content: String,
        structured: Option<serde_json::Value>,
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

#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExactMessage {
    pub role: MessageRole,
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

#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExactTool {
    pub name: String,
    pub description: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ThinkingConfig {
    Disabled,
    Adaptive,
    Enabled {
        #[serde(deserialize_with = "super::task::deserialize_optional_json_safe_u64")]
        #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
        budget_tokens: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExactRequestParameters {
    pub model: String,
    #[serde(deserialize_with = "super::task::deserialize_optional_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub max_output_tokens: Option<u64>,
    pub temperature: Option<f32>,
    pub stream: bool,
    pub thinking: ThinkingConfig,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExactRequest {
    pub messages: Vec<ExactMessage>,
    pub tools: Vec<ExactTool>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillLoadOrigin {
    You,
    Agent,
    Bootstrap,
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillContextFact {
    pub skill_id: String,
    pub source: SourceFact,
    pub origin: SkillLoadOrigin,
    pub content_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstructionKind {
    System,
    Project,
    Workspace,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstructionContextFact {
    pub kind: InstructionKind,
    pub source: SourceFact,
    pub content_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Global,
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryContextFact {
    pub kind: MemoryKind,
    pub source: SourceFact,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationContextFact {
    #[serde(deserialize_with = "super::task::deserialize_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub retained_turns: u64,
    #[serde(deserialize_with = "super::task::deserialize_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub handoff_boundaries: u64,
    pub history_summarized: bool,
    pub delegated_results: Vec<ConversationId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DelegatedResultFact {
    pub conversation_id: ConversationId,
    pub agent_id: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    FileRead,
    FileWrite,
    CommandExecution,
    NetworkAccess,
    AgentDelegation,
    SkillDiscovery,
    Memory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Available,
    Unavailable,
    RequiresApproval,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityFact {
    pub kind: CapabilityKind,
    pub state: CapabilityState,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObservationKind {
    FileRead,
    FileList,
    Search {
        query: String,
    },
    Command {
        command: String,
        exit_code: Option<i32>,
    },
    Tool {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationRetention {
    Retained,
    Summarized,
    Truncated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ObservedRange {
    #[serde(deserialize_with = "super::task::deserialize_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub start_line: u64,
    #[serde(deserialize_with = "super::task::deserialize_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub end_line: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ObservationFact {
    pub scope: RequestScope,
    pub tool_call_id: String,
    pub kind: ObservationKind,
    pub relative_path: Option<WorkspaceRelativePath>,
    pub observed_hash: Option<String>,
    pub range: Option<ObservedRange>,
    pub retention: ObservationRetention,
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

#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextBreakdown {
    pub skills: Vec<SkillContextFact>,
    pub instructions: Vec<InstructionContextFact>,
    pub memory: Vec<MemoryContextFact>,
    pub conversation: ConversationContextFact,
    pub observations: Vec<ObservationFact>,
    pub delegated_results: Vec<DelegatedResultFact>,
    pub capabilities: Vec<CapabilityFact>,
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

#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RequestSnapshot {
    pub scope: RequestScope,
    pub cause: RequestCause,
    pub provider: ProviderFact,
    pub tier_id: String,
    pub exact: ExactRequest,
    pub context: ContextBreakdown,
    pub catalog_revision: RevisionToken,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillLoadFact {
    pub execution: ExecutionScope,
    pub caused_by_request_id: Option<RequestId>,
    pub skill_id: String,
    pub source: SourceFact,
    pub origin: SkillLoadOrigin,
    pub content_hash: String,
}
