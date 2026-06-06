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
    parse_config_file(&text)
}

pub(crate) fn parse_config_file(raw: &str) -> Result<ConfigFile> {
    let mut value = toml::from_str::<toml::Value>(raw)
        .map_err(|error| Error::ConfigLoad(format!("invalid config: {error}")))?;
    resolve_env_refs(&mut value, "")?;
    value
        .try_into()
        .map_err(|error| Error::ConfigLoad(format!("invalid config: {error}")))
}

fn resolve_env_refs(value: &mut toml::Value, path: &str) -> Result<()> {
    match value {
        toml::Value::String(raw) if should_resolve_env_ref(path, raw) => {
            let env_name = raw
                .strip_prefix('$')
                .expect("should_resolve_env_ref checked prefix");
            *raw = std::env::var(env_name).map_err(|_| {
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
                resolve_env_refs(item, &item_path)?;
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
                resolve_env_refs(item, &item_path)?;
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
                    context_window: entry.context_window.unwrap_or(200000),
                    max_output_tokens: entry.max_output_tokens.unwrap_or(48000),
                    purpose: entry.purpose.clone().unwrap_or_else(|| name.clone()),
                },
            );
        }

        let mut providers = BTreeMap::new();
        for (name, entry) in &self.provider {
            let api_key_raw = entry.api_key.trim();
            let api_key = if let Some(env_name) = api_key_raw.strip_prefix('$') {
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
                },
            );
        }

        let default_tier = self
            .default_model
            .clone()
            .unwrap_or_else(|| "balanced".to_string());

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
        self.validate_required_tiers()?;
        self.validate_tier_entries()?;
        self.validate_provider_entries()?;
        self.validate_cross_references()
    }

    pub(crate) fn validate_required_tiers(&self) -> Result<()> {
        const REQUIRED_TIERS: &[&str] = &["strong", "balanced", "light"];
        for &name in REQUIRED_TIERS {
            if !self.model.contains_key(name) {
                return Err(Error::ConfigLoad(format!(
                    "required tier '{name}' is missing"
                )));
            }
        }
        Ok(())
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
                        "tier '{name}': think '{think}' is invalid, must be off/low/medium/high"
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
            if entry.base_url.trim().is_empty() {
                return Err(Error::ConfigLoad(format!(
                    "provider '{name}': base_url is required"
                )));
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
        let default_tier = self.default_model.as_deref().unwrap_or("balanced");
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
