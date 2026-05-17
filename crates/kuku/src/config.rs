//! Config file types, loading, and typed access for `~/.kuku/config.toml`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

// ── Raw config file ──

/// Raw deserialized contents of `config.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigFile {
    pub default_model: Option<String>,

    #[serde(default)]
    pub model: BTreeMap<String, ModelEntry>,

    #[serde(default)]
    pub provider: BTreeMap<String, ProviderEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub think: Option<String>,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub purpose: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderEntry {
    pub format: String,
    pub base_url: String,
    pub api_key: String,
}

// ── Validated / resolved config ──

/// Validated configuration for model tiers and providers.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub tiers: BTreeMap<String, TierConfig>,
    pub providers: BTreeMap<String, ProviderConfig>,
    pub default_tier: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TierConfig {
    pub provider: String,
    pub model: String,
    pub think: ThinkLevel,
    pub context_window: u32,
    pub max_output_tokens: u32,
    pub purpose: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderConfig {
    pub format: String,
    pub base_url: String,
    pub api_key: ApiKey,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ApiKey {
    Env(String),
    Plaintext(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThinkLevel {
    #[serde(rename = "off")]
    Off,
    #[serde(rename = "low")]
    Low,
    #[serde(rename = "medium")]
    Medium,
    #[serde(rename = "high")]
    High,
}

impl ThinkLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            ThinkLevel::Off => "off",
            ThinkLevel::Low => "low",
            ThinkLevel::Medium => "medium",
            ThinkLevel::High => "high",
        }
    }
}

/// Lightweight tier info for prompt injection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierInfo {
    pub name: String,
    pub purpose: String,
}

// ── Load ──

/// Load and parse a TOML config file. Returns empty defaults if the file does not exist.
pub fn load_config(path: &Path) -> Result<ConfigFile> {
    if !path.exists() {
        return Ok(ConfigFile {
            default_model: None,
            model: BTreeMap::new(),
            provider: BTreeMap::new(),
        });
    }
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::ConfigLoad(format!("cannot read config file {path:?}: {error}")))?;
    toml::from_str(&text).map_err(|error| Error::ConfigLoad(format!("invalid config: {error}")))
}

// ── Resolve / validate ──

impl ConfigFile {
    /// Validate and resolve the raw config into typed, validated Config.
    pub fn resolve(&self) -> Result<Config> {
        const REQUIRED_TIERS: &[&str] = &["strong", "balanced", "light"];
        for &name in REQUIRED_TIERS {
            if !self.model.contains_key(name) {
                return Err(Error::ConfigLoad(format!(
                    "required tier '{name}' is missing"
                )));
            }
        }

        let mut tiers = BTreeMap::new();
        for (name, entry) in &self.model {
            let provider = entry.provider.trim().to_string();
            if provider.is_empty() {
                return Err(Error::ConfigLoad(format!(
                    "tier '{name}': provider is required"
                )));
            }
            let model = entry.model.trim().to_string();
            if model.is_empty() {
                return Err(Error::ConfigLoad(format!(
                    "tier '{name}': model is required"
                )));
            }
            let think = match entry.think.as_deref() {
                None | Some("medium") => ThinkLevel::Medium,
                Some("off") => ThinkLevel::Off,
                Some("low") => ThinkLevel::Low,
                Some("high") => ThinkLevel::High,
                Some(other) => {
                    return Err(Error::ConfigLoad(format!(
                        "tier '{name}': think '{other}' is invalid, must be off/low/medium/high"
                    )));
                }
            };
            let context_window = entry.context_window.unwrap_or(200000);
            if context_window == 0 {
                return Err(Error::ConfigLoad(format!(
                    "tier '{name}': context_window must be a positive integer"
                )));
            }
            let max_output_tokens = entry.max_output_tokens.unwrap_or(48000);
            if max_output_tokens == 0 {
                return Err(Error::ConfigLoad(format!(
                    "tier '{name}': max_output_tokens must be a positive integer"
                )));
            }
            let purpose = entry.purpose.clone().unwrap_or_else(|| name.clone());

            tiers.insert(
                name.clone(),
                TierConfig {
                    provider,
                    model,
                    think,
                    context_window,
                    max_output_tokens,
                    purpose,
                },
            );
        }

        let mut providers = BTreeMap::new();
        for (name, entry) in &self.provider {
            let format = entry.format.trim().to_string();
            if format.is_empty() {
                return Err(Error::ConfigLoad(format!(
                    "provider '{name}': format is required"
                )));
            }
            if !matches!(format.as_str(), "anthropic" | "openai-chat" | "openai-responses") {
                return Err(Error::ConfigLoad(format!(
                    "provider '{name}': format '{format}' is not supported, must be anthropic/openai-chat/openai-responses"
                )));
            }
            let base_url = entry.base_url.trim().to_string();
            if base_url.is_empty() {
                return Err(Error::ConfigLoad(format!(
                    "provider '{name}': base_url is required"
                )));
            }
            let api_key_raw = entry.api_key.trim().to_string();
            if api_key_raw.is_empty() {
                return Err(Error::ConfigLoad(format!(
                    "provider '{name}': api_key is required"
                )));
            }
            let api_key = if let Some(env_name) = api_key_raw.strip_prefix('$') {
                if env_name.is_empty() {
                    return Err(Error::ConfigLoad(format!(
                        "provider '{name}': api_key '$' prefix must be followed by an env var name"
                    )));
                }
                ApiKey::Env(env_name.to_string())
            } else {
                ApiKey::Plaintext(api_key_raw)
            };

            providers.insert(
                name.clone(),
                ProviderConfig {
                    format,
                    base_url,
                    api_key,
                },
            );
        }

        for (name, tier) in &tiers {
            if !providers.contains_key(&tier.provider) {
                return Err(Error::ConfigLoad(format!(
                    "tier '{name}': provider '{}' is not defined in [provider]",
                    tier.provider
                )));
            }
        }

        let default_tier = self
            .default_model
            .clone()
            .unwrap_or_else(|| "balanced".to_string());

        if !tiers.contains_key(&default_tier) {
            return Err(Error::ConfigLoad(format!(
                "default_model '{default_tier}' is not defined in [model]"
            )));
        }

        Ok(Config {
            tiers,
            providers,
            default_tier,
        })
    }
}

