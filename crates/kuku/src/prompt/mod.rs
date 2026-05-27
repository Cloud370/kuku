pub mod catalog;
pub(crate) mod render;

pub use catalog::{builtin_prompt_catalog, PromptAsset, PromptCatalog};
pub(crate) use catalog::{
    builtin_handoff_instruction, builtin_session_query_guidance, load_prompt_template,
};
pub(crate) use render::{render_project_context, render_runtime_context, ProjectContextInput};
