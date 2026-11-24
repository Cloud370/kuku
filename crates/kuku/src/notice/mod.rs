pub(crate) mod assembly;
pub(crate) mod render;
pub(crate) mod types;

pub(crate) use assembly::{NoticeAssemblyInput, build_runtime_notices};
pub(crate) use render::render_notice_body;
pub(crate) use types::{ContextHeadroom, compute_context_headroom};
