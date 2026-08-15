mod json;
mod model_stop_reason;
pub(crate) mod scan;
pub mod store;
mod stored_event;
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
    MessageRoleFact, MetricFact, MetricValueError, ModelStopReason, ObservationFact,
    ObservationKind, ObservationRetention, ObservedRange, ProviderFact, ProviderFailureFact,
    ProviderFailureKind, ProviderUsage, RequestCause, RequestCompleted, RequestFailed, RequestId,
    RequestScope, RequestSnapshot, RequestStarted, ReviewAnnotationFact, ReviewSubmissionId,
    ReviewSubmissionRecorded, ReviewSubmissionReference, RevisionToken, RevisionTokenError,
    RollbackScope, RunFact, RunId, RunState, SkillContextFact, SkillLoadFact, SkillLoadOrigin,
    SkillsChangedFact, SourceFact, SourceScope, StorageExhaustionError, StoredEvent,
    TaskActivityBatch, TaskEvent, TaskId, TaskLedgerError, TaskLedgerRecord, TaskRecordClass,
    TaskRevision, TaskState, TaskTransaction, Temperature, TemperatureError, ThinkingConfig,
    ToolResultStatus, TurnId, WorkspaceChangesFact, WorkspaceId, WorkspaceRelativePath,
    WorkspaceRelativePathError, JSON_SAFE_INTEGER_MAX, MAX_WORKSPACE_RELATIVE_PATH_BYTES,
};

pub fn next_turn_index(events: &[StoredEvent]) -> u64 {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::TurnStarted { turn, .. } => Some(*turn),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        + 1
}

pub fn task_execution_scope(
    events: &[StoredEvent],
    workspace_id: &WorkspaceId,
    task_id: &TaskId,
    run_id: &RunId,
    conversation: &str,
) -> Result<ExecutionScope, ExecutionIdError> {
    if let Some(scope) = events.iter().rev().find_map(|event| {
        let EventPayload::TaskLedger(record) = &event.payload else {
            return None;
        };
        let events = match record {
            TaskLedgerRecord::Control(transaction) => transaction.events(),
            TaskLedgerRecord::Activity(batch) => batch.events(),
        };
        events.iter().rev().find_map(|event| match event {
            TaskEvent::SkillLoaded(skill) if &skill.execution.run_id == run_id => {
                Some(skill.execution.clone())
            }
            _ => None,
        })
    }) {
        return Ok(scope);
    }
    let turn_index = completed_turn_index(events, conversation) + 1;
    Ok(ExecutionScope {
        workspace_id: workspace_id.clone(),
        task_id: task_id.clone(),
        run_id: run_id.clone(),
        turn_id: derived_turn_id(workspace_id, task_id, run_id, conversation, turn_index),
        conversation_id: ConversationId::for_task_address(task_id, conversation)?,
        turn_index,
    })
}

fn completed_turn_index(events: &[StoredEvent], conversation: &str) -> u64 {
    events
        .iter()
        .filter(|event| {
            matches!(
                &event.payload,
                EventPayload::TurnCompleted {
                    conversation: event_conversation,
                    ..
                } if event_conversation == conversation
            )
        })
        .count() as u64
}

fn derived_turn_id(
    workspace_id: &WorkspaceId,
    task_id: &TaskId,
    run_id: &RunId,
    conversation: &str,
    turn_index: u64,
) -> TurnId {
    use sha2::{Digest, Sha256};

    let digest = format!(
        "{:x}",
        Sha256::digest(
            format!(
                "{}:{}:{}:{}:{}",
                workspace_id.as_str(),
                task_id.as_str(),
                run_id.as_str(),
                conversation,
                turn_index
            )
            .as_bytes()
        )
    );
    TurnId::parse(format!("trn_{}", &digest[..24]))
        .expect("sha256-derived turn id must be a valid turn id")
}

#[cfg(test)]
#[allow(dead_code)]
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
#[allow(dead_code)]
pub(crate) fn test_request_scope(seed: impl AsRef<str>) -> RequestScope {
    use sha2::{Digest, Sha256};

    let digest = format!("{:x}", Sha256::digest(seed.as_ref().as_bytes()));
    RequestScope {
        execution: test_execution_scope(),
        request_id: RequestId::parse(format!("req_{}", &digest[..24])).unwrap(),
    }
}
