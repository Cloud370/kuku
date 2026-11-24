//! HTTP access, durable command recovery, and resumable observation for kuku clients.

mod connection;
mod http;
pub mod local;
mod pending;
mod subscription;
mod types;
mod workspace;

pub use connection::{Connection, ConnectionOptions};
pub use http::{Client, Error, Health};
pub use kuku::adapter::ModelSelection;
pub use kuku::event::StoredEvent;
pub use kuku::execution::{
    CommandExecutionFacts, CommandInvocation, CommandResultKind, CommandShell, CommandStdin,
    CommandTermination, FileCommitState, FileMutationFacts, FileMutationKind, OutputReference,
    ReportedTests, TestReportFormat,
};
pub use kuku::permission::PermissionMode;
pub use kuku::query::ModelPhase;
pub use kuku::session::runtime::SessionCostSummary;
pub use kuku::session::{FileRewindChange, FileRewindPreview};
pub use kuku::{
    AgentConfig, AgentSelection, AgentSnapshot, AgentStatus, Input, InputSource, InputStatus,
    InputTiming, JobResult, JobSnapshot, JobStatus, MAX_INPUT_VALUES, MAX_VALUE_NAME_BYTES,
    NamedValue, NamedValueKind, NamedValueSource, OutputChunk, OutputSegment, OutputStream,
    PendingInput as QueuedInput, PermissionRequest, QuestionRequest, Receipt, TurnSummary,
    UsageTotals, WorkspaceScriptActionFacts, WorkspaceScriptActionStatus, WorkspaceScriptFacts,
    WorkspaceScriptOperation, WorkspaceScriptResultKind,
};
pub use pending::PendingStore;
pub use subscription::{InputDelivery, Observation, Subscription, SubscriptionUpdate};
pub use types::{
    Acknowledgement, Action, AgentProgress, AgentSettlement, AgentSummary, ApiError, Command,
    EventPage, ForwardHistoryPage, HistoryDestination, HistoryDestinations, HistoryEntry,
    HistoryKind, HistoryPage, ModelDefaultPlan, ReplyPreview, RetryFailureKind, RetryMetadata,
    RetryPhase, RetrySource, RetryStopReason, RewindGroup, RewindGroupPage, SessionStatus,
    SessionSummary, Snapshot, Target, Timing,
};
pub use workspace::{ConnectionSummary, ModelCatalog, SkillSummary, TierSummary};

/// An unresolved approval and its execution context.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PendingApproval {
    /// The exact request identity and action to approve or deny.
    pub request: PermissionRequest,
    /// Background Job waiting for the decision, when applicable.
    pub job_id: Option<String>,
}

/// Generate a new independent Session or command identity.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
