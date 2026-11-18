use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    AgentConfig, AgentSelection, AgentSnapshot, AgentStatus, InputStatus, ModelPhase,
    OutputReference, PermissionRequest, QuestionRequest, QueuedInput, Receipt, StoredEvent,
};

/// The exact server-side workspace, Session, and Agent selected by a caller.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Target {
    /// Canonical, absolute workspace path on the service host.
    pub workspace: PathBuf,
    /// Stable Session identity.
    pub session_id: String,
    /// Stable Session-local Agent address.
    pub agent: String,
}

/// An immutable command, kept intact until its durable receipt is confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    /// Workspace, Session, and recipient of the command.
    pub target: Target,
    /// Caller-selected idempotency key in this Session.
    pub command_id: String,
    /// Exact command intent.
    pub action: Action,
}

/// When an input becomes eligible for acceptance by an active Agent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Timing {
    /// Accept at the next safe Step, or wake an idle Agent.
    #[default]
    Steer,
    /// Accept at the start of the next Turn.
    NextTurn,
}

/// A durable mutation the service can associate with a caller command ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Deliver one input and optionally establish the Session and its initial Agent.
    Send {
        /// Original user input.
        text: String,
        /// The boundary where the Agent may accept the input.
        timing: Timing,
        /// Exact saved output references attached to the input.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        values: Vec<OutputReference>,
        /// Optional Skill instructions selected for this input's acceptance boundary.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bootstrap_skill: Option<String>,
        /// Intent to initialize a new Session and create its first Agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        create: Option<Box<AgentConfig>>,
        /// Optional Agent selection changes made as part of this new input.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config_patch: Option<Box<AgentConfig>>,
        /// Optional server-side prompt directory for the new or reopened Session.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompts_dir: Option<PathBuf>,
        /// Allow a fresh script input to create its target in an existing Session.
        #[serde(default)]
        create_agent_if_missing: bool,
    },
    /// Establish a separate Agent address in an existing Session.
    CreateAgent {
        /// Agent-local initial configuration.
        config: Box<AgentConfig>,
    },
    /// Set a new Agent configuration for a safe execution boundary.
    Configure {
        /// Full Agent-local configuration.
        config: Box<AgentConfig>,
    },
    /// Answer one exact durable question.
    ReplyQuestion {
        /// The question being answered.
        request_id: String,
        /// The user's answer.
        text: String,
    },
    /// Decide one exact authorization request.
    ReplyPermission {
        /// The authorization being decided.
        request_id: String,
        /// True if the user allows the bound action.
        allow: bool,
    },
    /// Withdraw an input that is still queued.
    Withdraw {
        /// The input's original command identity.
        input_command_id: String,
    },
    /// Replace the text of one input before it is admitted, preserving its queue position.
    ReplaceQueued {
        /// The input's original command identity.
        input_command_id: String,
        /// The text delivered under this replacement command's identity.
        text: String,
    },
    /// Explicitly let an Agent advance eligible work.
    Advance,
    /// Interrupt the specific execution the client observed.
    Cancel {
        /// Opaque execution identity from the Agent snapshot or a progress event.
        execution_id: u64,
    },
}

/// A durable receipt and its command's currently known disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Acknowledgement {
    /// Stable receipt committed by the service.
    pub receipt: Receipt,
    /// Whether this response was returned for an exact previously committed command.
    #[serde(default)]
    pub replayed: bool,
    /// Current input status when applicable.
    #[serde(default)]
    pub status: Option<InputStatus>,
}

/// Service error code and user-facing explanation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    /// Stable error code from the service protocol.
    pub code: String,
    /// Explanation supplied by the service.
    pub message: String,
}

/// An Agent's brief state, preserving the execution identity used to interrupt it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSummary {
    /// Stable Session-local address.
    pub address: String,
    /// Current execution state.
    pub status: AgentStatus,
    /// Latest accepted Turn, when present.
    pub turn: Option<u64>,
    /// Current execution identity, when present.
    pub execution_id: Option<u64>,
    /// Number of inputs awaiting acceptance.
    pub pending_inputs: usize,
    /// Excerpt of the most recent task input admitted by the Agent.
    pub admitted_task: Option<String>,
    /// Excerpt of the most recent task input still in the queue.
    pub queued_task: Option<String>,
    /// Matching durable question if the Agent is awaiting an answer.
    pub question: Option<QuestionRequest>,
    /// Matching authorization if the Agent is awaiting a decision.
    pub permission: Option<PermissionRequest>,
    /// Short excerpt of the most recent durable reply, when available.
    pub last_output_preview: Option<String>,
    /// Short excerpt of the most recent durable failure, when available.
    pub last_error_preview: Option<String>,
    /// Bounded execution history for this Agent, when requested and available.
    pub progress: Option<AgentProgress>,
}

/// State and output recorded when one Agent's turn settles.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentSettlement {
    /// Status after the turn ends or reaches a durable wait.
    pub status: AgentStatus,
    /// The settled Turn, when the event carries its identity.
    pub turn: Option<u64>,
    /// Result text or a bounded excerpt, according to observation scope.
    pub output: Option<String>,
    /// Failure text or a bounded excerpt, according to observation scope.
    pub error: Option<String>,
}

