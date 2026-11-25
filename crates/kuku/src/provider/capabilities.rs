use crate::config::{
    MediaAdapterSupport, ModelCapabilityDeclaration, ModelCapabilityEntry,
    ProtocolCapabilityDeclaration, ProviderFormat, ProviderOptions, ProviderProfile,
    SubscriptionChannel, ThinkLevel,
};
use crate::error::{Error, Result};
use sha2::{Digest, Sha256};

use super::types::{
    CapabilitySource, DeclaredCapability, ProviderCapabilities, ProviderKind, ProviderSettings,
};

pub(crate) fn resolve_capabilities(
    base_url: &str,
    model: &str,
    kind: ProviderKind,
    options: &ProviderOptions,
) -> ProviderCapabilities {
    let profile = profile_for(base_url, options.profile);
    let (reasoning, services): (&[&str], &[&str]) = match profile {
        ProviderProfile::DeepSeek => (&["none", "low", "high", "max"], &[]),
        ProviderProfile::Anthropic if native_sonnet_55(model) => (
            &["low", "medium", "high", "xhigh", "max"],
            &["auto", "standard_only"],
        ),
        ProviderProfile::Anthropic if adaptive_claude(model) => {
            let reasoning = if model.contains("opus-4-7") || model.contains("opus-4-8") {
                &["none", "low", "medium", "high", "xhigh", "max"][..]
            } else if model.contains("opus") {
                &["none", "low", "medium", "high", "max"][..]
            } else {
                &["none", "low", "medium", "high"][..]
            };
            (reasoning, &["auto", "standard_only"])
        }
        ProviderProfile::OpenRouter if model.starts_with("deepseek/") => {
            (&["none", "low", "medium", "high", "xhigh", "max"], &[])
        }
        ProviderProfile::OpenRouter
            if model.starts_with("anthropic/") && adaptive_claude(model) =>
        {
            (&["none", "low", "medium", "high", "xhigh", "max"], &[])
        }
        ProviderProfile::OpenAi
            if openai_gpt_62(model) || model_version(model, "gpt-6.1-terra") =>
        {
            (
                &["low", "medium", "high", "xhigh", "max"],
                &["auto", "default", "priority"],
            )
        }
        ProviderProfile::OpenAi if model.starts_with("gpt-5") => (
            &["low", "medium", "high"],
            &["auto", "default", "flex", "priority"],
        ),
        ProviderProfile::OpenAi if model.starts_with("o3") || model.starts_with("o4-mini") => (
            &["low", "medium", "high"],
            &["auto", "default", "flex", "priority"],
        ),
        ProviderProfile::OpenAi => (&[], &["auto", "default", "priority"]),
        ProviderProfile::Anthropic => (&["none"], &["auto", "standard_only"]),
        ProviderProfile::Auto | ProviderProfile::Compatible | ProviderProfile::OpenRouter => {
            (&[], &[])
        }
    };
    let subscription_channel = options
        .subscription
        .as_ref()
        .map(|account| account.channel());
    let services = match subscription_channel {
        Some(SubscriptionChannel::ChatGpt) => &["auto", "default", "fast", "ultrafast"][..],
        None => services,
    };
    let declaration = options.model_capabilities.get(model);
    let protocol = kind.discovery_protocol();
    let resolve = ModelDeclarations {
        entry: declaration,
        protocol,
    };
    let reasoning_override = resolve.field(
        |entry| entry.reasoning_efforts.as_ref(),
        |entry| entry.reasoning_efforts.as_ref(),
    );
    let connection_reasoning = || {
        options
            .reasoning_efforts
            .clone()
            .map(|value| DeclaredCapability {
                value,
                source: CapabilitySource::Manual,
            })
    };
    let reasoning_options = match reasoning_override {
        Some(declared)
            if matches!(
                declared.source,
                CapabilitySource::Manual | CapabilitySource::ProtocolManual
            ) =>
        {
            Some(declared)
        }
        declared => connection_reasoning().or(declared),
    };
    let (mut reasoning_efforts, reasoning_source) = if let Some(reasoning) = reasoning_options {
        (reasoning.value, Some(reasoning.source))
    } else if declaration.is_none_or(|entry| entry.discovered.is_none()) && !reasoning.is_empty() {
        (strings(reasoning), Some(CapabilitySource::AdapterProfile))
    } else {
        (Vec::new(), None)
    };
    if profile == ProviderProfile::DeepSeek
        && matches!(
            reasoning_source,
            Some(CapabilitySource::ModelList | CapabilitySource::ProtocolModelList)
        )
        && !reasoning_efforts.iter().any(|effort| effort == "none")
    {
        reasoning_efforts.insert(0, "none".into());
    }
    let thinking_test = declaration
        .and_then(|entry| entry.thinking_test.as_ref())
        .filter(|test| test.scope == probe_scope(base_url, kind.format(), options))
        .cloned();
    if let Some(test) = &thinking_test {
        for level in &test.accepted {
            if !reasoning_efforts.contains(level) {
                reasoning_efforts.push(level.clone());
            }
        }
    }
    if (profile == ProviderProfile::Anthropic && native_sonnet_55(model))
        || (profile == ProviderProfile::OpenAi && openai_gpt_62(model))
    {
        reasoning_efforts.retain(|effort| effort != "none");
    }
    let declared_services = resolve.field(
        |entry| entry.service_tiers.as_ref(),
        |entry| entry.service_tiers.as_ref(),
    );
    let (mut service_tiers, service_tier_source) = match declared_services {
        Some(declared)
            if matches!(
                declared.source,
                CapabilitySource::Manual | CapabilitySource::ProtocolManual
            ) =>
        {
            (declared.value, Some(declared.source))
        }
        _ if options.service_tiers.is_some() => (
            options.service_tiers.clone().unwrap_or_default(),
            Some(CapabilitySource::Manual),
        ),
        Some(declared) => (declared.value, Some(declared.source)),
        None => (
            strings(services),
            (!services.is_empty()).then_some(CapabilitySource::AdapterProfile),
        ),
    };
    let custom_service_tiers = declaration
        .map(|entry| entry.custom_service_tiers.clone())
        .unwrap_or_default();
    for tier in custom_service_tiers.keys() {
        if !service_tiers.contains(tier) {
            service_tiers.push(tier.clone());
        }
    }
    let input_modalities = resolve.field(
        |entry| entry.input_modalities.as_ref(),
        |entry| entry.input_modalities.as_ref(),
    );
    let image_verified = declaration
        .and_then(|entry| entry.image_test_scope.as_ref())
        .is_some_and(|scope| *scope == probe_scope(base_url, kind.format(), options));
    let input_modalities = if image_verified
        && !input_modalities.as_ref().is_some_and(|modalities| {
            matches!(
                modalities.source,
                CapabilitySource::Manual | CapabilitySource::ProtocolManual
            ) || modalities.value.iter().any(|value| value == "image")
        }) {
        let mut value = input_modalities
            .map(|modalities| modalities.value)
            .unwrap_or_else(|| vec!["text".into()]);
        value.push("image".into());
        Some(DeclaredCapability {
            value,
            source: CapabilitySource::ImageProbe,
        })
    } else {
        input_modalities
    };
    let media_adapter = if input_modalities
        .as_ref()
        .is_some_and(|input| input.value.iter().any(|modality| modality == "image"))
    {
        match kind {
            ProviderKind::Anthropic => MediaAdapterSupport::AnthropicMessagesImageBlocks,
            ProviderKind::OpenAiCompatible => MediaAdapterSupport::ChatCompletionsImageParts,
            ProviderKind::OpenAiResponses if profile == ProviderProfile::DeepSeek => {
                MediaAdapterSupport::ResponsesImageToolResults
            }
            ProviderKind::OpenAiResponses
                if image_verified
                    || input_modalities
                        .as_ref()
                        .is_some_and(|input| input.source == CapabilitySource::ProtocolManual) =>
            {
                MediaAdapterSupport::ResponsesImageInput
            }
            _ => MediaAdapterSupport::TextOnly,
        }
    } else {
        MediaAdapterSupport::TextOnly
    };
    ProviderCapabilities {
        profile,
        subscription_channel,
        prompt_cache_control: profile == ProviderProfile::OpenRouter
            && model
                .trim_start_matches('~')
                .starts_with("anthropic/claude-"),
        tool_calls: (profile == ProviderProfile::OpenAi && requires_responses_for_tools(model))
            .then_some(kind == ProviderKind::OpenAiResponses),
        reasoning_efforts,
        thinking_test,
        reasoning_source,
        reasoning_default: resolve.field(
            |entry| entry.reasoning_default.as_ref(),
            |entry| entry.reasoning_default.as_ref(),
        ),
        input_modalities,
        output_modalities: resolve.field(
            |entry| entry.output_modalities.as_ref(),
            |entry| entry.output_modalities.as_ref(),
        ),
        interfaces: resolve.field(|entry| entry.interfaces.as_ref(), |_| None),
        system_prompt_update: resolve.field(|_| None, |entry| entry.system_prompt_update.as_ref()),
        service_tiers,
        service_tier_source,
        custom_service_tiers,
        media_adapter,
    }
}

