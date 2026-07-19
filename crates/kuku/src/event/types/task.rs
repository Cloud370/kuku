use std::borrow::Cow;
use std::fmt;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{
    InteractionId, ObservationFact, RequestCompleted, RequestFailed, RequestId, RequestSnapshot,
    RequestStarted, ReviewSubmissionId, ReviewSubmissionRecorded, SkillLoadFact, TaskId,
    WorkspaceId, WorkspaceRelativePath,
};

pub const JSON_SAFE_INTEGER_MAX: u64 = 9_007_199_254_740_991;

pub(super) fn deserialize_json_safe_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value <= JSON_SAFE_INTEGER_MAX {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "integer exceeds JavaScript safe integer maximum",
        ))
    }
}

pub(super) fn deserialize_optional_json_safe_u64<'de, D>(
    deserializer: D,
) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<u64>::deserialize(deserializer)? {
        Some(value) if value > JSON_SAFE_INTEGER_MAX => Err(serde::de::Error::custom(
            "integer exceeds JavaScript safe integer maximum",
        )),
        value => Ok(value),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StorageExhaustionError {
    #[error("{value} exceeds the maximum JSON-safe {kind} value")]
    OutOfRange { kind: &'static str, value: u64 },
    #[error("{kind} storage is exhausted")]
    Exhausted { kind: &'static str },
}

macro_rules! checked_counter {
    ($name:ident, $kind:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u64);

        impl $name {
            pub fn try_new(value: u64) -> Result<Self, StorageExhaustionError> {
                if value <= JSON_SAFE_INTEGER_MAX {
                    Ok(Self(value))
                } else {
                    Err(StorageExhaustionError::OutOfRange {
                        kind: $kind,
                        value,
                    })
                }
            }

            pub fn get(self) -> u64 {
                self.0
            }

            pub fn checked_next(self) -> Result<Self, StorageExhaustionError> {
                if self.0 == JSON_SAFE_INTEGER_MAX {
                    Err(StorageExhaustionError::Exhausted { kind: $kind })
                } else {
                    Ok(Self(self.0 + 1))
                }
            }
        }

        impl TryFrom<u64> for $name {
            type Error = StorageExhaustionError;

            fn try_from(value: u64) -> Result<Self, Self::Error> {
                Self::try_new(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_u64(self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::try_new(u64::deserialize(deserializer)?)
                    .map_err(serde::de::Error::custom)
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "type": "integer",
                    "minimum": 0,
                    "maximum": JSON_SAFE_INTEGER_MAX
                })
            }
        }
    };
}

checked_counter!(Cursor, "cursor");
checked_counter!(TaskRevision, "task revision");

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RevisionToken(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("revision token must contain exactly 64 lowercase hexadecimal characters")]
pub struct RevisionTokenError;

impl RevisionToken {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, RevisionTokenError> {
        let value = value.as_ref();
        if value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            Ok(Self(value.to_owned()))
        } else {
            Err(RevisionTokenError)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RevisionToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for RevisionToken {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RevisionToken {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for RevisionToken {
    fn schema_name() -> Cow<'static, str> {
        "RevisionToken".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^[0-9a-f]{64}$"
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Draft,
    Queued,
    Running,
    NeedsAttention,
    Stopping,
    Completed,
    Stopped,
    Failed,
    Interrupted,
}

impl TaskState {
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Running | Self::NeedsAttention | Self::Stopping
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Queued,
    Running,
    NeedsAttention,
    Stopping,
    Completed,
    Stopped,
    Failed,
    Interrupted,
}

impl RunState {
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Running | Self::NeedsAttention | Self::Stopping
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunFact {
    pub run_id: super::RunId,
    pub task_id: TaskId,
    pub state: RunState,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub summary: Option<String>,
    pub checks: Option<Vec<CheckFact>>,
    pub metrics: Option<Vec<MetricFact>>,
    pub workspace_changes: Option<WorkspaceChangesFact>,
}

impl Eq for RunFact {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckFact {
    pub name: String,
    pub passed: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MetricFact {
    pub name: String,
    pub value: f64,
    pub unit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceChangesFact {
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InteractionChoiceFact {
    pub choice_id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InteractionFact {
    pub interaction_id: InteractionId,
    pub run_id: super::RunId,
    pub prompt: String,
    pub choices: Vec<InteractionChoiceFact>,
    pub selected_choice_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKindFact {
    Tool,
    DelegatedAgent,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatusFact {
    Pending,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileReferenceFact {
    pub workspace_id: WorkspaceId,
    pub relative_path: WorkspaceRelativePath,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessageFact {
    pub message_id: String,
    pub task_id: TaskId,
    pub run_id: Option<super::RunId>,
    pub role: MessageRoleFact,
    pub text: String,
    pub finalized: bool,
    pub request_ids: Vec<RequestId>,
    pub file_references: Vec<FileReferenceFact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageRoleFact {
    User,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActivityFact {
    pub activity_id: String,
    pub run_id: super::RunId,
    pub title: String,
    pub kind: ActivityKindFact,
    pub status: ActivityStatusFact,
    pub detail: Option<String>,
    pub file_references: Vec<FileReferenceFact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillsChangedFact {
    pub tier_id: String,
    pub skill_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewSubmissionReference {
    pub submission_id: ReviewSubmissionId,
    pub task_id: TaskId,
    pub run_id: super::RunId,
    pub task_revision: TaskRevision,
    pub submitted_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "event_type", content = "event", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum TaskEvent {
    TaskCreated {
        task_id: TaskId,
        workspace_id: WorkspaceId,
        title: String,
        created_at: String,
    },
    TaskTitleChanged {
        title: String,
    },
    RunQueued {
        run: RunFact,
    },
    RunStarted {
        run: RunFact,
    },
    RunNeedsAttention {
        run: RunFact,
    },
    RunStopping {
        run: RunFact,
    },
    RunCompleted {
        run: RunFact,
    },
    RunStopped {
        run: RunFact,
    },
    RunFailed {
        run: RunFact,
    },
    RunInterrupted {
        run: RunFact,
    },
    InteractionOpened {
        interaction: InteractionFact,
    },
    InteractionResolved {
        interaction_id: InteractionId,
        choice_id: String,
    },
    InteractionCancelled {
        interaction_id: InteractionId,
    },
    MessageAppended {
        message: MessageFact,
    },
    MessagePatched {
        message_id: String,
        append_text: String,
        finalized: bool,
        request_ids: Option<Vec<RequestId>>,
    },
    ActivityUpserted {
        activity: ActivityFact,
    },
    SkillsChanged {
        selection: SkillsChangedFact,
    },
    SkillLoaded(SkillLoadFact),
    ReviewSubmissionReferenced {
        submission: ReviewSubmissionReference,
    },
    ReviewSubmissionRecorded(ReviewSubmissionRecorded),
    RequestSnapshot(RequestSnapshot),
    RequestStarted(RequestStarted),
    RequestCompleted(RequestCompleted),
    RequestFailed(RequestFailed),
    ObservationRecorded(ObservationFact),
}

impl Eq for TaskEvent {}

impl TaskEvent {
    fn allowed_in_control(&self) -> bool {
        matches!(
            self,
            Self::TaskCreated { .. }
                | Self::TaskTitleChanged { .. }
                | Self::RunQueued { .. }
                | Self::RunStopping { .. }
                | Self::InteractionResolved { .. }
                | Self::MessageAppended { .. }
                | Self::SkillsChanged { .. }
                | Self::ReviewSubmissionReferenced { .. }
                | Self::ReviewSubmissionRecorded(_)
        )
    }

    fn allowed_in_activity(&self) -> bool {
        matches!(
            self,
            Self::RunStarted { .. }
                | Self::RunNeedsAttention { .. }
                | Self::RunCompleted { .. }
                | Self::RunStopped { .. }
                | Self::RunFailed { .. }
                | Self::RunInterrupted { .. }
                | Self::InteractionOpened { .. }
                | Self::InteractionCancelled { .. }
                | Self::MessagePatched { .. }
                | Self::ActivityUpserted { .. }
                | Self::SkillLoaded(_)
                | Self::RequestSnapshot(_)
                | Self::RequestStarted(_)
                | Self::RequestCompleted(_)
                | Self::RequestFailed(_)
                | Self::ObservationRecorded(_)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandIntent {
    CreateTask {
        workspace_id: WorkspaceId,
    },
    SubmitMessage {
        message: String,
        tier_id: String,
        skill_ids: Vec<String>,
    },
    SubmitReview {
        submission_id: ReviewSubmissionId,
    },
    Stop,
    ResolveInteraction {
        interaction_id: InteractionId,
        choice_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandResult {
    TaskCreated { task_id: TaskId },
    RunSubmitted { run_id: super::RunId },
    ReviewSubmitted { submission_id: ReviewSubmissionId },
    Stopped,
    InteractionResolved,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
pub struct CommandReceipt {
    idempotency_key: String,
    intent_digest: String,
    result: CommandResult,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskLedgerError {
    #[error("task ledger record must contain at least one event")]
    Empty,
    #[error("event is not valid in a control transaction")]
    InvalidControlEvent,
    #[error("event is not valid in an activity batch")]
    InvalidActivityEvent,
    #[error("idempotency key and intent digest must be non-empty")]
    EmptyReceiptField,
}

impl CommandReceipt {
    pub fn new(
        idempotency_key: impl Into<String>,
        intent_digest: impl Into<String>,
        result: CommandResult,
    ) -> Result<Self, TaskLedgerError> {
        let idempotency_key = idempotency_key.into();
        let intent_digest = intent_digest.into();
        if idempotency_key.is_empty() || intent_digest.is_empty() {
            return Err(TaskLedgerError::EmptyReceiptField);
        }
        Ok(Self {
            idempotency_key,
            intent_digest,
            result,
        })
    }

    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }
    pub fn intent_digest(&self) -> &str {
        &self.intent_digest
    }
    pub fn result(&self) -> &CommandResult {
        &self.result
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct TaskTransaction {
    pub(crate) task_revision: TaskRevision,
    pub(crate) command: CommandReceipt,
    pub(crate) events: Vec<TaskEvent>,
}

impl Eq for TaskTransaction {}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct TaskActivityBatch {
    pub(crate) events: Vec<TaskEvent>,
}

impl Eq for TaskActivityBatch {}

impl TaskTransaction {
    pub fn try_new(
        task_revision: TaskRevision,
        command: CommandReceipt,
        events: Vec<TaskEvent>,
    ) -> Result<Self, TaskLedgerError> {
        if events.is_empty() {
            return Err(TaskLedgerError::Empty);
        }
        if events.iter().any(|event| !event.allowed_in_control()) {
            return Err(TaskLedgerError::InvalidControlEvent);
        }
        Ok(Self {
            task_revision,
            command,
            events,
        })
    }

    pub fn task_revision(&self) -> TaskRevision {
        self.task_revision
    }
    pub fn command(&self) -> &CommandReceipt {
        &self.command
    }
    pub fn events(&self) -> &[TaskEvent] {
        &self.events
    }
}

impl TaskActivityBatch {
    pub fn try_new(events: Vec<TaskEvent>) -> Result<Self, TaskLedgerError> {
        if events.is_empty() {
            return Err(TaskLedgerError::Empty);
        }
        if events.iter().any(|event| !event.allowed_in_activity()) {
            return Err(TaskLedgerError::InvalidActivityEvent);
        }
        Ok(Self { events })
    }

    pub fn events(&self) -> &[TaskEvent] {
        &self.events
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "record_type", content = "record", rename_all = "snake_case")]
pub enum TaskLedgerRecord {
    Control(TaskTransaction),
    Activity(TaskActivityBatch),
}

impl<'de> Deserialize<'de> for CommandReceipt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            idempotency_key: String,
            intent_digest: String,
            result: CommandResult,
        }
        let value = Wire::deserialize(deserializer)?;
        Self::new(value.idempotency_key, value.intent_digest, value.result)
            .map_err(serde::de::Error::custom)
    }
}

impl Serialize for CommandReceipt {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Wire<'a> {
            idempotency_key: &'a str,
            intent_digest: &'a str,
            result: &'a CommandResult,
        }
        Wire {
            idempotency_key: &self.idempotency_key,
            intent_digest: &self.intent_digest,
            result: &self.result,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TaskTransaction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            task_revision: TaskRevision,
            command: CommandReceipt,
            events: Vec<TaskEvent>,
        }
        let raw = Raw::deserialize(deserializer)?;
        Self::try_new(raw.task_revision, raw.command, raw.events).map_err(serde::de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for TaskActivityBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            events: Vec<TaskEvent>,
        }
        let raw = Raw::deserialize(deserializer)?;
        Self::try_new(raw.events).map_err(serde::de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for TaskLedgerRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "record_type", content = "record", rename_all = "snake_case")]
        enum Raw {
            Control(TaskTransaction),
            Activity(TaskActivityBatch),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Control(value) => Ok(Self::Control(value)),
            Raw::Activity(value) => Ok(Self::Activity(value)),
        }
    }
}
