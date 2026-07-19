//! Event payload and persistence types.

mod codec;
mod context;
mod identity;
mod payload;
mod request;
mod review;
mod stored;
mod task;

pub use context::{
    CapabilityFact, CapabilityKind, CapabilityState, ContextBreakdown, ConversationContextFact,
    DelegatedResultFact, ExactContentBlock, ExactMessage, ExactRequest, ExactRequestParameters,
    ExactTool, InstructionContextFact, InstructionKind, MemoryContextFact, MemoryKind, MessageRole,
    ObservationFact, ObservationKind, ObservationRetention, ObservedRange, RequestSnapshot,
    SkillContextFact, SkillLoadFact, SkillLoadOrigin, SourceFact, SourceScope, Temperature,
    TemperatureError, ThinkingConfig, ToolResultStatus, WorkspaceRelativePath,
    WorkspaceRelativePathError, MAX_WORKSPACE_RELATIVE_PATH_BYTES,
};
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
