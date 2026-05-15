pub(crate) mod render;
pub(crate) mod types;

pub(crate) use render::render_notice_block;
pub(crate) use types::{
    compute_context_headroom, ContextBudgetTier, ContextDriftEntry, ContextDriftStatus,
    ContextHeadroom, Notice, NoticeKind, NoticeSeverity,
};
