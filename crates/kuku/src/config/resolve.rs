use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;

use crate::error::{Error, Result};

use super::types::{ApiKey, Config, ConfigFile, ModelConfig, ModelRef, ProviderConfig, ThinkLevel};

/// Load and parse a TOML config file. Returns an empty configuration if the file does not exist.
pub fn load_config(path: &Path) -> Result<ConfigFile> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_config_text(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ConfigFile::default()),
        Err(error) => Err(Error::ConfigLoad(format!(
            "cannot read config file {path:?}: {error}"
        ))),
    }
}

/// Parse config TOML text using the SDK's environment-reference expansion semantics.
pub fn parse_config_text(raw: &str) -> Result<ConfigFile> {
    parse_config_text_with_env(raw, |name| std::env::var(name))
}

/// Parse config TOML text using the supplied environment-variable lookup.
/// Keep API credential and request field references for the transport to resolve.
pub fn parse_config_text_with_env(
    raw: &str,
    environment: impl Fn(&str) -> std::result::Result<String, std::env::VarError>,
) -> Result<ConfigFile> {
    let mut value: toml::Value = toml::from_str(raw)
        .map_err(|error| Error::ConfigLoad(format!("invalid TOML: {}", error.message())))?;
    validate_model_shape(&value)?;
    resolve_env_refs(&mut value, &mut Vec::new(), &environment)?;
    let mut config: ConfigFile = value
        .try_into()
        .map_err(|error| Error::ConfigLoad(format!("invalid config: {}", error.message())))?;
    for (connection, registrations) in &mut config.model {
        if !registrations.keys().any(|model| model.starts_with('$')) {
            continue;
        }
        for (id, entry) in std::mem::take(registrations) {
            let path = ModelRef::new(connection.as_str(), &id).config_path();
            let model = resolve_env_ref(&id, &path, &environment)?.unwrap_or(id);
            if registrations.insert(model.clone(), entry).is_some() {
                return Err(config_field(
                    path,
                    format!(
                        "duplicates the registration of model {model:?} on connection {connection:?}"
                    ),
                ));
            }
        }
    }
    Ok(config)
}

fn validate_model_shape(value: &toml::Value) -> Result<()> {
    if let Some(default) = value.get("default_model") {
        if default.is_str() {
            return Err(config_field(
                "default_model",
                "named model tiers were replaced by { connection = \"name\", model = \"exact-id\" }",
            ));
        }
        let default = default.as_table().ok_or_else(|| {
            config_field(
                "default_model",
                "expected a table with connection and exact model ID",
            )
        })?;
        for field in ["connection", "model"] {
            if !default.get(field).is_some_and(toml::Value::is_str) {
                return Err(config_field(
                    format!("default_model.{field}"),
                    "a string is required",
                ));
            }
        }
    }
    if let Some(models) = value.get("model").and_then(toml::Value::as_table) {
        for (name, entry) in models {
            if ["provider", "model"]
                .iter()
                .any(|key| entry.get(key).is_some_and(toml::Value::is_str))
            {
                return Err(config_field(
                    format!("model.{}", toml_edit::Key::new(name)),
                    "named model tiers were replaced by [model.<connection>.\"exact-id\"]",
                ));
            }
        }
    }
    Ok(())
}

fn resolve_env_refs(
    value: &mut toml::Value,
    path: &mut Vec<String>,
    environment: &impl Fn(&str) -> std::result::Result<String, std::env::VarError>,
) -> Result<()> {
    match value {
        toml::Value::String(raw) if raw.starts_with('$') => {
            let field = path
                .iter()
                .map(|segment| toml_edit::Key::new(segment).to_string())
                .collect::<Vec<_>>()
                .join(".");
            if let Some(resolved) = resolve_env_ref(raw, &field, environment)? {
                *raw = resolved;
            }
        }
        toml::Value::Array(items) => {
            for item in items {
                resolve_env_refs(item, path, environment)?;
            }
        }
        toml::Value::Table(table) => {
            for (key, item) in table {
                path.push(key.clone());
                if !is_transport_ref(path) {
                    resolve_env_refs(item, path, environment)?;
                }
                path.pop();
            }
        }
        _ => {}
    }
    Ok(())
}

fn is_transport_ref(path: &[String]) -> bool {
    let [root, _, ..] = path else {
        return false;
    };
    let request_field = match root.as_str() {
        "provider" => 2,
        "model" => 3,
        _ => return false,
    };
    let Some(fields) = path.get(request_field..) else {
        return false;
    };
    match fields {
        [key] if root == "provider" && key == "api_key" => true,
        [request, key] => request == "request" && matches!(key.as_str(), "headers" | "body"),
        _ => false,
    }
}

fn resolve_env_ref(
    raw: &str,
    field: &str,
    environment: &impl Fn(&str) -> std::result::Result<String, std::env::VarError>,
) -> Result<Option<String>> {
    raw.strip_prefix('$')
        .map(|name| {
            if name.is_empty() {
                return Err(config_field(
                    field,
                    "'$' must be followed by an env var name",
                ));
            }
            environment(name)
                .map_err(|_| config_field(field, format!("env var {name:?} is not set")))
        })
        .transpose()
}