/// Latest identifiable phase from an Agent's durable execution or live provider activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum AgentProgress {
    /// A prepared model request has no committed response yet.
    WaitingForModel,
    /// The provider has emitted reasoning activity for the current request.
    Thinking,
    /// The provider is preparing a tool call that has not been dispatched.
    ToolArguments,
    /// Visible reply text was streaming at this snapshot's boundary.
    Streaming {
        /// Bounded text captured from the beginning of this request, if emitted.
        preview: String,
    },
    /// One or more dispatched tool calls have no committed result yet.
    RunningTools {
        /// Identities of the unfinished calls found within the history window.
        tool_call_ids: Vec<String>,
    },
}

/// State at one committed Session cursor, suitable for race-free subscription.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Snapshot {
    /// Stable Session identity.
    pub session_id: String,
    /// Canonical workspace on the service machine.
    pub workspace: PathBuf,
    /// Last durable Session event included in this state.
    pub cursor: u64,
    /// Full state for the selected Agent.
    pub agent: AgentSnapshot,
    /// Brief state for every Agent in the Session.
    pub agents: Vec<AgentSummary>,
    /// Current queue for the selected Agent.
    pub pending: Vec<QueuedInput>,
    /// Effective model, settings, and tool selection.
    pub selection: Option<AgentSelection>,
    /// Any error resolving the Agent's configuration on the service host.
    pub selection_error: Option<ApiError>,
    /// Selected model and tools queued for the Agent's next safe boundary.
    pub pending_selection: Option<AgentSelection>,
    /// Any error resolving the pending Agent configuration.
    pub pending_selection_error: Option<ApiError>,
    /// The stopped execution's owned work is still cleaning up.
    #[serde(default)]
    pub cleaning_up: bool,
    /// Bounded execution history for a new observer, when requested and available.
    pub progress: Option<AgentProgress>,
    /// Selected Agent's observed provider progress at this snapshot cursor.
    pub reply_preview: Option<ReplyPreview>,
}

impl Snapshot {
    /// Find the execution identity observed for this snapshot's selected Agent.
    pub fn execution_id(&self) -> Option<u64> {
        self.agent.execution_id
    }
}

/// A bounded prefix of the selected Agent's active reply.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReplyPreview {
    /// Identity of the execution currently producing the preview.
    pub execution_id: u64,
    /// Durable Turn identity.
    pub turn: u64,
    /// One-based Turn display number.
    pub turn_number: Option<u64>,
    /// Most recent provider activity in this request.
    pub phase: ModelPhase,
    /// Observed text from the beginning of the current request.
    pub text: String,
    /// Provider-visible reasoning text or summary, separate from reply text and provider replay.
    #[serde(default)]
    pub reasoning_preview: String,
    /// Additional summary text exists beyond this prefix.
    #[serde(default)]
    pub reasoning_preview_truncated: bool,
    /// Further text already exists past the preview limit.
    pub truncated: bool,
}

/// A displayable durable entry distinct from uncommitted streaming previews.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Durable Session event identity.
    pub id: u64,
    /// Agent that owns the entry.
    pub agent: String,
    /// Turn that produced the entry, if applicable.
    pub turn: Option<u64>,
    /// Entry type used to select a general or tool-specific view.
    #[serde(rename = "type")]
    pub kind: HistoryKind,
    /// Displayable message when the entry has one.
    pub text: Option<String>,
    /// Measured work attributed to this Turn at its settlement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_summary: Option<crate::TurnSummary>,
    /// Type-specific structured fields, including optional tool output references.
    #[serde(flatten)]
    pub details: serde_json::Map<String, Value>,
}

/// Known display roles in committed history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryKind {
    /// Input accepted from the user or host.
    User,
    /// Durable assistant message.
    Assistant,
    /// Model-requested tool invocation.
    ToolCall,
    /// Durable outcome of a tool invocation.
    ToolResult,
    /// Persisted question request.
    Question,
    /// Persisted authorization request.
    Permission,
    /// A matching host answer to a retained question.
    QuestionReply,
    /// A matching host decision for a retained authorization.
    PermissionReply,
    /// Aggregate work measured for a settled Turn.
    TurnSummary,
    /// Durable cancellation or interruption status.
    Status,
    /// Persisted execution failure.
    Error,
    /// A display role introduced by a later service build.
    #[serde(other)]
    Other,
}

/// One bounded page of displayable entries in ascending event order.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HistoryPage {
    /// Entries within the requested Session boundary.
    pub entries: Vec<HistoryEntry>,
    /// Exclusive `before` cursor for the previous page, when available.
    pub next_before: Option<u64>,
}

/// A raw ledger page for script consumers and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EventPage {
    /// Persisted SDK event records.
    pub events: Vec<StoredEvent>,
    /// Exclusive cursor for the next page, including scanned events excluded by filters.
    pub next_after: u64,
}

/// The overall state visible in session listings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// The service has an active session.
    #[serde(alias = "Active")]
    Active,
    /// Work finished cleanly.
    #[serde(alias = "Done")]
    Done,
    /// The last execution ended unexpectedly or has not completed.
    #[serde(alias = "Interrupted")]
    Interrupted,
}

/// A readable summary used to pick a stored Session.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SessionSummary {
    /// Stable Session identity.
    pub session_id: String,
    /// Workspace on the service host.
    pub workspace: PathBuf,
    /// Title or initial input excerpt.
    pub title: String,
    /// When the Session was created.
    pub created_at: String,
    /// Number of Turns in the retained history.
    pub turn_count: u64,
    /// Overall execution state.
    pub status: SessionStatus,
    /// Time of the most recently persisted activity.
    pub mtime: String,
    /// Size of the Session journal in bytes.
    pub size: u64,
    /// Agents in this Session awaiting a reply.
    #[serde(default)]
    pub waiting_agents: Vec<String>,
}
