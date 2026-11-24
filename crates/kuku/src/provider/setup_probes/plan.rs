use std::sync::Arc;

use crate::CapabilitySource;
use crate::config::{ModelPricing, ProviderConfig};

use super::{ModelTrial, Probe, ReasoningCheck, openrouter_quote_url};
use crate::provider::setup_reasoning::bounded_levels;

/// A selected model with the settings to check at its actual endpoint.
#[derive(Debug, Clone)]
pub struct TrialSelection {
    pub model: String,
    pub effort: Option<String>,
    pub service_tier: Option<String>,
}

/// Plan baseline, media, thinking and pricing tests. Calling this does not dispatch requests.
pub fn plan_model_trials(
    connection: Arc<ProviderConfig>,
    models: impl IntoIterator<Item = TrialSelection>,
) -> Vec<Probe> {
    let mut requests = Vec::new();
    let mut images = Vec::new();
    let mut thinking = Vec::new();
    for TrialSelection {
        model,
        effort,
        service_tier,
    } in models
    {
        if matches!(
            connection.options.model_pricing.get(&model),
            None | Some(ModelPricing::Automatic)
        ) {
            if let Some(url) = openrouter_quote_url(&connection, &model) {
                requests.push(Probe::Quotes {
                    connection: Arc::clone(&connection),
                    model: model.clone(),
                    url,
                    service_tier: service_tier.clone(),
                });
            }
        }
        let capabilities = crate::adapter::inspect_connection_model(&connection, &model).ok();
        let image = capabilities
            .as_ref()
            .and_then(|caps| caps.input_modalities.as_ref())
            .is_none_or(|media| media.value.iter().any(|item| item == "image"));
        requests.push(Probe::Trial {
            connection: Arc::clone(&connection),
            model: model.clone(),
            check: ModelTrial::Text {
                effort: effort.clone(),
                service_tier: service_tier.clone(),
            },
        });
        if image {
            images.push(Probe::Trial {
                connection: Arc::clone(&connection),
                model: model.clone(),
                check: ModelTrial::ImageInput {
                    service_tier: service_tier.clone(),
                },
            });
        }
        let declared = capabilities
            .as_ref()
            .filter(|caps| {
                matches!(
                    caps.reasoning_source,
                    Some(
                        CapabilitySource::ModelList
                            | CapabilitySource::ProtocolModelList
                            | CapabilitySource::Manual
                            | CapabilitySource::ProtocolManual
                    )
                )
            })
            .map(|caps| bounded_levels(caps.reasoning_efforts.iter().map(String::as_str)));
        let mut checks = declared.filter(|levels| !levels.is_empty()).map_or_else(
            || vec![ReasoningCheck::Discover],
            |levels| levels.into_iter().map(ReasoningCheck::Level).collect(),
        );
        if let Some(effort) = effort {
            checks.retain(|check| *check != ReasoningCheck::Level(effort.clone()));
        }
        thinking.push(
            checks
                .into_iter()
                .map(|check| Probe::Thinking {
                    connection: Arc::clone(&connection),
                    model: model.clone(),
                    check,
                    service_tier: service_tier.clone(),
                })
                .collect::<std::collections::VecDeque<_>>(),
        );
    }
    while thinking.iter().any(|checks| !checks.is_empty()) {
        requests.extend(
            thinking
                .iter_mut()
                .filter_map(std::collections::VecDeque::pop_front),
        );
    }
    requests.extend(images);
    requests
}
