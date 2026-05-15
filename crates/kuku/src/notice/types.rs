#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoticeKind {
    ContextDrift { entries: Vec<ContextDriftEntry> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoticeSeverity {
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Notice {
    pub(crate) kind: NoticeKind,
    pub(crate) severity: NoticeSeverity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextDriftEntry {
    pub(crate) path: String,
    pub(crate) status: ContextDriftStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextDriftStatus {
    Updated,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ContextBudgetTier {
    Tight,
    Normal,
    Roomy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextHeadroom {
    pub(crate) max_context_tokens: u32,
    pub(crate) reserved_output_tokens: u32,
    pub(crate) reserved_margin_tokens: u32,
    pub(crate) estimated_input_tokens: Option<u32>,
    pub(crate) remaining_input_tokens: Option<u32>,
    pub(crate) tier: ContextBudgetTier,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_budget_tier_orders_from_tight_to_roomy() {
        assert!(ContextBudgetTier::Tight < ContextBudgetTier::Normal);
        assert!(ContextBudgetTier::Normal < ContextBudgetTier::Roomy);
    }

    #[test]
    fn context_drift_status_supports_updated_and_deleted() {
        let updated = ContextDriftStatus::Updated;
        let deleted = ContextDriftStatus::Deleted;
        assert_ne!(format!("{updated:?}"), format!("{deleted:?}"));
    }
}
