pub mod catalog;
pub(crate) mod render;

pub use catalog::{builtin_prompt_catalog, PromptAsset, PromptCatalog};
pub(crate) use render::{render_synthetic_user, SyntheticUserTemplateInput};
