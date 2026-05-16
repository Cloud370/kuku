//! Config file types, loading, and typed access for `~/.kuku/config.toml`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Raw deserialized contents of `config.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(default)]
    pub(crate) model: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) provider: BTreeMap<String, ProviderEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ProviderEntry {
    pub(crate) format: String,
    #[serde(default)]
    pub(crate) api_key: Option<String>,
    #[serde(default)]
    pub(crate) api_key_env: Option<String>,
    #[serde(default)]
    pub(crate) base_url: Option<String>,
    #[serde(default)]
    pub(crate) thinking: Option<ThinkingEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ThinkingEntry {
    #[serde(default)]
    pub(crate) low: Option<toml::Value>,
    #[serde(default)]
    pub(crate) medium: Option<toml::Value>,
    #[serde(default)]
    pub(crate) high: Option<toml::Value>,
}

/// Validated, resolved configuration for provider and model setup.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub models: BTreeMap<String, String>,
    pub providers: BTreeMap<String, ResolvedProviderConfig>,
    pub default_model: String,
}

/// Validated provider entry with API key source and thinking overrides.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedProviderConfig {
    pub format: String,
    pub api_key: ApiKeySource,
    pub base_url: Option<String>,
    pub thinking: ResolvedThinking,
}

/// Whether the API key is stored inline or referenced via an environment variable.
#[derive(Debug, Clone, PartialEq)]
pub enum ApiKeySource {
    Plaintext(String),
    Env(String),
}

/// Per-level thinking budget overrides from the config file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedThinking {
    pub low: Option<toml::Value>,
    pub medium: Option<toml::Value>,
    pub high: Option<toml::Value>,
}

/// Load and parse a TOML config file. Returns empty defaults if the file does not exist.
pub fn load_config(path: &Path) -> Result<ConfigFile> {
    if !path.exists() {
        return Ok(ConfigFile {
            model: BTreeMap::new(),
            provider: BTreeMap::new(),
        });
    }
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::ConfigLoad(format!("cannot read config file {path:?}: {error}")))?;
    toml::from_str(&text).map_err(|error| Error::ConfigLoad(format!("invalid config: {error}")))
}

impl ConfigFile {
    /// Validate and resolve the raw config into typed, validated Config.
    pub fn resolve(&self) -> Result<Config> {
        let mut models = BTreeMap::new();
        for (alias, value) in &self.model {
            if alias == "default" {
                continue;
            }
            let trimmed = value.trim();
            if alias.is_empty() {
                return Err(Error::ConfigLoad("empty model alias".to_string()));
            }
            if !trimmed.contains(':') {
                return Err(Error::ConfigLoad(format!(
                    "model alias '{alias}': value must be 'provider:model-name', got '{trimmed}'"
                )));
            }
            models.insert(alias.clone(), trimmed.to_string());
        }

        let mut providers = BTreeMap::new();
        for (alias, entry) in &self.provider {
            if alias.is_empty() {
                return Err(Error::ConfigLoad("empty provider alias".to_string()));
            }
            if entry.format.is_empty() {
                return Err(Error::ConfigLoad(format!(
                    "provider '{alias}': format is required"
                )));
            }
            let api_key = match (&entry.api_key, &entry.api_key_env) {
                (Some(_), Some(_)) => {
                    return Err(Error::ConfigLoad(format!(
                        "provider '{alias}': set api_key or api_key_env, not both"
                    )))
                }
                (Some(key), None) => ApiKeySource::Plaintext(key.clone()),
                (None, Some(env)) => ApiKeySource::Env(env.clone()),
                (None, None) => {
                    return Err(Error::ConfigLoad(format!(
                        "provider '{alias}': api_key or api_key_env required"
                    )))
                }
            };

            let thinking = match &entry.thinking {
                Some(t) => ResolvedThinking {
                    low: t.low.clone(),
                    medium: t.medium.clone(),
                    high: t.high.clone(),
                },
                None => ResolvedThinking::default(),
            };

            providers.insert(
                alias.clone(),
                ResolvedProviderConfig {
                    format: entry.format.clone(),
                    api_key,
                    base_url: entry.base_url.clone(),
                    thinking,
                },
            );
        }

        let default_model = self
            .model
            .get("default")
            .cloned()
            .or_else(|| self.model.keys().find(|k| *k != "default").cloned())
            .unwrap_or_else(|| "default".to_string());

        Ok(Config {
            models,
            providers,
            default_model,
        })
    }
}

impl Config {
    /// Which model alias is the default.
    pub fn default_model(&self) -> &str {
        &self.default_model
    }

    /// Resolve a model alias to `"provider:model-name"`.
    pub fn resolve_model_alias(&self, alias: &str) -> Option<&str> {
        self.models.get(alias).map(|s| s.as_str())
    }

    /// Get the validated config for a provider alias.
    pub fn provider(&self, alias: &str) -> Option<&ResolvedProviderConfig> {
        self.providers.get(alias)
    }

