pub mod assembly;
pub mod catalog;
mod fact_sink;
pub mod message;
pub mod observations;
pub mod provenance;
pub mod provider_hook;
pub mod replay;
mod request_evidence;
mod request_snapshot;
pub mod revert;
mod usage;

pub use assembly::{
    assemble_context, restore_prompt_snapshot, ContextAssembly, ContextInput, EnvironmentSource,
    HostResponseContract, InstructionSource, MemorySource, ToolSchema,
};
pub use fact_sink::{ContextFactSink, ContextFactSinkError, EventStoreContextFactSink};
pub use message::{CanonicalMessage, MessageBlock, Role, ToolResult, ToolUse};
pub use provenance::{
    AgentRegistryProvenance, FileSource, HistoryRange, PluginRegistryProvenance,
    PromptCapabilityMetadata, PromptRendererIdentity, RequestProvenance, SkillRegistryProvenance,
    ToolRegistryProvenance,
};
pub use provider_hook::{begin_provider_request, ProviderHookError};
pub use replay::rebuild_history;
pub(crate) use replay::rebuild_history_for_provider;
pub use request_evidence::{DurableRequestEvidenceRecorder, RequestEvidenceRecorder};
pub use request_snapshot::{
    RequestIdAccumulator, RequestSnapshotBuilder, SnapshotBuildError, SnapshotInput,
    MAX_REQUEST_SNAPSHOT_BYTES,
};
pub use revert::{
    apply_file_revert, compute_file_revert_plan, count_file_turns_after, find_active_rollback,
    list_user_turns, rollback_turn, undo_rollback, ActiveRollback, FileRestore, RevertPlan,
    RollbackResult, UndoRollbackResult, UserTurnEntry,
};
pub use usage::{UsageAggregate, UsageAggregateSummary, UsageReductionError};
