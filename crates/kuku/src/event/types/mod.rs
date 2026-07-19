//! Event payload and persistence types.

mod codec;
mod identity;
mod payload;
mod request;
mod review;
mod stored;
mod task;

pub use identity::{
    ConversationId, ExecutionIdError, ExecutionScope, InteractionId, RequestId, RequestScope,
    ReviewSubmissionId, RunId, TaskId, TurnId, WorkspaceId,
};
pub use payload::{ContextMessage, EventPayload, RollbackScope};
pub use request::{
    CurrencyCode, DecimalCost, ProviderFact, ProviderFailureFact, ProviderFailureKind,
    ProviderUsage, RequestCause, RequestCompleted, RequestFailed, RequestStarted,
};
pub use review::{AnnotationSide, ReviewAnnotationFact, ReviewSubmissionRecorded};
pub use stored::StoredEvent;
pub use task::{
    Cursor, RevisionToken, RevisionTokenError, RunState, StorageExhaustionError, TaskRevision,
    TaskState, JSON_SAFE_INTEGER_MAX,
};

#[cfg(test)]
mod tests;
