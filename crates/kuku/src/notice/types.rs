use crate::conversation::address::ConversationAddress;

const CONTEXT_OUTPUT_RESERVE_TOKENS: u32 = 8_192;
const CONTEXT_MARGIN_TOKENS: u32 = 2_048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoticeKind {
    ContextDrift {
        entries: Vec<ContextDriftEntry>,
    },
    AgentDirectory {
        directory: String,
    },
    OpenConversations {
        entries: Vec<OpenConversationEntry>,
    },
    ConversationInbox {
        messages: Vec<ConversationInboxMessage>,
    },
    LoadedSkills {
        skills: Vec<String>,
    },
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenConversationEntry {
    pub(crate) conversation: ConversationAddress,
    pub(crate) summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConversationInboxMessage {
    pub(crate) from: Option<ConversationAddress>,
    pub(crate) text: String,
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

impl ContextBudgetTier {
    #[cfg(test)]
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            ContextBudgetTier::Tight => "tight",
            ContextBudgetTier::Normal => "normal",
            ContextBudgetTier::Roomy => "roomy",
        }
    }
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
    max_output_tokens: Option<u32>,
    estimated_input_tokens: Option<u32>,
) -> ContextHeadroom {
    let reserved_output_tokens = max_output_tokens
        .unwrap_or(CONTEXT_OUTPUT_RESERVE_TOKENS)
        .min(CONTEXT_OUTPUT_RESERVE_TOKENS);
    let budget = max_context_tokens
        .saturating_sub(reserved_output_tokens)
        .saturating_sub(CONTEXT_MARGIN_TOKENS);
    let remaining_input_tokens = estimated_input_tokens.map(|used| budget.saturating_sub(used));
    let tier = match remaining_input_tokens {
        Some(remaining) if budget == 0 || remaining < budget / 3 => ContextBudgetTier::Tight,
        Some(remaining) if remaining < budget - budget.div_ceil(3) => ContextBudgetTier::Normal,
        Some(_) => ContextBudgetTier::Roomy,
        None => ContextBudgetTier::Normal,
    };

    ContextHeadroom {
        max_context_tokens,
        reserved_output_tokens,
        reserved_margin_tokens: CONTEXT_MARGIN_TOKENS,
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
    fn context_budget_tier_as_str_produces_stable_wire_format() {
        assert_eq!(ContextBudgetTier::Tight.as_str(), "tight");
        assert_eq!(ContextBudgetTier::Normal.as_str(), "normal");
        assert_eq!(ContextBudgetTier::Roomy.as_str(), "roomy");
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

    #[test]
    fn headroom_uses_the_smaller_output_cap_on_a_short_context() {
        let headroom = compute_context_headroom(10_000, Some(1_024), Some(128));
        assert_eq!(1_024, headroom.reserved_output_tokens);
        assert_eq!(Some(6_800), headroom.remaining_input_tokens);
    }

    #[test]
    fn headroom_reserves_the_planning_budget_for_large_or_undeclared_output() {
        let default = compute_context_headroom(200_000, None, Some(20_000));
        let large = compute_context_headroom(200_000, Some(393_216), Some(20_000));
        assert_eq!(default, large);
        assert_eq!(Some(169_760), large.remaining_input_tokens);
    }

    #[test]
    fn headroom_maps_an_exhausted_context_with_known_usage_to_tight() {
        let headroom = compute_context_headroom(10_000, None, Some(128));
        assert_eq!(Some(0), headroom.remaining_input_tokens);
        assert_eq!(ContextBudgetTier::Tight, headroom.tier);
    }
}
