use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use futures_core::Stream;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::provider::chunk::ProviderChunk;
use crate::provider::types::{ProviderFailure, ProviderToolCall, ResolvedProvider};
use crate::tool::ToolDefinition;

#[derive(Debug, Clone)]
/// Builder for configuring and executing a model query.
pub struct Query {
    pub(super) prompt: String,
    pub(super) session_id: Option<String>,
    pub(super) provider: Option<crate::provider::Provider>,
    pub(super) model: Option<String>,
    pub(super) tier: Option<String>,
    pub(super) config_path: Option<PathBuf>,
    pub(super) config_obj: Option<Config>,
    pub(super) base_url: Option<String>,
    pub(super) api_key: Option<String>,
    pub(super) max_output_tokens: Option<u32>,
    pub(super) temperature: Option<f32>,
    pub(super) workspace_path: Option<PathBuf>,
    pub(super) prompts_dir: Option<PathBuf>,
    pub(super) disable_agents: bool,
    pub(super) disable_skills: bool,
    pub(super) skill_body: Option<String>,
    pub(super) subagent_registry: Option<crate::subagent::registry::SubagentRegistry>,
    pub(crate) tool_registry_override: Option<Vec<crate::tool::ToolDefinition>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Final output from a completed query run.
pub struct RunOutput {
    pub session_id: String,
    pub text: String,
    pub usage: Option<crate::provider::types::ProviderUsage>,
    pub turn: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
/// A pending permission request for a tool call.
pub struct PermissionRequest {
    pub id: String,
    pub tool_call_id: String,
    pub tool: String,
    pub risk: String,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The host's response to a permission request.
pub enum PermissionChoice {
    Once,
    Session,
    Project,
    Deny,
}

/// Identifies the kind of tool execution.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Simple,
    Agent { child_session_id: String },
    Command { pid: Option<u32> },
}

/// Events produced during a tool execution phase.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEvent {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolStart {
        id: String,
        tool: String,
        summary: String,
        kind: ToolKind,
    },
    ToolOutput {
        id: String,
        event: Box<ToolEvent>,
    },
    ToolEnd {
        id: String,
        status: String,
        summary: String,
    },
    Stdout {
        text: String,
    },
    Stderr {
        text: String,
    },
    PermissionRequested {
        request: PermissionRequest,
    },
    Error {
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Permission mode for child sessions.
pub enum PermissionMode {
    AutoAllow,
    Interactive,
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
    ToolStart {
        id: String,
        tool: String,
        summary: String,
        kind: ToolKind,
    },
    ToolOutput {
        id: String,
        event: ToolEvent,
    },
    ToolEnd {
        id: String,
        status: String,
        summary: String,
        model_content: Option<String>,
        result: Option<serde_json::Value>,
    },
    PermissionRequested {
        request: PermissionRequest,
    },
    Error {
        code: String,
        message: String,
    },
    ModelRequest {
        model: String,
        provider: String,
    },
    TurnStart {
        turn: u64,
    },
    Done {
        output: RunOutput,
        usage: Option<crate::provider::types::ProviderUsage>,
        turn: u64,
    },
    Cancelled {
        turn: u64,
    },
}

#[derive(Debug)]
/// An active query execution that yields UI events via `next()`.
pub struct Run {
    pub(super) session_id: String,
    pub(super) state: RunState,
    pub(crate) slots: std::collections::HashMap<String, ExecSlot>,
    pub(crate) slot_event_tx: tokio::sync::mpsc::Sender<(String, SlotEvent)>,
    pub(crate) slot_event_rx: tokio::sync::mpsc::Receiver<(String, SlotEvent)>,
    pub(crate) cancel_token: Arc<tokio::sync::Notify>,
    pub(crate) lock_path: PathBuf,
}

pub(crate) struct ExecSlot {
    pub(crate) tool_call_id: String,
    pub(crate) kind: ToolKind,
    pub(crate) label: String,
    pub(crate) cancel: Arc<tokio::sync::Notify>,
    pub(crate) child_permissions:
        Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<PermissionChoice>>>>,
}

impl std::fmt::Debug for ExecSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecSlot")
            .field("tool_call_id", &self.tool_call_id)
            .field("kind", &self.kind)
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(crate) enum SlotEvent {
    Output(ToolEvent),
    Done {
        status: String,
        summary: String,
        model_content: String,
        result: Option<serde_json::Value>,
    },
}

#[derive(Debug)]
pub(super) enum RunState {
    Pending(Box<PendingRun>),
    Streaming(Box<StreamingChunkState>),
    WaitingForPermission(Box<PendingPermission>),
    Cancelled {
        events_path: std::path::PathBuf,
        turn: u64,
    },
    Done(
        Option<(
            RunOutput,
            Option<crate::provider::types::ProviderUsage>,
            u64,
        )>,
    ),
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
    pub(super) cumulative_input_tokens: u64,
    pub(super) cumulative_output_tokens: u64,
    pub(super) cumulative_cache_read_input_tokens: u64,
    pub(super) cumulative_cache_creation_input_tokens: u64,
    pub(super) resolved: Option<ResolvedRuntime>,
    pub(super) queued_tool_calls: VecDeque<QueuedToolCall>,
    pub(super) config: Arc<Config>,
    pub(super) prompts_dir: Option<PathBuf>,
    pub(super) subagent_registry: Option<crate::subagent::registry::SubagentRegistry>,
    pub(super) skill_registry: Option<crate::skill::registry::SkillRegistry>,
    pub(super) skill_content_hash: Option<String>,
    pub(super) skill_body: Option<String>,
    pub(super) child_session_count: u32,
    pub(super) tool_registry_override: Option<Vec<crate::tool::ToolDefinition>>,
    pub(super) catalog: crate::prompt::PromptCatalog,
    pub(super) pending_events: std::collections::VecDeque<UiEvent>,
    pub(super) cancel_token: Arc<tokio::sync::Notify>,
    pub(super) handoff_triggered: bool,
    pub(super) handoff_keep_turns: usize,
}

#[derive(Debug)]
pub(super) struct ResolvedRuntime {
    pub(super) config: ResolvedProvider,
    pub(super) registry: Vec<ToolDefinition>,
    pub(super) registry_hash: String,
    pub(super) tool_names: Vec<String>,
    pub(super) tool_count: usize,
}

#[derive(Debug)]
pub(super) struct QueuedToolCall {
    pub(super) tool_call: ProviderToolCall,
    pub(super) display_summary: String,
}

#[derive(Debug)]
pub(super) struct PendingPermission {
    pub(super) pending: PendingRun,
    pub(super) request: PermissionRequest,
}

pub(super) enum PendingStep {
    /// One tool call processed. May carry a spawned slot and/or a UI event.
    /// When both are None, advance_pending has no more queued calls to process.
    Pending {
        pending: Box<PendingRun>,
        slot: Option<ExecSlot>,
        event: Option<UiEvent>,
    },
    NeedPermission(Box<PendingPermission>),
    Streaming(Box<StreamingChunkState>),
    #[allow(clippy::type_complexity)]
    Done(
        RunOutput,
        Option<crate::provider::types::ProviderUsage>,
        u64,
    ),
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
    pub(super) lead_events: Vec<UiEvent>,
    pub(super) handoff_detector: Option<HandoffDetector>,
}

const HANDOFF_OPEN_TAG: &str = "<kuku_handoff>";
const HANDOFF_CLOSE_TAG: &str = "</kuku_handoff>";

#[derive(Debug)]
enum DetectorState {
    UserText,
    TagScan,
    HandoffBody,
    ClosingScan,
    Done,
}

#[derive(Debug)]
pub(super) struct HandoffDetector {
    state: DetectorState,
    user_text: String,
    handoff_text: String,
    tag_buffer: String,
}

impl HandoffDetector {
    pub(super) fn new() -> Self {
        Self {
            state: DetectorState::UserText,
            user_text: String::new(),
            handoff_text: String::new(),
            tag_buffer: String::new(),
        }
    }

    pub(super) fn process(&mut self, chunk: &str) -> Option<String> {
        for ch in chunk.chars() {
            match &self.state {
                DetectorState::UserText => {
                    if ch == '<' {
                        self.tag_buffer.clear();
                        self.tag_buffer.push(ch);
                        self.state = DetectorState::TagScan;
                    } else {
                        self.user_text.push(ch);
                    }
                }
                DetectorState::TagScan => {
                    self.tag_buffer.push(ch);
                    if self.tag_buffer == HANDOFF_OPEN_TAG {
                        self.state = DetectorState::HandoffBody;
                        self.tag_buffer.clear();
                    } else if !HANDOFF_OPEN_TAG.starts_with(&self.tag_buffer) {
                        let buffered = self.tag_buffer.clone();
                        self.tag_buffer.clear();
                        self.state = DetectorState::UserText;
                        for buffered_ch in buffered.chars() {
                            self.user_text.push(buffered_ch);
                        }
                    }
                }
                DetectorState::HandoffBody => {
                    if ch == '<' {
                        self.tag_buffer.clear();
                        self.tag_buffer.push(ch);
                        self.state = DetectorState::ClosingScan;
                    } else {
                        self.handoff_text.push(ch);
                    }
                }
                DetectorState::ClosingScan => {
                    self.tag_buffer.push(ch);
                    if self.tag_buffer == HANDOFF_CLOSE_TAG {
                        self.state = DetectorState::Done;
                        self.tag_buffer.clear();
                    } else if !HANDOFF_CLOSE_TAG.starts_with(&self.tag_buffer) {
                        let buffered = self.tag_buffer.clone();
                        self.tag_buffer.clear();
                        self.state = DetectorState::HandoffBody;
                        for buffered_ch in buffered.chars() {
                            self.handoff_text.push(buffered_ch);
                        }
                    }
                }
                DetectorState::Done => {}
            }
        }

        match &self.state {
            DetectorState::UserText | DetectorState::TagScan => {
                let text = std::mem::take(&mut self.user_text);
                if text.is_empty() {
                    None
                } else {
                    Some(text)
                }
            }
            _ => None,
        }
    }

    pub(super) fn finish(self) -> Option<String> {
        match self.state {
            DetectorState::Done => Some(self.handoff_text),
            DetectorState::HandoffBody | DetectorState::ClosingScan => {
                if !self.handoff_text.is_empty() {
                    Some(self.handoff_text)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
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
    /// Create a new query builder for the given prompt.
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            session_id: None,
            provider: None,
            model: None,
            tier: None,
            config_path: None,
            config_obj: None,
            base_url: None,
            api_key: None,
            max_output_tokens: None,
            temperature: None,
            workspace_path: None,
            prompts_dir: None,
            disable_agents: false,
            disable_skills: false,
            skill_body: None,
            subagent_registry: None,
            tool_registry_override: None,
        }
    }

    /// Register a subagent registry for agent tool dispatch.
    pub fn subagents(mut self, registry: crate::subagent::registry::SubagentRegistry) -> Self {
        self.subagent_registry = Some(registry);
        self
    }

    /// Disable the agent tool (subagent delegation).
    pub fn no_agents(mut self) -> Self {
        self.disable_agents = true;
        self
    }

    /// Disable the use_skill tool.
    pub fn no_skills(mut self) -> Self {
        self.disable_skills = true;
        self
    }

    /// Attach a skill body to be injected as a separate block before user input.
    pub fn skill_body(mut self, body: String) -> Self {
        self.skill_body = Some(body);
        self
    }

    /// Set or resume a session by ID.
    pub fn session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Set the provider to use for this query.
    pub fn provider(mut self, provider: crate::provider::Provider) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Set the model alias or provider:model name.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Override the provider base URL.
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// Set the API key directly, bypassing config resolution.
    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Set the maximum output token budget.
    pub fn max_output_tokens(mut self, max_output_tokens: u32) -> Self {
        self.max_output_tokens = Some(max_output_tokens);
        self
    }

    /// Set the sampling temperature.
    pub fn temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Select a capability tier (strong/balanced/light). Mutually exclusive with `.model()`.
    pub fn tier(mut self, tier: impl Into<String>) -> Self {
        self.tier = Some(tier.into());
        self
    }

    /// Load config from the given path.
    pub fn config_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_path = Some(path.into());
        self
    }

    /// Use an in-memory config (bypasses file loading).
    pub fn config(mut self, cfg: Config) -> Self {
        self.config_obj = Some(cfg);
        self
    }

    /// Set the workspace directory (defaults to cwd).
    pub fn workspace(mut self, path: impl Into<PathBuf>) -> Self {
        self.workspace_path = Some(path.into());
        self
    }

    /// Directory containing prompt files to override embedded defaults.
    pub fn prompts_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.prompts_dir = Some(path.into());
        self
    }

    pub(super) fn validate(&self) -> Result<()> {
        if self.tier.is_some() && self.model.is_some() {
            return Err(Error::InvalidArgument(
                ".tier() and .model() are mutually exclusive".to_string(),
            ));
        }
        Ok(())
    }

    /// The prompt text.
    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// The session ID, if set.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_kind_serialization() {
        assert_eq!(
            serde_json::to_string(&ToolKind::Simple).unwrap(),
            r#""simple""#
        );
        let cmd = ToolKind::Command { pid: None };
        assert_eq!(
            serde_json::to_string(&cmd).unwrap(),
            r#"{"command":{"pid":null}}"#
        );
        let agent = ToolKind::Agent {
            child_session_id: "child_abc_0".into(),
        };
        let agent_json = serde_json::to_string(&agent).unwrap();
        let back: ToolKind = serde_json::from_str(&agent_json).unwrap();
        assert_eq!(back, agent);
    }

    #[test]
    fn tool_event_recursive_serialization() {
        let inner = ToolEvent::ToolStart {
            id: "tc_2".into(),
            tool: "find_files".into(),
            summary: "search".into(),
            kind: ToolKind::Simple,
        };
        let outer = ToolEvent::ToolOutput {
            id: "tc_1".into(),
            event: Box::new(inner),
        };
        let json = serde_json::to_string(&outer).unwrap();
        assert!(json.contains("tc_1"));
        assert!(json.contains("tc_2"));
    }

    #[test]
    fn tool_event_stdout_stderr_struct_variants() {
        let out = ToolEvent::Stdout {
            text: "hello".into(),
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("stdout"));
        assert!(json.contains("hello"));
        let back: ToolEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, out);
    }

