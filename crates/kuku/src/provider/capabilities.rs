use crate::config::{ProviderOptions, ProviderProfile, ThinkLevel};
use crate::error::{Error, Result};

use super::types::{ProviderCapabilities, ProviderKind, ProviderSettings};

pub(crate) fn resolve_capabilities(
    base_url: &str,
    model: &str,
    _kind: ProviderKind,
    options: &ProviderOptions,
) -> ProviderCapabilities {
    let profile = match options.profile {
        ProviderProfile::Auto => detect_profile(base_url),
        profile => profile,
    };
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
    ProviderCapabilities {
        profile,
        reasoning_efforts: options
            .reasoning_efforts
            .clone()
            .unwrap_or_else(|| strings(reasoning)),
        service_tiers: options
            .service_tiers
            .clone()
            .unwrap_or_else(|| strings(services)),
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
            think.map(|level| level.as_str().to_string()),
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
        Some("openrouter.ai") => ProviderProfile::OpenRouter,
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
