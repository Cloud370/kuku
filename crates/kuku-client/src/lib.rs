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
pub use kuku::{
    AgentConfig, AgentSelection, AgentSnapshot, AgentStatus, Input, InputSource, InputStatus,
    InputTiming, OutputChunk, OutputSegment, OutputStream, PendingInput as QueuedInput,
    PermissionRequest, QuestionRequest, Receipt, UsageTotals,
};
pub use pending::PendingStore;
pub use subscription::{InputDelivery, Observation, Subscription, SubscriptionUpdate};
pub use types::{
    Acknowledgement, Action, AgentProgress, AgentSettlement, AgentSummary, ApiError, Command,
    EventPage, HistoryEntry, HistoryKind, HistoryPage, ReplyPreview, SessionStatus, SessionSummary,
    Snapshot, Target, Timing,
};
pub use workspace::{ConnectionSummary, ModelCatalog, SkillSummary, TierSummary};

/// Generate a new independent Session or command identity.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
