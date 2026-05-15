/// Provider-neutral streaming chunk.
/// Each adapter normalizes its native SSE into these variants.
#[derive(Debug, Clone)]
pub(crate) enum ProviderChunk {
    /// Stream established. Carries model info for diagnostics.
    StreamStart { request_id: String, model: String },
    /// A text content fragment.
    TextDelta { text: String },
    /// A tool call has begun (id and name are now known).
    ToolCallStart {
        index: u64,
        id: String,
        name: String,
    },
    /// Incremental JSON argument fragment for a tool call at `index`.
    ToolCallArgDelta { index: u64, fragment: String },
    /// A content block (text or tool_use) at `index` is finished.
    ContentBlockStop { index: u64 },
    /// Usage statistics.
    StreamUsage {
        input_tokens: u64,
        output_tokens: u64,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    },
    /// The model's final stop reason for the response.
    StopReason { reason: String },
    /// The stream ended normally.
    StreamEnd,
}
