pub(crate) mod catalog;
pub(crate) mod render;

pub(crate) use catalog::builtin_prompt_catalog;
pub(crate) use render::{render_synthetic_user, SyntheticUserTemplateInput};
