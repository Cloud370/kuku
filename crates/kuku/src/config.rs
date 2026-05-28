//! Config file types, loading, and typed access for `~/.kuku/config.toml`.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;

use std::path::PathBuf;

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

    #[serde(default)]
    pub discovery: Option<DiscoveryConfig>,

    #[serde(default, deserialize_with = "deserialize_handoff")]
    pub handoff: Option<HandoffConfig>,
}

fn default_true() -> bool {
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
    pub discovery: DiscoveryConfig,
    pub handoff: HandoffConfig,
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

#[derive(Deserialize)]
struct RawHandoffConfig {
    enabled: Option<bool>,
    threshold: Option<f64>,
    keep_turns: Option<usize>,
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

fn deserialize_handoff<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<HandoffConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<RawHandoffConfig>::deserialize(deserializer)?;
    Ok(raw.map(HandoffConfig::from))
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
        self.validate_required_tiers()?;
        self.validate_tier_entries()?;
        self.validate_provider_entries()?;
        self.validate_cross_references()?;

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
                    format: entry.format.trim().to_string(),
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

        Ok(Config {
            tiers,
            providers,
            default_tier,
            discovery,
            handoff,
        })
    }

    fn validate_structural(&self) -> Result<()> {
        self.validate_required_tiers()?;
        self.validate_tier_entries()?;
        self.validate_provider_entries()?;
        self.validate_cross_references()
    }

    fn validate_required_tiers(&self) -> Result<()> {
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

    fn validate_tier_entries(&self) -> Result<()> {
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
                if !matches!(think.as_str(), "off" | "low" | "medium" | "high") {
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

    fn validate_provider_entries(&self) -> Result<()> {
        for (name, entry) in &self.provider {
            let format = entry.format.trim();
            if format.is_empty() {
                return Err(Error::ConfigLoad(format!(
                    "provider '{name}': format is required"
                )));
            }
            Self::validate_format(format).map_err(|_| {
                Error::ConfigLoad(format!(
                    "provider '{name}': format '{format}' is not supported, must be anthropic/openai-chat/openai-responses"
                ))
            })?;
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

    fn validate_cross_references(&self) -> Result<()> {
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

    fn validate_format(format: &str) -> Result<()> {
        if !matches!(format, "anthropic" | "openai-chat" | "openai-responses") {
            return Err(Error::ConfigLoad(format!(
                "'format' must be anthropic/openai-chat/openai-responses, got '{format}'"
            )));
        }
        Ok(())
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

    /// Return the handoff configuration.
    pub fn handoff(&self) -> HandoffConfig {
        self.handoff.clone()
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
                Error::ConfigLoad(format!("env var '{name}' referenced by api_key is not set"))
            }),
            ApiKey::Plaintext(key) => Ok(key.clone()),
        }
    }
}

// ── Default generation ──

/// Generate a default config file content as a TOML string.
/// Used by the host on first run.
pub fn generate_default() -> &'static str {
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/default-config.toml"
    ))
}

/// Detect missing config sections via struct deserialization, then inject
/// defaults using `toml_edit` to preserve user comments and formatting.
pub fn config_patch_defaults(raw: &str) -> Result<(String, bool)> {
    let file: ConfigFile = toml::from_str(raw)
        .map_err(|error| Error::ConfigLoad(format!("invalid config: {error}")))?;

    if file.handoff.is_some() {
        return Ok((raw.to_string(), false));
    }

    let mut doc: toml_edit::DocumentMut = raw
        .parse()
        .map_err(|error| Error::ConfigLoad(format!("invalid config: {error}")))?;

    inject_handoff_section(&mut doc);

    Ok((doc.to_string(), true))
}

fn inject_handoff_section(doc: &mut toml_edit::DocumentMut) {
    let mut section = toml_edit::Table::new();
    *section.decor_mut() = toml_edit::Decor::new(
        "\n\n# Context handoff: inject summary when context usage exceeds threshold.\n",
        "",
    );
    section["enabled"] = toml_edit::value(true);
    section["threshold"] = toml_edit::value(0.7);
    section["keep_turns"] = toml_edit::value(2);

    doc["handoff"] = toml_edit::Item::Table(section);
}

/// Load config from disk, patch missing sections, write back atomically if changed.
/// Returns the resolved ConfigFile. Preserves user comments via `toml_edit`.
pub fn load_and_patch_config(path: &Path) -> Result<ConfigFile> {
    let raw = std::fs::read_to_string(path)
        .map_err(|error| Error::ConfigLoad(format!("cannot read config file {path:?}: {error}")))?;
    let (patched, changed) = config_patch_defaults(&raw)?;
    if changed {
        let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let mut temp = tempfile::NamedTempFile::new_in(dir)
            .map_err(|error| Error::ConfigLoad(format!("cannot create temp file: {error}")))?;
        std::io::Write::write_all(&mut temp, patched.as_bytes())
            .map_err(|error| Error::ConfigLoad(format!("cannot write config: {error}")))?;
        temp.persist(path)
            .map_err(|error| Error::ConfigLoad(format!("cannot save config: {error}")))?;
    }
    toml::from_str(&patched).map_err(|error| Error::ConfigLoad(format!("invalid config: {error}")))
}

/// Modify a single value in a config file by dot-notation key.
///
/// Validates the modified config structurally (cross-references, types, enum values).
/// Does NOT validate runtime constraints (env var existence).
/// Preserves comments and formatting via `toml_edit`.
pub fn set_value(path: &Path, dot_key: &str, value: &str) -> Result<()> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::ConfigLoad(format!("cannot read config: {error}")))?;

    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error| Error::ConfigLoad(format!("invalid config: {error}")))?;

    let parts: Vec<&str> = dot_key.split('.').collect();
    if parts.is_empty() {
        return Err(Error::ConfigLoad("empty config key".to_string()));
    }

    let (parent_parts, leaf_key) = parts.split_at(parts.len() - 1);
    let leaf = leaf_key[0];

    let mut table = doc.as_table_mut();
    for part in parent_parts {
        let item = table.get_mut(part).ok_or_else(|| {
            Error::ConfigLoad(format!("unknown config key: segment '{part}' not found"))
        })?;
        table = item.as_table_mut().ok_or_else(|| {
            Error::ConfigLoad(format!(
                "unknown config key: segment '{part}' is not a table"
            ))
        })?;
    }

    let new_value = match leaf {
        "context_window" | "max_output_tokens" => {
            let n: i64 = value.parse().map_err(|_| {
                Error::ConfigLoad(format!(
                    "'{dot_key}' must be a positive integer, got '{value}'"
                ))
            })?;
            if n <= 0 {
                return Err(Error::ConfigLoad(format!(
                    "'{dot_key}' must be a positive integer, got '{value}'"
                )));
            }
            toml_edit::value(n)
        }
        "think" => {
            if !matches!(value, "off" | "low" | "medium" | "high") {
                return Err(Error::ConfigLoad(format!(
                    "'think' must be off/low/medium/high, got '{value}'"
                )));
            }
            toml_edit::value(value)
        }
        "format" => {
            ConfigFile::validate_format(value)?;
            toml_edit::value(value)
        }
        _ => toml_edit::value(value),
    };

    table[leaf] = new_value;

    let modified_text = doc.to_string();
    let config_file: ConfigFile = toml::from_str(&modified_text)
        .map_err(|error| Error::ConfigLoad(format!("modified config is invalid: {error}")))?;
    config_file.validate_structural()?;

    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(dir)
        .map_err(|error| Error::ConfigLoad(format!("cannot create temp file: {error}")))?;
    temp.write_all(modified_text.as_bytes())
        .map_err(|error| Error::ConfigLoad(format!("cannot write config: {error}")))?;
    temp.persist(path)
        .map_err(|error| Error::ConfigLoad(format!("cannot save config: {error}")))?;

    Ok(())
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
mod set_value_tests {
    use super::*;
    use std::fs;