pub(super) fn probe_scope(
    endpoint: &str,
    format: ProviderFormat,
    options: &ProviderOptions,
) -> String {
    let route = serde_json::to_vec(&(
        endpoint,
        format,
        options.profile,
        &options.routing,
        &options.proxy,
    ))
    .expect("serializable thinking route");
    format!("{:x}", Sha256::digest(route))
}

pub(crate) fn profile_for(endpoint: &str, configured: ProviderProfile) -> ProviderProfile {
    match configured {
        ProviderProfile::Auto => detect_profile(endpoint),
        profile => profile,
    }
}

struct ModelDeclarations<'a> {
    entry: Option<&'a ModelCapabilityEntry>,
    protocol: &'a str,
}

impl ModelDeclarations<'_> {
    fn field<T: Clone>(
        &self,
        model: impl Fn(&ModelCapabilityDeclaration) -> Option<&T>,
        protocol: impl Fn(&ProtocolCapabilityDeclaration) -> Option<&T>,
    ) -> Option<DeclaredCapability<T>> {
        let entry = self.entry?;
        for (declaration, model_source, protocol_source) in [
            (
                &entry.manual,
                CapabilitySource::Manual,
                CapabilitySource::ProtocolManual,
            ),
            (
                &entry.discovered,
                CapabilitySource::ModelList,
                CapabilitySource::ProtocolModelList,
            ),
        ] {
            let Some(declaration) = declaration else {
                continue;
            };
            if let Some(value) = declaration.protocols.get(self.protocol).and_then(&protocol) {
                return Some(DeclaredCapability {
                    value: value.clone(),
                    source: protocol_source,
                });
            }
            if let Some(value) = model(declaration) {
                return Some(DeclaredCapability {
                    value: value.clone(),
                    source: model_source,
                });
            }
        }
        None
    }
}

