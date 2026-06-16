pub mod catalog;
pub(crate) mod render;

pub(crate) use catalog::{builtin_handoff_instruction, load_prompt_template};
pub use catalog::{builtin_prompt_catalog, PromptAsset, PromptCatalog};
pub(crate) use render::{render_project_context, ProjectContextInput};
