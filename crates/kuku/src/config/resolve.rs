use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;

use crate::error::{Error, Result};

use super::types::{ApiKey, Config, ConfigFile, ProviderConfig, ThinkLevel, TierConfig};

// ── Load ──

/// Load and parse a TOML config file. Returns empty defaults if the file does not exist.
pub fn load_config(path: &Path) -> Result<ConfigFile> {
    if !path.exists() {
        return Ok(ConfigFile {
            default_model: None,
            model: BTreeMap::new(),
            provider: BTreeMap::new(),
            discovery: None,
            handoff: None,
            logs: None,
            plugin: None,
            update: None,
        });
    }
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::ConfigLoad(format!("cannot read config file {path:?}: {error}")))?;
    parse_config_text(&text)
}

/// Parse config TOML text using the SDK's environment-reference expansion semantics.
pub fn parse_config_text(raw: &str) -> Result<ConfigFile> {
    parse_config_text_with_env(raw, |name| std::env::var(name))
}

/// Parse config TOML text using the supplied environment-variable lookup.
pub fn parse_config_text_with_env(
    raw: &str,
    environment: impl Fn(&str) -> std::result::Result<String, std::env::VarError>,
) -> Result<ConfigFile> {
    let mut value = toml::from_str::<toml::Value>(raw)
        .map_err(|error| Error::ConfigLoad(format!("invalid config: {error}")))?;
    resolve_env_refs(&mut value, "", &environment)?;
    value
        .try_into()
        .map_err(|error| Error::ConfigLoad(format!("invalid config: {error}")))
}