    fn temp_config(content: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), content).unwrap();
        dir
    }

    const FULL_CONFIG: &str = r#"# kuku config
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"
context_window = 200000
max_output_tokens = 64000
purpose = "deep reasoning"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"
context_window = 200000
max_output_tokens = 48000
purpose = "general purpose"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"
context_window = 200000
max_output_tokens = 32000
purpose = "quick tasks"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key-anthropic"
"#;

    #[test]
    fn set_value_updates_string_field() {
        let dir = temp_config(FULL_CONFIG);
        let path = dir.path().join("config.toml");

        set_value(&path, "default_model", "strong").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains(r#"default_model = "strong""#));
        assert!(!content.contains(r#"default_model = "balanced""#));
    }

    #[test]
    fn set_value_updates_nested_string_field() {
        let dir = temp_config(FULL_CONFIG);
        let path = dir.path().join("config.toml");

        set_value(&path, "model.balanced.think", "high").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("think = \"high\""));
    }

    #[test]
    fn set_value_updates_integer_field() {
        let dir = temp_config(FULL_CONFIG);
        let path = dir.path().join("config.toml");

        set_value(&path, "model.balanced.context_window", "128000").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("context_window = 128000"));
    }

    #[test]
    fn set_value_preserves_comments() {
        let dir = temp_config(FULL_CONFIG);
        let path = dir.path().join("config.toml");

        set_value(&path, "default_model", "light").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("# kuku config"));
    }

    #[test]
    fn set_value_rejects_invalid_think_level() {
        let dir = temp_config(FULL_CONFIG);
        let path = dir.path().join("config.toml");

        let error = set_value(&path, "model.balanced.think", "invalid").unwrap_err();
        assert!(error.to_string().contains("think"));
        assert!(error.to_string().contains("off/low/medium/high"));
    }

    #[test]
    fn set_value_rejects_invalid_format() {
        let dir = temp_config(FULL_CONFIG);
        let path = dir.path().join("config.toml");

        let error = set_value(&path, "provider.anthropic.format", "invalid").unwrap_err();
        assert!(error.to_string().contains("format"));
    }

    #[test]
    fn set_value_rejects_zero_context_window() {
        let dir = temp_config(FULL_CONFIG);
        let path = dir.path().join("config.toml");

        let error = set_value(&path, "model.balanced.context_window", "0").unwrap_err();
        assert!(error.to_string().contains("context_window"));
    }

    #[test]
    fn set_value_rejects_non_numeric_for_integer_field() {
        let dir = temp_config(FULL_CONFIG);
        let path = dir.path().join("config.toml");

        let error = set_value(&path, "model.balanced.context_window", "abc").unwrap_err();
        assert!(error.to_string().contains("positive integer"));
        assert!(error.to_string().contains("abc"));
    }

    #[test]
    fn set_value_rejects_negative_integer() {
        let dir = temp_config(FULL_CONFIG);
        let path = dir.path().join("config.toml");

        let error = set_value(&path, "model.balanced.context_window", "-1").unwrap_err();
        assert!(error.to_string().contains("positive integer"));
    }

    #[test]
    fn set_value_rejects_zero_max_output_tokens() {
        let dir = temp_config(FULL_CONFIG);
        let path = dir.path().join("config.toml");

        let error = set_value(&path, "model.balanced.max_output_tokens", "0").unwrap_err();
        assert!(error.to_string().contains("positive integer"));
    }

    #[test]
    fn set_value_rejects_missing_subsection() {
        let dir = temp_config(FULL_CONFIG);
        let path = dir.path().join("config.toml");

        let error = set_value(&path, "model.nonexistent.think", "high").unwrap_err();
        assert!(error.to_string().contains("unknown config key"));
    }

    #[test]
    fn set_value_rejects_invalid_provider_reference() {
        let dir = temp_config(FULL_CONFIG);
        let path = dir.path().join("config.toml");

        let error = set_value(&path, "model.balanced.provider", "nonexistent").unwrap_err();
        assert!(error.to_string().contains("provider"));
    }

    #[test]
    fn set_value_rejects_unknown_dot_path() {
        let dir = temp_config(FULL_CONFIG);
        let path = dir.path().join("config.toml");

        let error = set_value(&path, "unknown.field", "value").unwrap_err();
        assert!(error.to_string().contains("unknown config key"));
    }
}