impl ConfigFile {
    /// Validate and resolve the parsed file to runtime configuration.
    /// The default pair is registered with default settings when its model table is absent.
    pub fn resolve(&self) -> Result<Config> {
        let default_model = self.validate_cross_references()?.clone();
        let providers = self.resolve_providers()?;
        let mut models = self.resolve_models()?;
        models.entry(default_model.clone()).or_default();
        let discovery = self.discovery.clone().unwrap_or_default();

        let handoff = self.handoff.clone().unwrap_or_default();
        if !(0.0..=1.0).contains(&handoff.threshold) {
            return Err(config_field(
                "handoff.threshold",
                "must be between 0.0 and 1.0",
            ));
        }
        if handoff.keep_turns == 0 {
            return Err(config_field("handoff.keep_turns", "must be >= 1"));
        }

        let logs = self.logs.clone().unwrap_or_default();
        if logs.max_age_days == 0 {
            return Err(config_field("logs.max_age_days", "must be >= 1"));
        }
        if logs.max_total_size_mb == 0 {
            return Err(config_field("logs.max_total_size_mb", "must be >= 1"));
        }

        Ok(Config {
            models,
            providers,
            default_model,
            discovery,
            handoff,
            logs,
            plugin: self.plugin.clone().unwrap_or_default(),
            update: self.update.clone().unwrap_or_default(),
        })
    }

    /// Validate and resolve named connections without requiring a default model or loading credentials.
    pub fn resolve_providers(&self) -> Result<BTreeMap<String, ProviderConfig>> {
        self.validate_provider_entries()?;
        Ok(self
            .provider
            .iter()
            .map(|(name, entry)| {
                let api_key_raw = entry.api_key.trim();
                let api_key = if let Some(path) = api_key_raw.strip_prefix("file:") {
                    ApiKey::File(path.into())
                } else if let Some(env_name) = api_key_raw.strip_prefix('$') {
                    ApiKey::Env(env_name.into())
                } else {
                    ApiKey::Plaintext(api_key_raw.into())
                };
                (
                    name.clone(),
                    ProviderConfig {
                        format: entry.format,
                        base_url: entry.base_url.trim().into(),
                        api_key,
                        options: entry.options.clone(),
                    },
                )
            })
            .collect())
    }

    fn resolve_models(&self) -> Result<BTreeMap<ModelRef, ModelConfig>> {
        let defaults = ModelConfig::default();
        let mut models = BTreeMap::new();
        for (connection, registrations) in &self.model {
            for (model, entry) in registrations {
                let selection = ModelRef::new(connection, model);
                let path = selection.config_path();
                validate_identity(model, &path, "model ID")?;
                if let Some(request) = &entry.request {
                    request.validate().map_err(|error| {
                        config_field(format!("{path}.request"), error.to_string())
                    })?;
                }
                let think = match &entry.think {
                    Some(think) => ThinkLevel::from_str(think).map_err(|_| {
                        config_field(
                            format!("{path}.think"),
                            format!("value {think:?} must be provider-default/off/low/medium/high"),
                        )
                    })?,
                    None => defaults.think,
                };
                for (field, value) in [
                    ("context_window", entry.context_window),
                    ("max_output_tokens", entry.max_output_tokens),
                ] {
                    if value == Some(0) {
                        return Err(config_field(
                            format!("{path}.{field}"),
                            "must be a positive integer",
                        ));
                    }
                }
                models.insert(
                    selection,
                    ModelConfig {
                        think,
                        reasoning_effort: entry.reasoning_effort.clone(),
                        service_tier: entry.service_tier.clone(),
                        context_window: entry.context_window.unwrap_or(defaults.context_window),
                        max_output_tokens: entry.max_output_tokens,
                        purpose: entry.purpose.clone().unwrap_or_default(),
                        output: entry.output.unwrap_or_default(),
                        request: entry.request.clone(),
                    },
                );
            }
        }
        Ok(models)
    }

