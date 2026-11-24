pub mod assembly;
pub mod message;
pub mod provenance;
pub mod replay;
pub mod revert;
pub(crate) mod runtime;

pub use assembly::{
    ContextAssembly, ContextInput, EnvironmentSource, HostResponseContract, InstructionSource,
    MemorySource, ToolSchema, assemble_context, restore_prompt_snapshot,
};
pub use message::{CanonicalMessage, MessageBlock, Role, ToolResult, ToolUse};
pub use provenance::{
    AgentRegistryProvenance, FileSource, HistoryRange, PluginRegistryProvenance,
    PromptCapabilityMetadata, PromptRendererIdentity, RequestProvenance, SkillRegistryProvenance,
    ToolRegistryProvenance,
};
pub use replay::rebuild_history;
pub(crate) use replay::rebuild_history_for_provider;