#[cfg(test)]
mod show_redacted_tests {
    use super::*;
    use std::fs;

    fn temp_config(content: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), content).unwrap();
        dir
    }

    #[test]
    fn show_redacted_masks_plaintext_api_key() {
        let config = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"
context_window = 200000
max_output_tokens = 64000
purpose = "deep reasoning"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"
context_window = 200000
max_output_tokens = 48000
purpose = "general purpose"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"
context_window = 200000
max_output_tokens = 32000
purpose = "quick tasks"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "sk-ant-secret123"
"#;
        let dir = temp_config(config);
        let path = dir.path().join("config.toml");

        let output = show_redacted(&path).unwrap();
        assert!(output.contains("<redacted>"));
        assert!(!output.contains("sk-ant-secret123"));
    }

    #[test]
    fn show_redacted_preserves_env_var_reference() {
        let _guard = crate::env_lock().lock().unwrap();
        std::env::set_var("_KUKU_TEST_SHOW_KEY", "test-value");
        let config = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"
context_window = 200000
max_output_tokens = 64000
purpose = "deep reasoning"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"
context_window = 200000
max_output_tokens = 48000
purpose = "general purpose"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"
context_window = 200000
max_output_tokens = 32000
purpose = "quick tasks"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "$_KUKU_TEST_SHOW_KEY"
"#;
        let dir = temp_config(config);
        let path = dir.path().join("config.toml");

        let output = show_redacted(&path).unwrap();
        assert!(output.contains("$_KUKU_TEST_SHOW_KEY"));
        std::env::remove_var("_KUKU_TEST_SHOW_KEY");
    }

    #[test]
    fn show_redacted_errors_on_missing_file() {
        let error = show_redacted(std::path::Path::new("/nonexistent/config.toml")).unwrap_err();
        assert!(error.to_string().contains("required tier"));
    }

    #[test]
    fn show_redacted_errors_on_invalid_config() {
        let dir = temp_config("not valid toml [[[");
        let path = dir.path().join("config.toml");

        let error = show_redacted(&path).unwrap_err();
        assert!(error.to_string().contains("invalid config"));
    }
}