fn resolve_env_refs(
    value: &mut toml::Value,
    path: &str,
    environment: &impl Fn(&str) -> std::result::Result<String, std::env::VarError>,
) -> Result<()> {
    match value {
        toml::Value::String(raw) if should_resolve_env_ref(path, raw) => {
            let env_name = raw
                .strip_prefix('$')
                .expect("should_resolve_env_ref checked prefix");
            *raw = environment(env_name).map_err(|_| {
                Error::ConfigLoad(format!(
                    "env var '{env_name}' referenced by {} is not set",
                    display_path(path)
                ))
            })?;
            Ok(())
        }
        toml::Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                let item_path = if path.is_empty() {
                    format!("[{index}]")
                } else {
                    format!("{path}[{index}]")
                };
                resolve_env_refs(item, &item_path, environment)?;
            }
            Ok(())
        }
        toml::Value::Table(table) => {
            for (key, item) in table.iter_mut() {
                let item_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                resolve_env_refs(item, &item_path, environment)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn should_resolve_env_ref(path: &str, value: &str) -> bool {
    value.starts_with('$') && !path.ends_with("api_key")
}

fn display_path(path: &str) -> &str {
    if path.is_empty() {
        "config value"
    } else {
        path
    }
}

// ── Resolve / validate ──

impl ConfigFile {
    /// Validate and resolve the raw config into typed, validated Config.
    pub fn resolve(&self) -> Result<Config> {
        self.validate_all()?;

        let mut tiers = BTreeMap::new();
        for (name, entry) in &self.model {
            let think = match entry.think.as_deref() {
                None | Some("medium") => ThinkLevel::Medium,
                Some("provider-default") => ThinkLevel::ProviderDefault,
                Some("off") => ThinkLevel::Off,
                Some("low") => ThinkLevel::Low,
                Some("high") => ThinkLevel::High,
                _ => unreachable!("validate_tier_entries checked"),
            };
            tiers.insert(
                name.clone(),
                TierConfig {
                    provider: entry.provider.trim().to_string(),
                    model: entry.model.trim().to_string(),
                    think,
                    reasoning_effort: entry.reasoning_effort.clone(),
                    service_tier: entry.service_tier.clone(),
                    context_window: entry.context_window.unwrap_or(200000),
                    max_output_tokens: entry.max_output_tokens,
                    purpose: entry.purpose.clone().unwrap_or_else(|| name.clone()),
                    output: entry.output.unwrap_or_default(),
                },
            );
        }

        let mut providers = BTreeMap::new();
        for (name, entry) in &self.provider {
            let api_key_raw = entry.api_key.trim();
            let api_key = if let Some(file_path) = api_key_raw.strip_prefix("file:") {
                let path = std::path::PathBuf::from(file_path);
                if !path.is_absolute() {
                    return Err(Error::ConfigLoad(format!(
                        "provider '{name}': file api_key must use an absolute path"
                    )));
                }
                ApiKey::File(path)
            } else if let Some(env_name) = api_key_raw.strip_prefix('$') {
                ApiKey::Env(env_name.to_string())
            } else {
                ApiKey::Plaintext(api_key_raw.to_string())
            };
            providers.insert(
                name.clone(),
                ProviderConfig {
                    format: entry.format,
                    base_url: entry.base_url.trim().to_string(),
                    api_key,
                    options: entry.options.clone(),
                },
            );
        }

        let default_tier = self.default_route().to_owned();

        let discovery = self.discovery.clone().unwrap_or_default();

        let handoff = self.handoff.clone().unwrap_or_default();
        if !(0.0..=1.0).contains(&handoff.threshold) {
            return Err(Error::ConfigLoad(
                "handoff.threshold must be between 0.0 and 1.0".to_string(),
            ));
        }
        if handoff.keep_turns == 0 {
            return Err(Error::ConfigLoad(
                "handoff.keep_turns must be >= 1".to_string(),
            ));
        }

        let logs = self.logs.clone().unwrap_or_default();
        if logs.max_age_days == 0 {
            return Err(Error::ConfigLoad(
                "logs.max_age_days must be >= 1".to_string(),
            ));
        }
        if logs.max_total_size_mb == 0 {
            return Err(Error::ConfigLoad(
                "logs.max_total_size_mb must be >= 1".to_string(),
            ));
        }

        let plugin = self.plugin.clone().unwrap_or_default();

        let update = self.update.clone().unwrap_or_default();

        Ok(Config {
            tiers,
            providers,
            default_tier,
            discovery,
            handoff,
            logs,
            plugin,
            update,
        })
    }

    pub(crate) fn validate_all(&self) -> Result<()> {
        self.validate_tier_entries()?;
        self.validate_provider_entries()?;
        self.validate_cross_references()
    }

    fn default_route(&self) -> &str {
        self.default_model.as_deref().unwrap_or_else(|| {
            if self.model.len() == 1 {
                self.model.keys().next().expect("one configured model")
            } else {
                "balanced"
            }
        })
    }

    pub(crate) fn validate_tier_entries(&self) -> Result<()> {
        for (name, entry) in &self.model {
            if entry.provider.trim().is_empty() {
                return Err(Error::ConfigLoad(format!(
                    "tier '{name}': provider is required"
                )));
            }
            if entry.model.trim().is_empty() {
                return Err(Error::ConfigLoad(format!(
                    "tier '{name}': model is required"
                )));
            }
            if let Some(ref think) = entry.think {
                if ThinkLevel::from_str(think).is_err() {
                    return Err(Error::ConfigLoad(format!(
                        "tier '{name}': think '{think}' is invalid, must be provider-default/off/low/medium/high"
                    )));
                }
            }
            if let Some(cw) = entry.context_window {
                if cw == 0 {
                    return Err(Error::ConfigLoad(format!(
                        "tier '{name}': context_window must be a positive integer"
                    )));
                }
            }
            if let Some(mot) = entry.max_output_tokens {
                if mot == 0 {
                    return Err(Error::ConfigLoad(format!(
                        "tier '{name}': max_output_tokens must be a positive integer"
                    )));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn validate_provider_entries(&self) -> Result<()> {
        for (name, entry) in &self.provider {
            if entry.base_url.trim().is_empty() && entry.options.endpoint_url.is_none() {
                return Err(Error::ConfigLoad(format!(
                    "provider '{name}': base_url or endpoint_url is required"
                )));
            }
            super::resolve_request_url(
                entry.format,
                &entry.base_url,
                entry.options.endpoint_url.as_deref(),
            )
            .map_err(|error| Error::ConfigLoad(format!("provider '{name}': {error}")))?;
            if entry.options.models_url.is_some() {
                super::resolve_models_url(entry.format, &entry.base_url, &entry.options)
                    .map_err(|error| Error::ConfigLoad(format!("provider '{name}': {error}")))?;
            }
            entry
                .options
                .proxy
                .validate()
                .map_err(|error| Error::ConfigLoad(format!("provider '{name}': {error}")))?;
            for (model, pricing) in &entry.options.model_pricing {
                if model.trim().is_empty()
                    || matches!(pricing, super::ModelPricing::Configured { rates } if !rates.valid())
                {
                    return Err(Error::ConfigLoad(format!(
                        "provider '{name}': invalid USD pricing for model '{model}'"
                    )));
                }
            }
            let api_key = entry.api_key.trim();
            if api_key.is_empty() {
                return Err(Error::ConfigLoad(format!(
                    "provider '{name}': api_key is required"
                )));
            }
            if let Some(env_name) = api_key.strip_prefix('$') {
                if env_name.is_empty() {
                    return Err(Error::ConfigLoad(format!(
                        "provider '{name}': api_key '$' prefix must be followed by an env var name"
                    )));
                }
            }
            if let Some(file_path) = api_key.strip_prefix("file:") {
                if !Path::new(file_path).is_absolute() {
                    return Err(Error::ConfigLoad(format!(
                        "provider '{name}': file api_key must use an absolute path"
                    )));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn validate_cross_references(&self) -> Result<()> {
        for (name, tier) in &self.model {
            if !self.provider.contains_key(tier.provider.trim()) {
                return Err(Error::ConfigLoad(format!(
                    "tier '{name}': provider '{}' is not defined in [provider]",
                    tier.provider
                )));
            }
        }
        let default_tier = self.default_route();
        if !self.model.contains_key(default_tier) {
            return Err(Error::ConfigLoad(format!(
                "default_model '{default_tier}' is not defined in [model]"
            )));
        }
        Ok(())
    }
}

// ── ApiKey ──

impl ApiKey {
    /// Resolve the actual key value. Returns error if an env var ref points to a missing variable.
    pub fn resolve(&self) -> Result<String> {
        match self {
            ApiKey::Env(name) => std::env::var(name).map_err(|_| {
                Error::ConfigLoad(format!("env var '{name}' referenced by api_key is not set"))
            }),
            ApiKey::File(path) => crate::util::read_private_key(path).map_err(|_| {
                Error::ConfigLoad("stored credential file is unavailable or unsafe".into())
            }),
            ApiKey::Plaintext(key) => Ok(key.clone()),
        }
    }
}

/// Load a config file, validate it, and return a redacted display string.
///
/// Env-var references (`$FOO`) are shown as-is. Plaintext keys are masked.
pub fn show_redacted(path: &Path) -> Result<String> {
    let config_file = load_config(path)?;
    let config = config_file.resolve()?;
    Ok(config.redacted_display())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::parse_config_text_with_env;

    #[test]
    fn parse_config_text_uses_the_supplied_environment_lookup() {
        let raw = r#"
default_model = "balanced"
model = { balanced = { provider = "anthropic", model = "fixture-model" } }
provider = { anthropic = { format = "anthropic", base_url = "https://api.anthropic.com", api_key = "fixture-key" } }
[discovery]
extra_user_paths = ["$KUKU_HOME"]
"#;
        let parsed = parse_config_text_with_env(raw, |name| {
            assert_eq!("KUKU_HOME", name);
            Ok("service-data".into())
        })
        .unwrap();

        assert_eq!(
            vec![PathBuf::from("service-data")],
            parsed.discovery.unwrap().extra_user_paths
        );
    }
}
