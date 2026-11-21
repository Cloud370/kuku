use ratatui::style::Style;
use ratatui::text::Line;
use serde::Deserialize;

use crate::text::{effort_label, service_tier_label, shorten, single_line, with_suffix};
use crate::theme::MUTED;

#[derive(Debug, Clone, Deserialize)]
pub(in crate::transcript) struct AgentModel {
    preset: Option<String>,
    reasoning_effort: Option<String>,
    service_tier: Option<String>,
}

impl super::AgentCard<'_> {
    pub(super) fn model_label(&self, width: usize) -> Option<Line<'static>> {
        let label = match (self.model, self.preset) {
            (Some(model), _) => match &model.preset {
                Some(preset) => format!("Preset · {}", single_line(preset)),
                None => format!(
                    "Thinking · {}",
                    effort_label(model.reasoning_effort.as_deref())
                ),
            },
            (None, Some(preset)) => format!("Preset requested · {}", single_line(preset)),
            (None, None) => return None,
        };
        let label = if let Some(tier) = self
            .model
            .and_then(|model| model.service_tier.as_deref())
            .filter(|tier| !matches!(*tier, "auto" | "default" | "standard"))
        {
            with_suffix(&label, &service_tier_label(Some(tier)), width)
        } else {
            shorten(&label, width)
        };
        Some(Line::styled(label, Style::new().fg(MUTED)))
    }
}

#[cfg(test)]
mod tests {
    use kuku_client::Receipt;
    use serde_json::json;

    use super::AgentModel;
    use crate::transcript::agent::AgentRequest;

    #[test]
    fn cards_show_preset_or_thinking_and_only_specific_service_tiers() {
        let call = AgentRequest::from_call("create_agent", &json!({"address":"reviewer"})).unwrap();
        let receipt = Receipt {
            command_id: "create".into(),
            event_id: 1,
            agent: "reviewer".into(),
        };
        for (value, expected) in [
            (json!({}), "Thinking · Default"),
            (
                json!({"reasoning_effort":"high","service_tier":"default"}),
                "Thinking · High",
            ),
            (
                json!({"reasoning_effort":"high","service_tier":"priority"}),
                "Thinking · High · Fast",
            ),
            (
                json!({"preset":"review","reasoning_effort":"high","service_tier":"flex"}),
                "Preset · review · Flex",
            ),
        ] {
            let model: AgentModel = serde_json::from_value(value).unwrap();
            let card = call.card(Some(&receipt), Some(&model), "ok", "");
            assert_eq!(card.model_label(40).unwrap().to_string(), expected);
        }
    }
}
