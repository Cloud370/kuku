use crate::config::{
    ModelCapabilityDeclaration, ModelCapabilityEntry, ProtocolCapabilityDeclaration,
    ProviderFormat, ProviderOptions, ProviderProfile, ThinkLevel,
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
        .filter(|test| test.scope == thinking_scope(base_url, kind.format(), options))
        .cloned();
    if let Some(test) = &thinking_test {
        for level in &test.accepted {
            if !reasoning_efforts.contains(level) {
                reasoning_efforts.push(level.clone());
            }
        }
    }
    ProviderCapabilities {
        profile,
        reasoning_efforts,
        thinking_test,
        reasoning_source,
        reasoning_default: resolve.field(
            |entry| entry.reasoning_default.as_ref(),
            |entry| entry.reasoning_default.as_ref(),
        ),
        input_modalities: resolve.field(
            |entry| entry.input_modalities.as_ref(),
            |entry| entry.input_modalities.as_ref(),
        ),
        output_modalities: resolve.field(
            |entry| entry.output_modalities.as_ref(),
            |entry| entry.output_modalities.as_ref(),
        ),
        interfaces: resolve.field(|entry| entry.interfaces.as_ref(), |_| None),
        system_prompt_update: resolve.field(|_| None, |entry| entry.system_prompt_update.as_ref()),
        service_tiers: options
            .service_tiers
            .clone()
            .unwrap_or_else(|| strings(services)),
        ..ProviderCapabilities::default()
    }
}

pub(super) fn thinking_scope(
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
    validate_choice(
        connection,
        "service_tier",
        service.as_deref(),
        &capabilities.service_tiers,
    )?;
    Ok(ProviderSettings {
        requested_reasoning_effort,
        reasoning_effort,
        requested_service_tier: service.clone(),
        service_tier: service,
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
