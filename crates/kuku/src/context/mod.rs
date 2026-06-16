pub mod assembly;
pub mod message;
pub mod provenance;
pub mod replay;
pub mod revert;

pub use assembly::{
    assemble_context, restore_prompt_snapshot, ContextAssembly, ContextInput, EnvironmentSource,
    HostResponseContract, InstructionSource, MemorySource, ToolSchema,
};
pub use message::{CanonicalMessage, MessageBlock, Role, ToolResult, ToolUse};
pub use provenance::{
    AgentRegistryProvenance, FileSource, HistoryRange, PluginRegistryProvenance,
    PromptCapabilityMetadata, PromptRendererIdentity, RequestProvenance, SkillRegistryProvenance,
    ToolRegistryProvenance,
};
pub use replay::rebuild_history;
pub(crate) use replay::rebuild_history_for_provider;
pub use revert::{
    apply_file_revert, compute_file_revert_plan, count_file_turns_after, find_active_rollback,
    list_user_turns, rollback_turn, undo_rollback, ActiveRollback, FileRestore, RevertPlan,
    RollbackResult, UndoRollbackResult, UserTurnEntry,
};