    fn validate_provider_entries(&self) -> Result<()> {
        for (name, entry) in &self.provider {
            let path = format!("provider.{}", toml_edit::Key::new(name));
            let field = |suffix: &str| format!("{path}.{suffix}");
            validate_identity(name, &path, "connection name")?;
            if let Some(request) = &entry.options.request {
                request
                    .validate()
                    .map_err(|error| config_field(field("request"), error.to_string()))?;
            }
            if entry.base_url.trim().is_empty() && entry.options.endpoint_url.is_none() {
                return Err(config_field(
                    field("base_url"),
                    "base_url or endpoint_url is required",
                ));
            }
            let endpoint_field = if entry.options.endpoint_url.is_some() {
                "endpoint_url"
            } else {
                "base_url"
            };
            super::resolve_request_url(
                entry.format,
                &entry.base_url,
                entry.options.endpoint_url.as_deref(),
            )
            .map_err(|error| config_field(field(endpoint_field), error.to_string()))?;
            if entry.options.models_url.is_some() {
                super::resolve_models_url(entry.format, &entry.base_url, &entry.options)
                    .map_err(|error| config_field(field("models_url"), error.to_string()))?;
            }
            if let Some(account) = &entry.options.subscription {
                if !entry.api_key.trim().is_empty() {
                    return Err(config_field(
                        field("api_key"),
                        "a subscription connection cannot also use an API key",
                    ));
                }
                account
                    .validate_connection(entry.format, &entry.base_url, &entry.options)
                    .map_err(|error| config_field(field("subscription"), error.to_string()))?;
            }
            entry
                .options
                .proxy
                .validate()
                .map_err(|error| config_field(field("proxy"), error.to_string()))?;
            for (model, pricing) in &entry.options.model_pricing {
                if model.trim().is_empty() || !pricing.valid() {
                    let model_key = toml_edit::Key::new(model);
                    return Err(config_field(
                        field(&format!("model_pricing.{model_key}")),
                        "invalid USD pricing",
                    ));
                }
            }
            let api_key = entry.api_key.trim();
            if api_key.is_empty() && entry.options.subscription.is_none() {
                return Err(config_field(field("api_key"), "api_key is required"));
            }
            if api_key == "$" {
                return Err(config_field(
                    field("api_key"),
                    "'$' must be followed by an env var name",
                ));
            }
            if let Some(file_path) = api_key.strip_prefix("file:")
                && !Path::new(file_path).is_absolute()
            {
                return Err(config_field(
                    field("api_key"),
                    "file api_key must use an absolute path",
                ));
            }
        }
        Ok(())
    }

    fn validate_cross_references(&self) -> Result<&ModelRef> {
        let default = self.default_model.as_ref().ok_or_else(|| {
            config_field("default_model", "set both connection and exact model ID")
        })?;
        validate_identity(
            &default.connection,
            "default_model.connection",
            "connection name",
        )?;
        validate_identity(&default.model, "default_model.model", "model ID")?;
        if !self.provider.contains_key(&default.connection) {
            return Err(config_field(
                "default_model.connection",
                format!(
                    "connection {:?} is not defined in [provider]",
                    default.connection,
                ),
            ));
        }
        for (connection, registrations) in &self.model {
            if !self.provider.contains_key(connection) {
                let field = registrations.keys().next().map_or_else(
                    || format!("model.{}", toml_edit::Key::new(connection)),
                    |model| ModelRef::new(connection, model).config_path(),
                );
                return Err(config_field(
                    field,
                    format!("connection {connection:?} is not defined in [provider]"),
                ));
            }
        }
        Ok(default)
    }
}

fn validate_identity(value: &str, field: &str, label: &str) -> Result<()> {
    if value.is_empty() || value.trim() != value {
        return Err(config_field(
            field,
            format!("{label} must be nonempty with no surrounding whitespace"),
        ));
    }
    Ok(())
}

fn config_field(field: impl Into<String>, message: impl Into<String>) -> Error {
    Error::ConfigField {
        field: field.into(),
        message: message.into(),
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
default_model = { connection = "anthropic", model = "fixture-model" }
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

    #[test]
    fn request_fields_keep_header_references_and_reject_adapter_overrides() {
        let raw = r#"
default_model = { connection = "router", model = "anthropic/claude-haiku-5.5" }
[provider.router]
format = "openai-responses"
base_url = "https://openrouter.ai/api/v1"
api_key = "$FIXTURE_KEY"
[provider.router.request.headers]
"X-Tenant" = "$FIXTURE_TENANT"
[provider.router.request.body.provider]
data_collection = "deny"
zdr = true
[model.router."anthropic/claude-haiku-5.5".request.body]
cache_control = { type = "ephemeral", ttl = "1h" }
"#;
        let parse = |raw: &str| {
            parse_config_text_with_env(raw, |_| panic!("config must keep credential references"))
                .unwrap()
                .resolve()
        };
        let config = parse(raw).unwrap();
        let request = config
            .provider("router")
            .unwrap()
            .options
            .request
            .as_ref()
            .unwrap();
        assert_eq!(request.headers["X-Tenant"], "$FIXTURE_TENANT");
        assert_eq!(request.body["provider"]["zdr"], true);
        assert_eq!(
            config
                .model(config.default_model())
                .unwrap()
                .request
                .as_ref()
                .unwrap()
                .body["cache_control"]["ttl"],
            "1h"
        );
        assert!(
            parse(&raw.replace("X-Tenant", "aUtHoRiZaTiOn"))
                .unwrap_err()
                .to_string()
                .contains("managed by the adapter")
        );
        assert!(
            parse(&raw.replace("cache_control =", "messages ="))
                .unwrap_err()
                .to_string()
                .contains("managed by the adapter")
        );
    }
}