#[cfg(test)]
mod discovery_config_tests {
    use super::*;

    #[test]
    fn discovery_config_from_toml() {
        let toml = r#"
[discovery]
auto_discover = false
extra_user_paths = ["/opt/skills"]
extra_project_paths = [".custom/agents"]
"#;
        let file: ConfigFile = toml::from_str(toml).unwrap();
        let disc = file.discovery.unwrap();
        assert!(!disc.auto_discover);
        assert_eq!(disc.extra_user_paths.len(), 1);
        assert_eq!(disc.extra_project_paths.len(), 1);
    }

    #[test]
    fn discovery_config_defaults_when_absent() {
        let toml = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"
"#;
        let file: ConfigFile = toml::from_str(toml).unwrap();
        let disc = file.discovery.unwrap_or_default();
        assert!(disc.auto_discover);
        assert!(disc.extra_user_paths.is_empty());
    }

    #[test]
    fn discovery_config_propagated_to_resolved_config() {
        let toml = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"

[discovery]
auto_discover = false
"#;
        let file: ConfigFile = toml::from_str(toml).unwrap();
        let config = file.resolve().unwrap();
        assert!(!config.discovery.auto_discover);
    }
}

#[cfg(test)]
mod handoff_config_tests {
    use super::*;

    const VALID_CONFIG: &str = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"
"#;

    #[test]
    fn handoff_config_round_trip() {
        let toml_str = format!(
            "{VALID_CONFIG}\n[handoff]\nenabled = false\nthreshold = 0.5\nkeep_turns = 3\n"
        );
        let file: ConfigFile = toml::from_str(&toml_str).unwrap();
        let h = file.handoff.as_ref().unwrap();
        assert!(!h.enabled);
        assert!((h.threshold - 0.5).abs() < f64::EPSILON);
        assert_eq!(h.keep_turns, 3);

        let serialized = toml::to_string(&file).unwrap();
        let file2: ConfigFile = toml::from_str(&serialized).unwrap();
        assert_eq!(file.handoff, file2.handoff);
    }

