use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

// ── Provider format (moved from provider/ to break reverse dependency) ──

/// Wire format selector for provider API protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderFormat {
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai-chat")]
    OpenAiChat,
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
}

impl ProviderFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderFormat::Anthropic => "anthropic",
            ProviderFormat::OpenAiChat => "openai-chat",
            ProviderFormat::OpenAiResponses => "openai-responses",
        }
    }
}

impl std::str::FromStr for ProviderFormat {
    type Err = &'static str;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "anthropic" => Ok(ProviderFormat::Anthropic),
            "openai-chat" => Ok(ProviderFormat::OpenAiChat),
            "openai-responses" => Ok(ProviderFormat::OpenAiResponses),
            _ => Err("format must be anthropic/openai-chat/openai-responses"),
        }
    }
}

// ── Raw config file ──

/// Raw deserialized contents of `config.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigFile {
    pub default_model: Option<String>,

    #[serde(default)]
    pub model: BTreeMap<String, ModelEntry>,

    #[serde(default)]
    pub provider: BTreeMap<String, ProviderEntry>,

    #[serde(default)]
    pub discovery: Option<DiscoveryConfig>,

    #[serde(default, deserialize_with = "deserialize_handoff")]
    pub handoff: Option<HandoffConfig>,

    #[serde(default, deserialize_with = "deserialize_logs")]
    pub logs: Option<LogsConfig>,

    #[serde(default)]
    pub plugin: Option<PluginConfig>,

    #[serde(default)]
    pub update: Option<UpdateConfig>,
}

pub(crate) fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryConfig {
    #[serde(default = "default_true")]
    pub auto_discover: bool,
    #[serde(default)]
    pub extra_user_paths: Vec<PathBuf>,
    #[serde(default)]
    pub extra_project_paths: Vec<PathBuf>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            auto_discover: true,
            extra_user_paths: Vec::new(),
            extra_project_paths: Vec::new(),
        }
    }
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
    pub format: ProviderFormat,
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
    pub discovery: DiscoveryConfig,
    pub handoff: HandoffConfig,
    pub logs: LogsConfig,
    pub plugin: PluginConfig,
    pub update: UpdateConfig,
}

/// Configuration for session handoff behaviour (threshold, history retention).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandoffConfig {
    pub enabled: bool,
    pub threshold: f64,
    pub keep_turns: usize,
}

impl Default for HandoffConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 0.7,
            keep_turns: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogsConfig {
    pub max_age_days: u32,
    pub max_total_size_mb: u32,
}

impl Default for LogsConfig {
    fn default() -> Self {
        Self {
            max_age_days: 14,
            max_total_size_mb: 512,
        }
    }
}

#[derive(Deserialize)]
struct RawHandoffConfig {
    enabled: Option<bool>,
    threshold: Option<f64>,
    keep_turns: Option<usize>,
}

#[derive(Deserialize)]
struct RawLogsConfig {
    max_age_days: Option<u32>,
    max_total_size_mb: Option<u32>,
}

impl From<RawHandoffConfig> for HandoffConfig {
    fn from(raw: RawHandoffConfig) -> Self {
        let defaults = HandoffConfig::default();
        Self {
            enabled: raw.enabled.unwrap_or(defaults.enabled),
            threshold: raw.threshold.unwrap_or(defaults.threshold),
            keep_turns: raw.keep_turns.unwrap_or(defaults.keep_turns),
        }
    }
}

impl From<RawLogsConfig> for LogsConfig {
    fn from(raw: RawLogsConfig) -> Self {
        let defaults = LogsConfig::default();
        Self {
            max_age_days: raw.max_age_days.unwrap_or(defaults.max_age_days),
            max_total_size_mb: raw.max_total_size_mb.unwrap_or(defaults.max_total_size_mb),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateConfig {
    pub source: String,
    pub channel: String,
    #[serde(default)]
    pub sources: BTreeMap<String, String>,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            source: "github".into(),
            channel: "stable".into(),
            sources: BTreeMap::new(),
        }
    }
}

fn deserialize_handoff<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<HandoffConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<RawHandoffConfig>::deserialize(deserializer)?;
    Ok(raw.map(HandoffConfig::from))
}

fn deserialize_logs<'de, D>(deserializer: D) -> std::result::Result<Option<LogsConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<RawLogsConfig>::deserialize(deserializer)?;
    Ok(raw.map(LogsConfig::from))
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
    pub format: ProviderFormat,
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

    pub fn overhead_tokens(&self) -> u32 {
        match self {
            ThinkLevel::Off => 0,
            ThinkLevel::Low => 1024,
            ThinkLevel::Medium => 4096,
            ThinkLevel::High => 16000,
        }
    }
}

impl FromStr for ThinkLevel {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "off" => Ok(ThinkLevel::Off),
            "low" => Ok(ThinkLevel::Low),
            "medium" => Ok(ThinkLevel::Medium),
            "high" => Ok(ThinkLevel::High),
            _ => Err(Error::ConfigLoad(format!(
                "unknown ThinkLevel: '{s}', must be off/low/medium/high"
            ))),
        }
    }
}

/// Per-level thinking budget overrides (retained for provider adapter override support).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedThinking {
    pub low: Option<toml::Value>,
    pub medium: Option<toml::Value>,
    pub high: Option<toml::Value>,
}

/// Lightweight tier info for prompt injection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierInfo {
    pub name: String,
    pub purpose: String,
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

    /// Return the handoff configuration.
    pub fn handoff(&self) -> HandoffConfig {
        self.handoff.clone()
    }

    /// Return the logs configuration.
    pub fn logs(&self) -> LogsConfig {
        self.logs.clone()
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
            out.push_str(&format!("format = \"{}\"\n", provider.format.as_str()));
            out.push_str(&format!("base_url = \"{}\"\n", provider.base_url));
            match &provider.api_key {
                ApiKey::Plaintext(_) => out.push_str("api_key = \"<redacted>\"\n"),
                ApiKey::Env(env) => out.push_str(&format!("api_key = \"${env}\"\n")),
            }
            out.push('\n');
        }

        out.push_str("[logs]\n");
        out.push_str(&format!("max_age_days = {}\n", self.logs.max_age_days));
        out.push_str(&format!(
            "max_total_size_mb = {}\n\n",
            self.logs.max_total_size_mb
        ));
        out
    }
}
