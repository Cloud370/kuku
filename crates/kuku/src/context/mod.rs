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

pub(crate) fn estimate_text_tokens(text: &str) -> u32 {
    if text.is_ascii() {
        return u32::try_from(text.len().div_ceil(4)).unwrap_or(u32::MAX);
    }
    let (ascii, other) = text.chars().fold((0u32, 0u32), |(ascii, other), ch| {
        if ch.is_ascii() {
            (ascii.saturating_add(1), other)
        } else {
            (ascii, other.saturating_add(1))
        }
    });
    ascii.div_ceil(4).saturating_add(other)
}
