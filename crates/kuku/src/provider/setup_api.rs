use url::Url;

use crate::Error;
use crate::config::{
    ModelCapabilityDeclaration, ProviderConfig, ProviderFormat, resolve_request_url,
};

use super::trial::{TrialProblem, trial_with_route_evidence};
use super::{ModelTrial, TrialResult};

pub(super) const DECLARED_ORDER: [ProviderFormat; 3] = [
    ProviderFormat::OpenAiResponses,
    ProviderFormat::OpenAiChat,
    ProviderFormat::Anthropic,
];

/// Identify the supported protocol used by an explicit standard request URL.
pub fn setup_endpoint_format(address: &str) -> Option<ProviderFormat> {
    let url = Url::parse(address.trim()).ok()?;
    if !matches!(url.scheme(), "https" | "http") {
        return None;
    }
    format_for_interface(url.path())
}

fn format_for_interface(value: &str) -> Option<ProviderFormat> {
    let value = value.trim_end_matches('/');
    match value {
        "openai" | "openai-chat" | "openai_chat" | "chat/completions" => {
            Some(ProviderFormat::OpenAiChat)
        }
        "openai-response" | "openai-responses" | "openai_responses" | "responses" => {
            Some(ProviderFormat::OpenAiResponses)
        }
        "anthropic" | "anthropic_messages" | "messages" => Some(ProviderFormat::Anthropic),
        _ => {
            let path = Url::parse(value)
                .ok()
                .map(|url| url.path().trim_end_matches('/').to_owned());
            DECLARED_ORDER.into_iter().find(|format| {
                path.as_deref()
                    .unwrap_or(value)
                    .ends_with(format.request_suffix())
            })
        }
    }
}

/// Find protocols shared by the selected models, using the catalog's supported interfaces.
pub fn setup_api_candidates<'a>(
    preferred: ProviderFormat,
    models: impl IntoIterator<Item = &'a ModelCapabilityDeclaration>,
) -> Vec<ProviderFormat> {
    let mut candidates = DECLARED_ORDER.to_vec();
    let mut advertised = false;
    for model in models {
        let declared = model
            .interfaces
            .iter()
            .flatten()
            .chain(model.protocols.keys())
            .filter_map(|interface| format_for_interface(interface))
            .collect::<Vec<_>>();
        if !declared.is_empty() {
            advertised = true;
            candidates.retain(|format| declared.contains(format));
        }
    }
    if !advertised {
        candidates.retain(|format| *format != preferred);
        candidates.insert(0, preferred);
    }
    candidates
}

/// Evidence from testing protocols with a single selected model and connection.
pub struct SetupApiEvidence {
    pub format: ProviderFormat,
    pub base_url: String,
    pub trial: TrialResult,
    pub attempted: Vec<ProviderFormat>,
}

pub(super) async fn check_setup_api(
    connection: &ProviderConfig,
    model: &str,
    formats: &[ProviderFormat],
) -> Result<SetupApiEvidence, TrialProblem> {
    let mut attempted = Vec::new();
    let mut attempted_routes = Vec::new();
    let mut candidate = connection.clone();
    let mut bases = vec![connection.base_url.clone()];
    if connection.options.endpoint_url.is_none()
        && Url::parse(&connection.base_url)
            .is_ok_and(|url| url.path() == "/" && url.query().is_none())
    {
        let bare = connection.base_url.trim_end_matches('/');
        bases.extend([format!("{bare}/v1"), format!("{bare}/api/v1")]);
    }
    for base in bases {
        candidate.base_url = base;
        for format in formats {
            candidate.format = *format;
            let url = resolve_request_url(
                *format,
                &candidate.base_url,
                candidate.options.endpoint_url.as_deref(),
            )?;
            let route = (*format, url);
            if attempted_routes.contains(&route) {
                continue;
            }
            attempted_routes.push(route);
            attempted.push(*format);
            match trial_with_route_evidence(
                &candidate,
                model,
                ModelTrial::Text {
                    effort: None,
                    service_tier: None,
                },
            )
            .await
            {
                Ok(trial)
                    if trial.text_returned
                        || trial.observed_reasoning
                        || trial.stop_reason.is_some() =>
                {
                    return Ok(SetupApiEvidence {
                        format: *format,
                        base_url: candidate.base_url,
                        trial,
                        attempted,
                    });
                }
                Ok(_) => {
                    return Err(Error::InvalidArgument(
                        "The API response contained no test result.".into(),
                    )
                    .into());
                }
                Err(problem) if problem.route_missing => {}
                Err(problem) => return Err(problem),
            }
        }
    }
    Err(TrialProblem {
        route_missing: true,
        ..Error::InvalidArgument("No API type answered for this model.".into()).into()
    })
}
