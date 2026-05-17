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
