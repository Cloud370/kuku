pub mod catalog;
pub(crate) mod render;

pub use catalog::{PromptAsset, PromptCatalog, builtin_prompt_catalog};
pub(crate) use catalog::{builtin_handoff_instruction, load_prompt_template};
pub(crate) use render::{ProjectContextInput, render_project_context, render_runtime_date};
