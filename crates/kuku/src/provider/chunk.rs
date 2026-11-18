/// Provider-neutral streaming chunk.
/// Each adapter normalizes its native SSE into these variants.
#[derive(Debug, Clone)]
pub enum ProviderChunk {
    /// Stream established.
    StreamStart { request_id: String },
    /// A text content fragment.
    TextDelta { text: String },
    /// The provider reported reasoning activity without displayable text.
    Reasoning,
    /// A provider-published reasoning summary fragment.
    ThinkingDelta { text: String },
    Replay {
        protocol: crate::config::ProviderFormat,
        value: serde_json::Value,
    },
    Report {
        report: super::types::ProviderReport,
    },
    /// A tool call has begun (id and name are now known).
    ToolCallStart {
        index: u64,
        id: String,
        name: String,
    },
    /// Incremental JSON argument fragment for a tool call at `index`.
    ToolCallArgDelta { index: u64, fragment: String },
    /// A tool call at `index` is finished.
    ToolCallStop { index: u64 },
    /// A non-tool content block at `index` is finished.
    ContentBlockStop { index: u64 },
    /// The provider emitted a malformed tool lifecycle event.
    InvalidToolStream,
    /// Usage statistics.
    StreamUsage {
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    },
    /// The model's final stop reason for the response.
    StopReason { reason: ModelStopReason },
    /// The provider returned an error event mid-stream.
    ServerError { code: String, message: String },
    /// The stream ended normally.
    StreamEnd,
}
use crate::event::ModelStopReason;
