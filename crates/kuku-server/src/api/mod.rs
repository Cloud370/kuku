use std::borrow::Cow;
use std::fmt;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

mod context;
pub mod contract_export;
mod error;
mod platform;
mod review;
mod schema;
mod task;

pub use context::{
    AgentCatalogEntry, AgentSummary, AgentThread, CapabilityProjection, CatalogQuery,
    ContextBreakdownProjection, ContextCatalog, ContextHealth, ContextHealthLevel, ContextSections,
    ContextSnapshot, ContextSummary, ContextUsage, ContextWarning, ContextWarningCode,
    ConversationContext, DelegatedAgentProjection, DelegatedAgentStatus, DiscoverableContext,
    InstructionContextItem, MemoryContextItem, ObservationContextItem, RequestStatus,
    RequestSummary, SkillCatalogEntry, SkillContextItem, TierCatalogEntry, TierSummary,
    ToolCatalogEntry, UsageSummary,
};
pub use error::{ApiError, ApiErrorCode};
pub use platform::{
    AuthMode, AuthStatus, CompleteInitRequest, ConnectionInfo, CredentialInput, CredentialStatus,
    InitPhase, InitStatus, PlatformCatalog, PlatformStatus, ProviderDraft,
    RegisterInitialWorkspaceRequest, RegisterWorkspaceRequest, RegistrationRootId,
    RegistrationRootPage, RegistrationRootSummary, RemoveWorkspaceRequest, SettingsPatch,
    SettingsSnapshot, TestProviderRequest, TestProviderResult, TierDraft, UpdateDefaultTierRequest,
    UpdateProvidersRequest, UpdateSettingsRequest, WorkspaceAvailability, WorkspacePage,
    WorkspaceSummary,
};
pub use review::{
    AnnotationBatch, AnnotationDraft, AnnotationStatus, ChangeEntry, ChangeKind,
    ChangesAvailability, ChangesQuery, DiffDocument, DiffHunk, DiffLine, DiffLineKind, DiffQuery,
    FileContent, FileContentQuery, FileEntry, FileKind, FilePage, FileSearchPage, FileSearchQuery,
    FileTreeQuery, ReviewSnapshot, ReviewSubmissionPage, ReviewSubmissionProjection,
    ReviewSubmissionQuery, ReviewSubmissionResult, ReviewSubmissionsChanged,
    ReviewSummaryProjection, SearchMatch, SubmittedReviewNote, TextRange,
};
pub use schema::WebApiContract;
pub use task::{
    ActivityKind, ActivityProjection, ActivityStatus, CheckProjection, CommandAccepted,
    CompletionProjection, CreateTaskRequest, CreateTaskResponse, FileReferenceProjection,
    InteractionChoiceProjection, InteractionProjection, InteractionResponseRequest,
    InteractionStatus, ListTasksQuery, LoadedSkillProjection, MessageProjection, MessageRole,
    MetricProjection, RunProjection, StopRunRequest, SubmitRunRequest, SubmitRunResponse,
    TaskChange, TaskDelta, TaskPage, TaskProjection, TaskStreamEvent, TaskStreamQuery, TaskSummary,
    TimelineItemProjection, TimelinePage, TimelineQuery, TimelineWindowDelta,
};

pub use kuku::event::{
    AnnotationSide, CapabilityFact, CapabilityKind, CapabilityState, ContextBreakdown,
    ConversationContextFact, ConversationId, CurrencyCode, Cursor, DecimalCost,
    DelegatedResultFact, ExactContentBlock, ExactMessage, ExactRequest, ExactRequestParameters,
    ExactTool, ExecutionScope, InstructionContextFact, InstructionKind, InteractionId,
    MemoryContextFact, MemoryKind, ObservationFact, ObservationKind, ObservationRetention,
    ObservedRange, ProviderFact, ProviderFailureFact, ProviderFailureKind, ProviderUsage,
    RequestCause, RequestCompleted, RequestFailed, RequestId, RequestScope, RequestSnapshot,
    RequestStarted, ReviewAnnotationFact, ReviewSubmissionId, ReviewSubmissionRecorded,
    RevisionToken, RunId, RunState, SkillContextFact, SkillLoadFact, SkillLoadOrigin, SourceFact,
    SourceScope, TaskId, TaskRevision, TaskState, Temperature, ThinkingConfig, ToolResultStatus,
    TurnId, WorkspaceId, WorkspaceRelativePath,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ApiVersion;

impl Serialize for ApiVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u8(1)
    }
}

impl<'de> Deserialize<'de> for ApiVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match u8::deserialize(deserializer)? {
            1 => Ok(Self),
            _ => Err(serde::de::Error::custom("api_version must be 1")),
        }
    }
}

impl JsonSchema for ApiVersion {
    fn schema_name() -> Cow<'static, str> {
        "ApiVersion".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({"type": "integer", "const": 1})
    }
}

pub const MAX_PAGE_CURSOR_BYTES: usize = 2_048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursorError;

impl fmt::Display for PageCursorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("page cursor must contain between 1 and 2048 bytes")
    }
}

impl std::error::Error for PageCursorError {}

impl PageCursor {
    pub fn try_new(value: impl Into<String>) -> Result<Self, PageCursorError> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_PAGE_CURSOR_BYTES {
            Err(PageCursorError)
        } else {
            Ok(Self(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for PageCursor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for PageCursor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::try_new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for PageCursor {
    fn schema_name() -> Cow<'static, str> {
        "PageCursor".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({"type": "string", "minLength": 1, "maxLength": 2048})
    }
}