    #[test]
    fn handoff_config_defaults_when_absent() {
        let file: ConfigFile = toml::from_str(VALID_CONFIG).unwrap();
        assert!(file.handoff.is_none());
        let config = file.resolve().unwrap();
        assert!(config.handoff.enabled);
        assert!((config.handoff.threshold - 0.7).abs() < f64::EPSILON);
        assert_eq!(config.handoff.keep_turns, 2);
    }

    #[test]
    fn handoff_config_partial() {
        let toml_str = format!("{VALID_CONFIG}\n[handoff]\nenabled = false\n");
        let file: ConfigFile = toml::from_str(&toml_str).unwrap();
        let config = file.resolve().unwrap();
        assert!(!config.handoff.enabled);
        assert!((config.handoff.threshold - 0.7).abs() < f64::EPSILON);
        assert_eq!(config.handoff.keep_turns, 2);
    }

    #[test]
    fn handoff_config_invalid_threshold() {
        let toml_str = format!("{VALID_CONFIG}\n[handoff]\nthreshold = 1.5\n");
        let file: ConfigFile = toml::from_str(&toml_str).unwrap();
        let err = file.resolve().unwrap_err();
        assert!(err.to_string().contains("threshold"));
    }

    #[test]
    fn handoff_config_zero_keep_turns() {
        let toml_str = format!("{VALID_CONFIG}\n[handoff]\nkeep_turns = 0\n");
        let file: ConfigFile = toml::from_str(&toml_str).unwrap();
        let err = file.resolve().unwrap_err();
        assert!(err.to_string().contains("keep_turns"));
    }

    #[test]
    fn handoff_propagation() {
        let toml_str = format!(
            "{VALID_CONFIG}\n[handoff]\nenabled = false\nthreshold = 0.5\nkeep_turns = 3\n"
        );
        let file: ConfigFile = toml::from_str(&toml_str).unwrap();
        let config = file.resolve().unwrap();
        assert!(!config.handoff.enabled);
        assert!((config.handoff.threshold - 0.5).abs() < f64::EPSILON);
        assert_eq!(config.handoff.keep_turns, 3);
    }

    #[test]
    fn generate_default_includes_all_sections() {
        let toml_str = generate_default();
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        assert!(!file.model.is_empty());
        assert!(!file.provider.is_empty());
        assert!(file.discovery.is_some());
        assert!(file.handoff.is_some());
    }
}

#[cfg(test)]
mod patch_defaults_tests {
    use super::*;

    const FULL_CONFIG: &str = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"

[handoff]
enabled = true
threshold = 0.8
keep_turns = 3
"#;

    #[test]
    fn no_change_when_complete() {
        let (patched, changed) = config_patch_defaults(FULL_CONFIG).unwrap();
        assert!(!changed);
        assert_eq!(patched, FULL_CONFIG);
    }

    #[test]
    fn fills_missing_handoff() {
        let input = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"
"#;
        let (patched, changed) = config_patch_defaults(input).unwrap();
        assert!(changed);
        let file: ConfigFile = toml::from_str(&patched).unwrap();
        assert!(file.handoff.is_some());
        let h = file.handoff.unwrap();
        assert!(h.enabled);
        assert!((h.threshold - 0.7).abs() < f64::EPSILON);
        assert_eq!(h.keep_turns, 2);
    }

    #[test]
    fn preserves_user_comments() {
        let input = r#"# My custom config
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"
"#;
        let (patched, _) = config_patch_defaults(input).unwrap();
        assert!(patched.contains("# My custom config"));
    }

    #[test]
    fn preserves_existing_handoff_values() {
        let input = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"

[handoff]
enabled = false
threshold = 0.5
keep_turns = 5
"#;
        let (patched, changed) = config_patch_defaults(input).unwrap();
        assert!(!changed);
        let file: ConfigFile = toml::from_str(&patched).unwrap();
        let h = file.handoff.unwrap();
        assert!(!h.enabled);
        assert!((h.threshold - 0.5).abs() < f64::EPSILON);
        assert_eq!(h.keep_turns, 5);
    }

    #[test]
    fn rejects_invalid_toml() {
        let result = config_patch_defaults("not [valid toml");
        assert!(result.is_err());
    }
}
