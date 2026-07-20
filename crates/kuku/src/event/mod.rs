pub(crate) mod scan;
pub mod store;
pub mod types;

pub use store::EventStore;
pub use types::{
    ActivityFact, ActivityKindFact, ActivityStatusFact, AnnotationSide, CapabilityFact,
    CapabilityKind, CapabilityState, ChangeEntryFact, ChangeKindFact, ChangesAvailabilityFact,
    CheckFact, CommandIntent, CommandReceipt, CommandResult, ContextBreakdown,
    ConversationContextFact, ConversationId, CurrencyCode, Cursor, DecimalCost,
    DelegatedResultFact, EventPayload, ExactContentBlock, ExactMessage, ExactRequest,
    ExactRequestParameters, ExactTool, ExecutionIdError, ExecutionScope, FileReferenceFact,
    FiniteMetricValue, InstructionContextFact, InstructionKind, InteractionChoiceFact,
    InteractionFact, InteractionId, MemoryContextFact, MemoryKind, MessageFact, MessageRole,
    MessageRoleFact, MetricFact, MetricValueError, ObservationFact, ObservationKind,
    ObservationRetention, ObservedRange, ProviderFact, ProviderFailureFact, ProviderFailureKind,
    ProviderUsage, RequestCause, RequestCompleted, RequestFailed, RequestId, RequestScope,
    RequestSnapshot, RequestStarted, ReviewAnnotationFact, ReviewSubmissionId,
    ReviewSubmissionRecorded, ReviewSubmissionReference, RevisionToken, RevisionTokenError,
    RollbackScope, RunFact, RunId, RunState, SkillContextFact, SkillLoadFact, SkillLoadOrigin,
    SkillsChangedFact, SourceFact, SourceScope, StorageExhaustionError, StoredEvent,
    TaskActivityBatch, TaskEvent, TaskId, TaskLedgerError, TaskLedgerRecord, TaskRecordClass,
    TaskRevision, TaskState, TaskTransaction, Temperature, TemperatureError, ThinkingConfig,
    ToolResultStatus, TurnId, WorkspaceChangesFact, WorkspaceId, WorkspaceRelativePath,
    WorkspaceRelativePathError, JSON_SAFE_INTEGER_MAX, MAX_WORKSPACE_RELATIVE_PATH_BYTES,
};

#[cfg(test)]
pub(crate) fn test_execution_scope() -> ExecutionScope {
    ExecutionScope {
        workspace_id: WorkspaceId::parse("wsp_111111111111111111111111").unwrap(),
        task_id: TaskId::parse("tsk_222222222222222222222222").unwrap(),
        run_id: RunId::parse("run_333333333333333333333333").unwrap(),
        turn_id: TurnId::parse("trn_444444444444444444444444").unwrap(),
        conversation_id: ConversationId::parse("con_555555555555555555555555").unwrap(),
        turn_index: 1,
    }
}

#[cfg(test)]
pub(crate) fn test_request_scope(seed: impl AsRef<str>) -> RequestScope {
    use sha2::{Digest, Sha256};

    let digest = format!("{:x}", Sha256::digest(seed.as_ref().as_bytes()));
    RequestScope {
        execution: test_execution_scope(),
        request_id: RequestId::parse(format!("req_{}", &digest[..24])).unwrap(),
    }
}