impl ProviderCapabilities {
    /// Whether two tier names mean the same thing for this adapter.
    pub fn equivalent_service_tiers(&self, left: &str, right: &str) -> bool {
        match self.profile {
            ProviderProfile::OpenAi | ProviderProfile::OpenRouter => {
                let alias = |tier| matches!(tier, "fast" | "priority");
                (alias(left) && alias(right))
                    || (self.profile == ProviderProfile::OpenRouter
                        && normalized_tier(Some(left)) == normalized_tier(Some(right)))
                    || left == right
            }
            _ => left == right,
        }
    }

    /// Whether the existing capability resolver accepts this value for the configured route.
    pub fn supports_service_tier(&self, value: &str) -> bool {
        self.service_tiers
            .iter()
            .any(|tier| self.equivalent_service_tiers(tier, value))
    }
}

pub(crate) fn resolve_settings(
    connection: &str,
    capabilities: &ProviderCapabilities,
    native_reasoning: Option<String>,
    think: Option<ThinkLevel>,
    service: Option<String>,
) -> Result<ProviderSettings> {
    let (requested_reasoning_effort, reasoning_effort) = match native_reasoning {
        Some(effort) if effort == ThinkLevel::ProviderDefault.as_str() => (None, None),
        Some(effort) => {
            validate_choice(
                connection,
                "reasoning_effort",
                Some(&effort),
                &capabilities.reasoning_efforts,
            )?;
            (Some(effort.clone()), Some(effort))
        }
        None => (
            think
                .filter(|level| *level != ThinkLevel::ProviderDefault)
                .map(|level| level.as_str().to_string()),
            think.and_then(|level| portable_reasoning(level, capabilities)),
        ),
    };
    if let Some(tier) = service
        .as_deref()
        .filter(|tier| !capabilities.supports_service_tier(tier))
    {
        validate_choice(
            connection,
            "service_tier",
            Some(tier),
            &capabilities.service_tiers,
        )?;
    }
    let resolved_service = service.as_deref().map(|tier| match capabilities.profile {
        ProviderProfile::OpenRouter => normalized_tier(Some(tier)).to_owned(),
        _ => tier.to_owned(),
    });
    Ok(ProviderSettings {
        requested_reasoning_effort,
        reasoning_effort,
        requested_service_tier: service.clone(),
        service_tier: resolved_service,
    })
}

fn portable_reasoning(level: ThinkLevel, capabilities: &ProviderCapabilities) -> Option<String> {
    let effort = match level {
        ThinkLevel::ProviderDefault => return None,
        ThinkLevel::Off => "none",
        ThinkLevel::Medium if capabilities.profile == ProviderProfile::DeepSeek => "high",
        ThinkLevel::Low | ThinkLevel::Medium | ThinkLevel::High => level.as_str(),
    };
    capabilities
        .reasoning_efforts
        .iter()
        .find(|candidate| *candidate == effort)
        .cloned()
}

