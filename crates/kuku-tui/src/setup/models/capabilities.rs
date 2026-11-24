use crossterm::event::{KeyCode, KeyEvent};
use kuku::ProviderCapabilities;
use kuku::config::{ModelConfig, ModelRef, ProviderConfig, ThinkLevel};

use crate::setup::service_tier::valid_tier;

use super::SetupState;
use crate::setup::ConnectionChoice;
use crate::setup::config::Settings;
use crate::setup::state::{SetupStep, edit_field};

pub(in crate::setup) const CUSTOM_TIERS_ROW: usize = 5;

pub(in crate::setup) fn same_service_tier(left: Option<&str>, right: Option<&str>) -> bool {
    left == right
        || matches!(
            (left, right),
            (Some("fast"), Some("priority")) | (Some("priority"), Some("fast"))
        )
}

pub(in crate::setup) fn automatic_reasoning(capabilities: &ProviderCapabilities) -> Option<&str> {
    const ORDER: [&str; 8] = [
        "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
    ];
    let efforts: std::collections::BTreeSet<_> = capabilities
        .selectable_reasoning_efforts()
        .filter(|effort| *effort != ThinkLevel::ProviderDefault.as_str())
        .collect();
    if efforts.contains("medium") {
        return Some("medium");
    }
    if efforts.len() == 1 {
        return efforts.first().copied();
    }
    let ordered: Vec<_> = ORDER
        .into_iter()
        .filter(|effort| efforts.contains(effort))
        .collect();
    if !ordered.is_empty() && ordered.len() == efforts.len() {
        return Some(ordered[(ordered.len() - 1) / 2]);
    }
    capabilities
        .reasoning_default
        .as_ref()
        .map(|declared| declared.value.as_str())
        .filter(|effort| efforts.contains(effort))
}

pub(in crate::setup) fn has_configured_reasoning(route: &ModelConfig) -> bool {
    route.reasoning_effort.is_some() || route.think != ThinkLevel::ProviderDefault
}

impl SetupState {
    pub(in crate::setup) fn displayed_reasoning(&self, settings: &Settings) -> Option<String> {
        if self.reasoning_changed {
            self.reasoning.clone()
        } else if let ConnectionChoice::Existing(name) = self.choice()
            && let Some(model) = self.primary_model()
            && let Some(settings) = settings.model(&ModelRef::new(name, model))
            && has_configured_reasoning(settings)
        {
            settings
                .reasoning_effort
                .clone()
                .or_else(|| Some(settings.think.as_str().into()))
        } else {
            automatic_reasoning(&self.capabilities(settings)?).map(str::to_owned)
        }
    }

    pub(in crate::setup) fn open_thinking(&mut self, settings: &Settings) {
        self.models.cursor = self
            .displayed_reasoning(settings)
            .and_then(|current| {
                self.thinking_choices(settings)
                    .iter()
                    .position(|choice| choice == &current)
            })
            .unwrap_or(0);
        self.step = SetupStep::Thinking;
    }

    pub(in crate::setup) fn service_override(&self, model: &str) -> Option<Option<&str>> {
        let (selected_model, tier) = self.selected_service.as_ref()?;
        (selected_model == model).then_some(tier.as_deref())
    }

