use std::collections::VecDeque;
use std::path::PathBuf;
use std::pin::Pin;

use futures_core::Stream;

use crate::provider::chunk::ProviderChunk;
use crate::provider::types::{ProviderFailure, ProviderToolCall, ResolvedProvider};
use crate::tool::ToolDefinition;

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub(super) prompt: String,
    pub(super) session_id: Option<String>,
    pub(super) provider: Option<crate::provider::Provider>,
    pub(super) model: Option<String>,
    pub(super) base_url: Option<String>,
    pub(super) api_key: Option<String>,
    pub(super) max_output_tokens: Option<u32>,
    pub(super) temperature: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub session_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
    pub id: String,
    pub tool_call_id: String,
    pub tool: String,
    pub risk: String,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionChoice {
    Once,
    Session,
    Project,
    Deny,
}

/// Host-facing runtime event stream.
///
/// This enum is non-exhaustive; hosts must keep a fallback arm when matching it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolCall {
        tool_call_id: String,
        tool: String,
        summary: String,
    },
    ToolResult {
        tool_call_id: String,
        summary: String,
    },
    PermissionRequested {
        request: PermissionRequest,
    },
    Done {
        output: RunOutput,
    },
}

#[derive(Debug)]
pub struct Run {
    pub(super) session_id: String,
    pub(super) state: RunState,
}

#[derive(Debug)]
pub(super) enum RunState {
    Pending(Box<PendingRun>),
    Streaming(Box<StreamingChunkState>),
    WaitingForPermission(Box<PendingPermission>),
    Done(Option<RunOutput>),
}

#[derive(Debug)]
pub(super) struct PendingRun {
    pub(super) session_id: String,
    pub(super) query: Query,
    pub(super) events_path: PathBuf,
    pub(super) kuku_home: PathBuf,
    pub(super) workspace: PathBuf,
    pub(super) policy_path: PathBuf,
    pub(super) turn: u64,
    pub(super) request_num: u64,
    pub(super) resolved: Option<ResolvedRuntime>,
    pub(super) queued_tool_calls: VecDeque<QueuedToolCall>,
    pub(super) saved_tool_call: Option<QueuedToolCall>,
}

#[derive(Debug)]
pub(super) struct ResolvedRuntime {
    pub(super) config: ResolvedProvider,
    pub(super) registry: Vec<ToolDefinition>,
    pub(super) registry_hash: String,
    pub(super) ordered_tool_names: Vec<String>,
    pub(super) tool_count: usize,
    pub(super) provider_name: String,
}

#[derive(Debug)]
pub(super) struct QueuedToolCall {
    pub(super) tool_call: ProviderToolCall,
    pub(super) summary: String,
}

#[derive(Debug)]
pub(super) struct PendingPermission {
    pub(super) pending: PendingRun,
    pub(super) queued_tool_call: QueuedToolCall,
    pub(super) request: PermissionRequest,
}

pub(super) enum PendingStep {
    Pending(Box<PendingRun>),
    NeedPermission(Box<PendingPermission>),
    Streaming(Box<StreamingChunkState>),
    ToolCallReady {
        pending: Box<PendingRun>,
        ui_event: UiEvent,
    },
    ToolResultReady {
        pending: Box<PendingRun>,
        ui_event: UiEvent,
    },
    Done(RunOutput),
}

pub(super) struct StreamingChunkState {
    pub(super) pending: PendingRun,
    pub(super) request_id: String,
    pub(super) stream:
        Pin<Box<dyn Stream<Item = std::result::Result<ProviderChunk, ProviderFailure>> + Send>>,
    pub(super) accumulated_text: String,
    pub(super) accumulated_thinking: String,
    pub(super) stop_reason: Option<String>,
    pub(super) tool_calls: Vec<ProviderToolCall>,
    pub(super) tool_arg_buffers: Vec<(u64, String)>,
    pub(super) provider_request_id: Option<String>,
    pub(super) usage: Option<crate::provider::types::ProviderUsage>,
}

impl std::fmt::Debug for StreamingChunkState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamingChunkState")
            .field("request_id", &self.request_id)
            .field("accumulated_text", &self.accumulated_text)
            .field("stop_reason", &self.stop_reason)
            .field("tool_calls", &self.tool_calls)
            .finish_non_exhaustive()
    }
}

// ---------- Query builder ----------

impl Query {
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            session_id: None,
            provider: None,
            model: None,
            base_url: None,
            api_key: None,
            max_output_tokens: None,
            temperature: None,
        }
    }

    pub fn session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn provider(mut self, provider: crate::provider::Provider) -> Self {
        self.provider = Some(provider);
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    pub fn max_output_tokens(mut self, max_output_tokens: u32) -> Self {
        self.max_output_tokens = Some(max_output_tokens);
        self
    }

    pub fn temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
}