// ── Config methods ──

impl Config {
    /// Which tier is the default.
    pub fn default_tier(&self) -> &str {
        &self.default_tier
    }

    /// Look up a tier by name.
    pub fn tier(&self, name: &str) -> Option<&TierConfig> {
        self.tiers.get(name)
    }

    /// Look up a provider by name.
    pub fn provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers.get(name)
    }

    /// All tier names.
    pub fn tier_names(&self) -> Vec<&str> {
        self.tiers.keys().map(|s| s.as_str()).collect()
    }

    /// All tier names with their purpose.
    pub fn tier_infos(&self) -> Vec<TierInfo> {
        self.tiers
            .iter()
            .map(|(name, tier)| TierInfo {
                name: name.clone(),
                purpose: tier.purpose.clone(),
            })
            .collect()
    }

    /// Redacted display suitable for `kuku config show`.
    pub fn redacted_display(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("default_model = \"{}\"\n\n", self.default_tier));

        for (name, tier) in &self.tiers {
            out.push_str(&format!("[model.{name}]\n"));
            out.push_str(&format!("provider = \"{}\"\n", tier.provider));
            out.push_str(&format!("model = \"{}\"\n", tier.model));
            out.push_str(&format!("think = \"{}\"\n", tier.think.as_str()));
            out.push_str(&format!("context_window = {}\n", tier.context_window));
            out.push_str(&format!("max_output_tokens = {}\n", tier.max_output_tokens));
            if !tier.purpose.is_empty() && tier.purpose != *name {
                out.push_str(&format!("purpose = \"{}\"\n", tier.purpose));
            }
            out.push('\n');
        }

        for (name, provider) in &self.providers {
            out.push_str(&format!("[provider.{name}]\n"));
            out.push_str(&format!("format = \"{}\"\n", provider.format));
            out.push_str(&format!("base_url = \"{}\"\n", provider.base_url));
            match &provider.api_key {
                ApiKey::Plaintext(_) => out.push_str("api_key = \"<redacted>\"\n"),
                ApiKey::Env(env) => out.push_str(&format!("api_key = \"${env}\"\n")),
            }
            out.push('\n');
        }
        out
    }
}

// ── ApiKey ──

impl ApiKey {
    /// Resolve the actual key value. Returns error if an env var ref points to a missing variable.
    pub fn resolve(&self) -> Result<String> {
        match self {
            ApiKey::Env(name) => std::env::var(name).map_err(|_| {
                Error::ConfigLoad(format!(
                    "env var '{name}' referenced by api_key is not set"
                ))
            }),
            ApiKey::Plaintext(key) => Ok(key.clone()),
        }
    }
}

// ── Default generation ──

/// Generate a default config file content as a TOML string.
/// Used by the host on first run.
pub fn generate_default() -> &'static str {
    r##"# kuku configuration
# Generated by kuku init. Modify as needed.

default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"
context_window = 200000
max_output_tokens = 64000
purpose = "deep reasoning, complex analysis"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"
context_window = 200000
max_output_tokens = 48000
purpose = "general purpose, everyday tasks"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5"
think = "off"
context_window = 200000
max_output_tokens = 32000
purpose = "quick simple tasks, summaries"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "$ANTHROPIC_API_KEY"

[provider.openai]
format = "openai-responses"
base_url = "https://api.openai.com/v1"
api_key = "$OPENAI_API_KEY"
"##
}