    pub(in crate::setup) fn displayed_service<'a>(
        &'a self,
        settings: &'a Settings,
    ) -> Option<&'a str> {
        let model = self.primary_model()?;
        if let Some(tier) = self.service_override(&model) {
            return tier;
        }
        let ConnectionChoice::Existing(name) = self.choice() else {
            return None;
        };
        settings
            .model(&ModelRef::new(name, model))?
            .service_tier
            .as_deref()
    }

    pub(in crate::setup) fn service_choices(&self, settings: &Settings) -> Vec<Option<String>> {
        let mut services = self
            .capabilities(settings)
            .map_or_else(Vec::new, |caps| caps.service_tiers);
        if services.iter().any(|value| value == "fast") {
            services.retain(|value| value != "priority");
        }
        std::iter::once(None)
            .chain(services.into_iter().map(Some))
            .collect()
    }

    pub(in crate::setup) fn open_service(&mut self, settings: &Settings) {
        let service = self.displayed_service(settings);
        self.models.cursor = self
            .service_choices(settings)
            .iter()
            .position(|choice| same_service_tier(choice.as_deref(), service))
            .unwrap_or(0);
        self.step = SetupStep::Service;
    }

    pub(in crate::setup) fn choose_service(&mut self, key: KeyEvent, settings: &Settings) {
        let choices = self.service_choices(settings);
        if matches!(key.code, KeyCode::Enter | KeyCode::Tab) {
            if let (Some(model), Some(tier)) =
                (self.primary_model(), choices.get(self.models.cursor))
            {
                self.selected_service = Some((model, tier.clone()));
                self.review_models(settings);
            }
        } else {
            self.models.move_cursor(key.code, choices.len());
        }
    }

    pub(in crate::setup) fn update_capabilities(&self, connection: &mut ProviderConfig) {
        self.update_model_declarations(connection);
        for model in self.model_ids().into_iter().flatten() {
            if let Some(tiers) = self.trials.advertised_service_tiers(connection, &model) {
                connection
                    .options
                    .model_capabilities
                    .entry(model.clone())
                    .or_default()
                    .discovered
                    .get_or_insert_with(Default::default)
                    .service_tiers = Some(tiers);
            }
            let Some(test) = self.trials.thinking_test(connection, &model) else {
                continue;
            };
            let entry = connection
                .options
                .model_capabilities
                .entry(model)
                .or_default();
            if let Some(previous) = &mut entry.thinking_test {
                previous.merge(test);
            } else {
                entry.thinking_test = Some(test);
            }
        }
    }

    pub(super) fn update_model_declarations(&self, connection: &mut ProviderConfig) {
        if self
            .models
            .discovered_from
            .as_ref()
            .is_some_and(|original| {
                original.format == connection.format
                    && original.base_url == connection.base_url
                    && original.api_key == connection.api_key
                    && original.options.subscription == connection.options.subscription
                    && original.options.endpoint_url == connection.options.endpoint_url
                    && original.options.models_url == connection.options.models_url
                    && original.options.proxy == connection.options.proxy
            })
        {
            for model in &self.models.found {
                if !self.models.selected.contains(&model.id)
                    && !connection
                        .options
                        .model_capabilities
                        .contains_key(&model.id)
                    && self.model.text() != model.id
                {
                    continue;
                }
                let entry = connection
                    .options
                    .model_capabilities
                    .entry(model.id.clone())
                    .or_default();
                let mut declaration = model.capabilities.clone();
                if declaration.service_tiers.is_none() {
                    declaration.service_tiers = entry
                        .discovered
                        .as_ref()
                        .and_then(|previous| previous.service_tiers.clone());
                }
                entry.discovered = Some(declaration);
            }
        }
        for (id, tiers) in &self.models.custom_services {
            connection
                .options
                .model_capabilities
                .entry(id.clone())
                .or_default()
                .custom_service_tiers = tiers.clone();
        }
        for (id, manual) in &self.models.overrides {
            connection
                .options
                .model_capabilities
                .entry(id.clone())
                .or_default()
                .manual = Some(manual.clone());
        }
    }
    pub(in crate::setup) fn choose_advanced(&mut self, key: KeyEvent, settings: &Settings) {
        if matches!(key.code, KeyCode::Enter | KeyCode::Tab) {
            let Some(id) = self.primary_model() else {
                return;
            };
            let saved = self
                .connection_options(settings)
                .model_capabilities
                .remove(&id);
            if self.models.cursor == CUSTOM_TIERS_ROW {
                let tiers = self.models.custom_services.entry(id).or_insert_with(|| {
                    saved
                        .map(|entry| entry.custom_service_tiers)
                        .unwrap_or_default()
                });
                self.models
                    .override_value
                    .set_text(tiers.keys().cloned().collect::<Vec<_>>().join(", "));
                self.step = SetupStep::Override;
                return;
            }
            let saved = saved.and_then(|entry| entry.manual);
            let declaration = self
                .models
                .overrides
                .entry(id)
                .or_insert_with(|| saved.unwrap_or_default());
            let value = match self.models.cursor {
                0 => declaration
                    .reasoning_efforts
                    .as_ref()
                    .map(|items| items.join(", ")),
                1 => declaration.reasoning_default.clone(),
                2 => declaration
                    .input_modalities
                    .as_ref()
                    .map(|items| items.join(", ")),
                3 => declaration
                    .output_modalities
                    .as_ref()
                    .map(|items| items.join(", ")),
                _ => declaration
                    .interfaces
                    .as_ref()
                    .map(|items| items.join(", ")),
            };
            self.models
                .override_value
                .set_text(value.unwrap_or_default());
            self.step = SetupStep::Override;
        } else {
            self.models.move_cursor(key.code, CUSTOM_TIERS_ROW + 1);
        }
    }

    pub(in crate::setup) fn edit_override(&mut self, key: KeyEvent) {
        if matches!(key.code, KeyCode::Enter | KeyCode::Tab) {
            let Some(id) = self.primary_model() else {
                return;
            };
            let text = self.models.override_value.text().trim();
            let list = (!text.is_empty()).then(|| {
                text.split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_owned)
                    .collect()
            });
            if self.models.cursor == CUSTOM_TIERS_ROW {
                let values: Vec<String> = list.unwrap_or_default();
                if !values.iter().all(|tier| valid_tier(tier)) {
                    self.problem = Some("Enter tier names separated by commas.".into());
                    return;
                }
                let tiers = self.models.custom_services.entry(id).or_default();
                *tiers = values
                    .into_iter()
                    .map(|tier| {
                        let evidence = tiers.get(&tier).copied().unwrap_or_default();
                        (tier, evidence)
                    })
                    .collect();
                self.step = SetupStep::Advanced;
                self.problem = None;
                return;
            }
            let declaration = self.models.overrides.entry(id).or_default();
            match self.models.cursor {
                0 => declaration.reasoning_efforts = list,
                1 => declaration.reasoning_default = (!text.is_empty()).then(|| text.to_owned()),
                2 => declaration.input_modalities = list,
                3 => declaration.output_modalities = list,
                _ => declaration.interfaces = list,
            }
            self.step = SetupStep::Advanced;
        } else {
            edit_field(&mut self.models.override_value, key);
        }
    }
}

#[cfg(test)]
mod tests {
    use kuku::{CapabilitySource, DeclaredCapability, ProviderCapabilities};

    use super::automatic_reasoning;

    #[test]
    fn reasoning_default_prefers_medium_then_a_known_middle_value() {
        let default = |choices: &[&str], declared: Option<&str>| {
            let caps = ProviderCapabilities {
                reasoning_efforts: choices.iter().map(|choice| (*choice).into()).collect(),
                reasoning_default: declared.map(|value| DeclaredCapability {
                    value: value.into(),
                    source: CapabilitySource::ModelList,
                }),
                ..Default::default()
            };
            automatic_reasoning(&caps).map(str::to_owned)
        };
        assert_eq!(
            default(&["low", "medium", "high", "xhigh", "ultra"], Some("low")).as_deref(),
            Some("medium")
        );
        assert_eq!(
            default(&["high", "ultra", "xhigh", "low"], None).as_deref(),
            Some("high")
        );
        assert_eq!(default(&["native"], None).as_deref(), Some("native"));
        assert_eq!(default(&["a", "b"], Some("b")).as_deref(), Some("b"));
        assert_eq!(default(&["a", "b"], None), None);
        assert_eq!(default(&[], Some("low")), None);
    }
}