fn detect_profile(base_url: &str) -> ProviderProfile {
    match url::Url::parse(base_url)
        .ok()
        .as_ref()
        .and_then(url::Url::host_str)
    {
        Some("api.deepseek.com") => ProviderProfile::DeepSeek,
        Some("openrouter.ai" | "us.openrouter.ai" | "eu.openrouter.ai") => {
            ProviderProfile::OpenRouter
        }
        Some("api.openai.com") => ProviderProfile::OpenAi,
        Some("api.anthropic.com") => ProviderProfile::Anthropic,
        Some(_) | None => ProviderProfile::Compatible,
    }
}

fn adaptive_claude(model: &str) -> bool {
    ["opus-4-6", "opus-4-7", "opus-4-8", "sonnet-4-6"]
        .iter()
        .any(|name| model.contains(name))
}

fn model_version(model: &str, name: &str) -> bool {
    model == name
        || model
            .strip_prefix(name)
            .is_some_and(|suffix| suffix.starts_with('-'))
}

fn native_sonnet_55(model: &str) -> bool {
    model_version(model, "claude-sonnet-5-5") || model_version(model, "claude-sonnet-5.5")
}

fn openai_gpt_62(model: &str) -> bool {
    model_version(model, "gpt-6.2-sol") || model_version(model, "gpt-6.2-astra")
}

/// Whether a known official route requires Responses to call tools.
pub fn setup_requires_responses_for_tools(
    connection: &crate::config::ProviderConfig,
    model: &str,
) -> bool {
    let endpoint = connection
        .options
        .endpoint_url
        .as_deref()
        .unwrap_or(&connection.base_url);
    profile_for(endpoint, connection.options.profile) == ProviderProfile::OpenAi
        && requires_responses_for_tools(model)
}

fn requires_responses_for_tools(model: &str) -> bool {
    openai_gpt_62(model) || model_version(model, "gpt-6.1-terra")
}

fn validate_choice(
    connection: &str,
    setting: &str,
    value: Option<&str>,
    allowed: &[String],
) -> Result<()> {
    if let Some(value) = value {
        if !allowed.iter().any(|candidate| candidate == value) {
            return Err(Error::MissingProviderConfig(format!(
                "connection '{connection}': unsupported {setting} '{value}'; supported values: {}",
                if allowed.is_empty() {
                    "none declared".into()
                } else {
                    allowed.join(", ")
                }
            )));
        }
    }
    Ok(())
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

pub(super) fn normalized_tier(tier: Option<&str>) -> &str {
    match tier {
        None | Some("standard") => "default",
        Some("fast") => "priority",
        Some(tier) => tier,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_cache_control_applies_to_claude_names_and_aliases_only() {
        for kind in [
            ProviderKind::OpenAiCompatible,
            ProviderKind::OpenAiResponses,
            ProviderKind::Anthropic,
        ] {
            for (model, supported) in [
                ("anthropic/claude-haiku-5.5", true),
                ("~anthropic/claude-sonnet-latest", true),
                ("openai/gpt-6-luna", false),
            ] {
                let caps = resolve_capabilities(
                    "https://openrouter.ai/api/v1",
                    model,
                    kind,
                    &ProviderOptions::default(),
                );
                assert_eq!(supported, caps.prompt_cache_control);
            }
        }
    }

    #[test]
    fn official_models_resolve_supported_efforts_and_preserve_provider_default() {
        for (endpoint, model, kind) in [
            (
                "https://api.openai.com/v1",
                "gpt-6.2-sol",
                ProviderKind::OpenAiResponses,
            ),
            (
                "https://api.openai.com/v1",
                "gpt-6.2-astra",
                ProviderKind::OpenAiResponses,
            ),
            (
                "https://api.anthropic.com/v1",
                "claude-sonnet-5-5",
                ProviderKind::Anthropic,
            ),
        ] {
            let caps = resolve_capabilities(endpoint, model, kind, &ProviderOptions::default());
            assert_eq!(
                vec!["low", "medium", "high", "xhigh", "max"],
                caps.selectable_reasoning_efforts().collect::<Vec<_>>()
            );
            let explicit =
                resolve_settings("fixture", &caps, Some("xhigh".into()), None, None).unwrap();
            assert_eq!(Some("xhigh"), explicit.reasoning_effort.as_deref());
            let portable =
                resolve_settings("fixture", &caps, None, Some(ThinkLevel::Low), None).unwrap();
            assert_eq!(Some("low"), portable.reasoning_effort.as_deref());
            let default = resolve_settings("fixture", &caps, None, None, None).unwrap();
            assert_eq!(None, default.reasoning_effort);
            assert!(resolve_settings("fixture", &caps, Some("none".into()), None, None).is_err());
        }
    }
}