    /// All model alias names.
    pub fn model_names(&self) -> Vec<&str> {
        self.models.keys().map(|s| s.as_str()).collect()
    }

    /// Full config with API keys redacted, suitable for display.
    pub fn redacted_display(&self) -> String {
        let mut out = String::new();

        out.push_str("[model]\n");
        for (alias, target) in &self.models {
            out.push_str(&format!("{alias} = \"{target}\"\n"));
        }

        out.push('\n');
        for (name, provider) in &self.providers {
            out.push_str(&format!("[provider.{name}]\n"));
            out.push_str(&format!("format = \"{}\"\n", provider.format));
            match &provider.api_key {
                ApiKeySource::Plaintext(_) => out.push_str("api_key = \"<redacted>\"\n"),
                ApiKeySource::Env(env) => out.push_str(&format!("api_key_env = \"{env}\"\n")),
            }
            if let Some(base_url) = &provider.base_url {
                out.push_str(&format!("base_url = \"{base_url}\"\n"));
            }
            if provider.thinking.low.is_some()
                || provider.thinking.medium.is_some()
                || provider.thinking.high.is_some()
            {
                out.push_str(&format!("[provider.{name}.thinking]\n"));
                if let Some(low) = &provider.thinking.low {
                    out.push_str(&format!("low = {low}\n"));
                }
                if let Some(medium) = &provider.thinking.medium {
                    out.push_str(&format!("medium = {medium}\n"));
                }
                if let Some(high) = &provider.thinking.high {
                    out.push_str(&format!("high = {high}\n"));
                }
            }
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_valid_config() {
        let toml = r#"
[model]
strong = "anthropic:claude-sonnet-4-6"
default = "strong"

[provider.anthropic]
format = "anthropic"
api_key = "sk-ant-123"
"#;
        let file: ConfigFile = toml::from_str(toml).unwrap();
        let cfg = file.resolve().unwrap();
        assert_eq!(cfg.default_model(), "strong");
        assert_eq!(
            cfg.resolve_model_alias("strong"),
            Some("anthropic:claude-sonnet-4-6")
        );
        assert_eq!(cfg.provider("anthropic").unwrap().format, "anthropic");
    }

    #[test]
    fn parses_provider_with_api_key_env() {
        let toml = r#"
[provider.openai]
format = "openai"
api_key_env = "OPENAI_API_KEY"
"#;
        let file: ConfigFile = toml::from_str(toml).unwrap();
        let cfg = file.resolve().unwrap();
        match &cfg.provider("openai").unwrap().api_key {
            ApiKeySource::Env(env) => assert_eq!(env, "OPENAI_API_KEY"),
            other => panic!("expected Env, got {other:?}"),
        }
    }

    #[test]
    fn parses_thinking_overrides() {
        let toml = r#"
[provider.anthropic]
format = "anthropic"
api_key = "sk-ant-123"

[provider.anthropic.thinking]
low = 2048
medium = 8192
high = 32000
"#;
        let file: ConfigFile = toml::from_str(toml).unwrap();
        let cfg = file.resolve().unwrap();
        let thinking = &cfg.provider("anthropic").unwrap().thinking;
        assert!(thinking.low.is_some());
        assert!(thinking.medium.is_some());
        assert!(thinking.high.is_some());
    }

    #[test]
    fn missing_config_file_returns_empty_defaults() {
        let file = load_config(Path::new("/nonexistent/config.toml")).unwrap();
        let cfg = file.resolve().unwrap();
        assert!(cfg.models.is_empty());
        assert!(cfg.providers.is_empty());
    }

    #[test]
    fn rejects_model_value_without_colon() {
        let toml = r#"
[model]
bad = "no-colon-here"
"#;
        let file: ConfigFile = toml::from_str(toml).unwrap();
        let err = file.resolve().unwrap_err();
        assert!(err.to_string().contains("no-colon-here"));
    }

    #[test]
    fn rejects_both_api_key_and_api_key_env() {
        let toml = r#"
[provider.dup]
format = "anthropic"
api_key = "sk-123"
api_key_env = "ENV_KEY"
"#;
        let file: ConfigFile = toml::from_str(toml).unwrap();
        let err = file.resolve().unwrap_err();
        assert!(err.to_string().contains("not both"));
    }

    #[test]
    fn redacted_display_hides_plaintext_keys() {
        let toml = r#"
[model]
strong = "anthropic:claude-sonnet-4-6"

[provider.anthropic]
format = "anthropic"
api_key = "sk-ant-secret"

[provider.openai]
format = "openai"
api_key_env = "OPENAI_API_KEY"
"#;
        let file: ConfigFile = toml::from_str(toml).unwrap();
        let cfg = file.resolve().unwrap();
        let display = cfg.redacted_display();
        assert!(!display.contains("sk-ant-secret"));
        assert!(display.contains("<redacted>"));
        assert!(display.contains("OPENAI_API_KEY"));
    }
}
