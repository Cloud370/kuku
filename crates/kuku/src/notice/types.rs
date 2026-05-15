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

pub(crate) fn compute_context_headroom(
    max_context_tokens: u32,
    reserved_output_tokens: Option<u32>,
    estimated_input_tokens: Option<u32>,
) -> ContextHeadroom {
    let reserved_output_tokens = reserved_output_tokens.unwrap_or(8_192);
    let reserved_margin_tokens = 2_048;
    let remaining_input_tokens = estimated_input_tokens.map(|used| {
        max_context_tokens
            .saturating_sub(reserved_output_tokens)
            .saturating_sub(reserved_margin_tokens)
            .saturating_sub(used)
    });
    let tier = match remaining_input_tokens {
        Some(remaining) if remaining < 8_000 => ContextBudgetTier::Tight,
        Some(remaining) if remaining < 32_000 => ContextBudgetTier::Normal,
        Some(_) => ContextBudgetTier::Roomy,
        None => ContextBudgetTier::Normal,
    };

    ContextHeadroom {
        max_context_tokens,
        reserved_output_tokens,
        reserved_margin_tokens,
        estimated_input_tokens,
        remaining_input_tokens,
        tier,
    }
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

    #[test]
    fn headroom_defaults_to_normal_when_token_estimate_is_missing() {
        let headroom = compute_context_headroom(200_000, None, None);
        assert_eq!(headroom.tier, ContextBudgetTier::Normal);
    }

    #[test]
    fn headroom_maps_low_remaining_budget_to_tight() {
        let headroom = compute_context_headroom(50_000, Some(8_000), Some(41_000));
        assert_eq!(headroom.tier, ContextBudgetTier::Tight);
    }

    #[test]
    fn headroom_maps_large_remaining_budget_to_roomy() {
        let headroom = compute_context_headroom(200_000, Some(8_000), Some(20_000));
        assert_eq!(headroom.tier, ContextBudgetTier::Roomy);
    }
}