    #[test]
    fn handoff_tag_detection_simple() {
        let mut state = HandoffDetector::new();
        assert_eq!(state.process("Hello "), Some("Hello ".to_string()));
        assert_eq!(state.process("<kuku_handoff>"), None);
        assert_eq!(state.process("## Goal\nDo stuff"), None);
        assert_eq!(state.process("</kuku_handoff>"), None);
        assert_eq!(state.finish(), Some("## Goal\nDo stuff".to_string()));
    }

    #[test]
    fn handoff_tag_split_across_chunks() {
        let mut state = HandoffDetector::new();
        assert_eq!(
            state.process("reply text<kuku_"),
            Some("reply text".to_string())
        );
        assert_eq!(state.process("handoff>summary</kuku_handoff>"), None);
        assert_eq!(state.finish(), Some("summary".to_string()));
    }

    #[test]
    fn no_handoff_tag_returns_none_on_finish() {
        let mut state = HandoffDetector::new();
        assert_eq!(
            state.process("just normal text"),
            Some("just normal text".to_string())
        );
        assert_eq!(state.finish(), None);
    }

    #[test]
    fn handoff_close_tag_split_across_chunks() {
        let mut state = HandoffDetector::new();
        assert_eq!(state.process("text<kuku_handoff>body"), None);
        assert_eq!(
            state.process("more</kuku_hand"),
            None
        );
        assert_eq!(state.process("off>rest"), None);
        assert_eq!(state.finish(), Some("bodymore".to_string()));
    }

    #[test]
    fn false_start_tag_recovered() {
        let mut state = HandoffDetector::new();
        assert_eq!(state.process("hello <not"), Some("hello <not".to_string()));
        assert_eq!(state.finish(), None);
    }
}
