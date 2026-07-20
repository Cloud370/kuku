use sha2::{Digest, Sha256};

use crate::event::{
    ContextBreakdown, ExactContentBlock, ExactMessage, ExactRequest, ExactRequestParameters,
    ExactTool, MessageRole, ProviderFact, RequestCause, RequestScope, RequestSnapshot,
    RevisionToken, ToolResultStatus,
};

use super::{CanonicalMessage, ContextAssembly, MessageBlock, Role};

/// Maximum serialized size of one sensitive request snapshot.
pub const MAX_REQUEST_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;

/// Inputs required to build one immutable request snapshot.
pub struct SnapshotInput<'a> {
    /// Request and execution identity.
    pub scope: RequestScope,
    /// Causal origin of this provider request.
    pub cause: RequestCause,
    /// Resolved provider kind.
    pub provider: ProviderFact,
    /// Stable selected Tier identifier.
    pub tier_id: &'a str,
    /// Provider-neutral assembled prompt and tools.
    pub assembly: &'a ContextAssembly,
    /// Current provider input appended after assembled history.
    pub current_input: &'a CanonicalMessage,
    /// Explicit non-secret request parameters.
    pub allowlisted_provider_parameters: ExactRequestParameters,
    /// Structured context provenance for this request.
    pub breakdown: ContextBreakdown,
    /// Catalog revision used by the assembly.
    pub catalog_revision: RevisionToken,
}

/// Failure to build or size an exact request snapshot.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotBuildError {
    /// Exact request serialization failed without rendering sensitive values.
    #[error("request snapshot serialization failed")]
    Serialization(#[source] serde_json::Error),
    /// The serialized snapshot exceeded the hard persistence limit.
    #[error("request snapshot is {size} bytes, exceeding the {limit}-byte limit")]
    TooLarge { size: usize, limit: usize },
}

/// Builds sensitive immutable request snapshots from SDK-owned values.
pub struct RequestSnapshotBuilder;

impl RequestSnapshotBuilder {
    /// Builds and size-checks one exact request snapshot.
    pub fn build(input: SnapshotInput<'_>) -> Result<RequestSnapshot, SnapshotBuildError> {
        let exact = exact_request(
            input.assembly,
            input.current_input,
            input.allowlisted_provider_parameters,
        );
        let exact_payload_hash = exact_payload_hash(&exact)?;
        let snapshot = RequestSnapshot {
            scope: input.scope,
            cause: input.cause,
            provider: input.provider,
            tier_id: input.tier_id.to_string(),
            exact,
            context: input.breakdown,
            catalog_revision: input.catalog_revision,
            exact_payload_hash,
        };
        let size = serde_json::to_vec(&snapshot)
            .map_err(SnapshotBuildError::Serialization)?
            .len();
        if size > MAX_REQUEST_SNAPSHOT_BYTES {
            return Err(SnapshotBuildError::TooLarge {
                size,
                limit: MAX_REQUEST_SNAPSHOT_BYTES,
            });
        }
        Ok(snapshot)
    }
}

fn exact_request(
    assembly: &ContextAssembly,
    current_input: &CanonicalMessage,
    parameters: ExactRequestParameters,
) -> ExactRequest {
    let mut messages =
        Vec::with_capacity(2 + assembly.prelude_messages.len() + assembly.history.len());
    messages.push(ExactMessage {
        role: MessageRole::System,
        content: vec![ExactContentBlock::Text {
            text: assembly.system_prompt.clone(),
        }],
    });
    messages.extend(assembly.prelude_messages.iter().map(exact_message));
    messages.extend(assembly.history.iter().map(exact_message));
    messages.push(exact_message(current_input));

    let tools = assembly
        .tools
        .iter()
        .map(|tool| ExactTool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
        })
        .collect();

    ExactRequest {
        messages,
        tools,
        parameters,
    }
}

fn exact_message(message: &CanonicalMessage) -> ExactMessage {
    let role = match message.role {
        Role::User => MessageRole::User,
        Role::Assistant => MessageRole::Assistant,
    };
    let content = message.blocks.iter().map(exact_content_block).collect();
    ExactMessage { role, content }
}

fn exact_content_block(block: &MessageBlock) -> ExactContentBlock {
    match block {
        MessageBlock::Text(text) => ExactContentBlock::Text { text: text.clone() },
        MessageBlock::Thinking(text) => ExactContentBlock::Thinking { text: text.clone() },
        MessageBlock::ToolUse(tool) => ExactContentBlock::ToolUse {
            tool_call_id: tool.id.clone(),
            name: tool.name.clone(),
            input: tool.args.clone(),
        },
        MessageBlock::ToolResult(result) => ExactContentBlock::ToolResult {
            tool_call_id: result.tool_call_id.clone(),
            status: if result.status == "ok" {
                ToolResultStatus::Completed
            } else {
                ToolResultStatus::Failed
            },
            content: result.model_content.clone(),
            structured: result.structured.clone(),
            truncated: result.truncated,
        },
    }
}

fn exact_payload_hash(exact: &ExactRequest) -> Result<String, SnapshotBuildError> {
    let value = serde_json::to_value(exact).map_err(SnapshotBuildError::Serialization)?;
    let canonical = canonicalize_json(value);
    let bytes = serde_json::to_vec(&canonical).map_err(SnapshotBuildError::Serialization)?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn canonicalize_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonicalize_json).collect())
        }
        serde_json::Value::Object(values) => {
            let mut entries = values.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize_json(value)))
                    .collect(),
            )
        }
        scalar => scalar,
    }
}
