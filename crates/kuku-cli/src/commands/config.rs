use std::fs;
use std::io;
use std::path::PathBuf;

use kuku::adapter::{ModelSelection, discover_models, inspect_model, validate_model_config};
use kuku::config::{
    Config, ModelRef, config_sha256, config_text_with_value_in_env, parse_config_text_with_env,
    patch_config_text_with_loaded_default, publish_config,
};
use kuku::session::AgentConfig;
use kuku_client::local::LocalService;
use serde::Serialize;
use serde_json::{Value, json};

use crate::cli_args::{ConfigArgs, ConfigSubcommand, ConnectionArgs};
use crate::commands::CliResult;

#[derive(Serialize)]
struct Diagnostic {
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<String>,
    code: &'static str,
    message: String,
}

struct ConfigError {
    status: &'static str,
    diagnostic: Diagnostic,
}

impl From<kuku::Error> for ConfigError {
    fn from(error: kuku::Error) -> Self {
        let (field, message) = match error {
            kuku::Error::ConfigField { field, message } => (Some(field), message),
            error => (None, error.to_string()),
        };
        Self::new("invalid", "invalid_config", field, message)
    }
}

impl From<io::Error> for ConfigError {
    fn from(error: io::Error) -> Self {
        let (status, code) = match error.kind() {
            io::ErrorKind::AlreadyExists | io::ErrorKind::WouldBlock => {
                ("conflict", "config_conflict")
            }
            io::ErrorKind::NotFound => ("error", "config_not_found"),
            io::ErrorKind::InvalidData => ("invalid", "invalid_config"),
            _ => ("error", "config_io"),
        };
        Self::new(status, code, None, error.to_string())
    }
}

impl ConfigError {
    fn new(
        status: &'static str,
        code: &'static str,
        field: Option<String>,
        message: String,
    ) -> Self {
        Self {
            status,
            diagnostic: Diagnostic {
                field,
                code,
                message,
            },
        }
    }
}

struct ConfigTarget {
    path: PathBuf,
    local_service: Result<LocalService, kuku_client::Error>,
}

impl ConfigTarget {
    async fn resolve(target: &ConnectionArgs) -> Result<Self, Box<dyn std::error::Error>> {
        let local_service = target.local_service();
        let path = if let Some(explicit) = &target.config {
            match explicit.canonicalize() {
                Ok(path) => path,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    std::path::absolute(explicit)?
                }
                Err(error) => return Err(error.into()),
            }
        } else {
            let local = local_service.as_ref().map_err(|error| error.to_string())?;
            local.status().await?.map_or_else(
                || local.config_path().to_owned(),
                |connection| connection.identity().config_path,
            )
        };
        Ok(Self {
            path,
            local_service,
        })
    }

    fn environment(&self, name: &str) -> Result<String, std::env::VarError> {
        if name == "KUKU_HOME"
            && let Ok(service) = &self.local_service
        {
            return service
                .data_dir()
                .as_os_str()
                .to_owned()
                .into_string()
                .map_err(std::env::VarError::NotUnicode);
        }
        std::env::var(name)
    }

    async fn loaded_default(&self, sha256: Option<&str>) -> Option<ModelRef> {
        let sha256 = sha256?;
        let local = self.local_service.as_ref().ok()?;
        let connection = local.status().await.ok()??;
        let status = connection.status();
        if connection.identity().config_path == self.path
            && status.config_sha256.as_deref() == Some(sha256)
        {
            status.config_default_model.clone()
        } else {
            None
        }
    }

    async fn service_state(&self, sha256: Option<&str>) -> Value {
        let local = match &self.local_service {
            Ok(local) => local,
            Err(error) => return json!({"state":"unknown","message":error.to_string()}),
        };
        match local.status().await {
            Ok(Some(connection)) => {
                let status = connection.status();
                let state = match (sha256, status.config_sha256.as_deref()) {
                    _ if connection.identity().config_path != self.path => "different_path",
                    (Some(validated), Some(loaded)) if validated == loaded => "loaded",
                    (Some(_), Some(_)) => "pending_reload",
                    _ => "unknown",
                };
                json!({"state":state,"config_path":status.config_path})
            }
            Ok(None) => json!({"state":"stopped"}),
            Err(error) => json!({"state":"unknown","message":error.to_string()}),
        }
    }
}

/// Resolve a local config path without starting the service, then execute the selected operation.
pub async fn run(args: ConfigArgs, target: &ConnectionArgs) -> CliResult {
    let command = args.cmd.unwrap_or(ConfigSubcommand::Show { json: false });
    let plain_show = matches!(command, ConfigSubcommand::Show { json: false });
    let target = match ConfigTarget::resolve(target).await {
        Ok(target) => target,
        Err(error) => {
            if !plain_show {
                let diagnostics = [Diagnostic {
                    field: None,
                    code: "config_path",
                    message: error.to_string(),
                }];
                let path = target
                    .config
                    .as_ref()
                    .and_then(|explicit| std::path::absolute(explicit).ok())
                    .map(|path| path.to_string_lossy().into_owned());
                print_json(
                    &json!({"path":path,"status":"error","service":{"state":"unknown"},"diagnostics":diagnostics}),
                )?;
            }
            return Err(error);
        }
    };
    let mut sha256 = None;
    let operation = execute(command, &target, &mut sha256).await;
    let result = match operation {
        Ok(result) if plain_show => {
            eprintln!("config {}", target.path.display());
            print!("{}", result["config"].as_str().unwrap_or_default());
            return Ok(());
        }
        result => result,
    };
    let service = target.service_state(sha256.as_deref()).await;
    match result {
        Ok(mut result) => {
            result["path"] = json!(target.path.to_string_lossy());
            result["service"] = service;
            result["diagnostics"] = json!([]);
            print_json(&result)
        }
        Err(error) => {
            let message = if let Some(field) = &error.diagnostic.field {
                format!(
                    "{}: {field}: {}",
                    target.path.display(),
                    error.diagnostic.message
                )
            } else {
                format!("{}: {}", target.path.display(), error.diagnostic.message)
            };
            if !plain_show {
                print_json(&json!({
                    "path":target.path.to_string_lossy(), "status":error.status, "service":service,
                    "diagnostics":[error.diagnostic],
                }))?;
            }
            Err(message.into())
        }
    }
}

async fn execute(
    command: ConfigSubcommand,
    target: &ConfigTarget,
    sha256: &mut Option<String>,
) -> Result<Value, ConfigError> {
    let path = &target.path;
    let environment = |name: &str| target.environment(name);
    let raw = match fs::read_to_string(path) {
        Ok(raw) => Some(raw),
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                && matches!(&command, ConfigSubcommand::Set { patch: Some(_), .. }) =>
        {
            None
        }
        Err(error) => return Err(error.into()),
    };
    *sha256 = raw.as_ref().map(|raw| config_sha256(raw.as_bytes()));
    let text = raw.as_deref().unwrap_or_default();
    match command {
        ConfigSubcommand::Show { .. } => {
            let config = parse_config_text_with_env(text, environment)?.resolve()?;
            Ok(json!({"status":"valid","config":config.redacted_display()}))
        }
        ConfigSubcommand::Models {
            connection: Some(name),
            ..
        } => {
            let providers = parse_config_text_with_env(text, environment)?.resolve_providers()?;
            let provider = providers.get(&name).ok_or_else(|| {
                ConfigError::new(
                    "invalid",
                    "unknown_connection",
                    Some("connection".into()),
                    format!("connection {name:?} is not configured"),
                )
            })?;
            let models = discover_models(provider).await.map_err(|error| {
                ConfigError::new(
                    "error",
                    "discovery_failed",
                    Some("connection".into()),
                    error.to_string(),
                )
            })?;
            Ok(json!({"status":"valid","connection":name,"models":models}))
        }
        ConfigSubcommand::Models { .. } => {
            let config = parse_config_text_with_env(text, environment)?.resolve()?;
            let models = config
                .models
                .keys()
                .map(|pair| {
                    let mut row = json!({"connection":pair.connection,"model":pair.model});
                    match inspect_configured_model(&config, pair) {
                        Ok(selection) => row["selection"] = json!(selection),
                        Err(error) => row["error"] = json!(error.diagnostic),
                    }
                    row
                })
                .collect::<Vec<_>>();
            Ok(json!({"status":"valid","default_model":config.default_model(),"models":models}))
        }
        ConfigSubcommand::Validate => {
            let config = parse_config_text_with_env(text, environment)?.resolve()?;
            validate_models(&config)?;
            Ok(json!({"status":"valid","default_model":config.default_model()}))
        }
        ConfigSubcommand::Set { key, value, patch } => {
            let updated = match (key, value, patch) {
                (_, _, Some(patch)) => {
                    let loaded = target.loaded_default(sha256.as_deref()).await;
                    patch_config_text_with_loaded_default(
                        text,
                        &patch,
                        loaded.as_ref(),
                        environment,
                    )?
                }
                (Some(key), Some(value), None) => {
                    config_text_with_value_in_env(text, &key, &value, environment)?
                }
                _ => {
                    return Err(ConfigError::new(
                        "invalid",
                        "config_arguments",
                        None,
                        "provide a dotted key and value, or a TOML patch".into(),
                    ));
                }
            };
            let config = parse_config_text_with_env(&updated, environment)?.resolve()?;
            validate_models(&config)?;
            publish_config(
                path,
                raw.as_deref().map(str::as_bytes),
                None,
                updated.as_bytes(),
                || {},
            )?;
            *sha256 = Some(config_sha256(updated.as_bytes()));
            Ok(json!({"status":"updated","default_model":config.default_model()}))
        }
    }
}

fn validate_models(config: &Config) -> Result<(), ConfigError> {
    config.models.keys().try_for_each(|pair| {
        validate_model_config(config, pair).map_err(|error| invalid_model_settings(pair, error))
    })
}

fn inspect_configured_model(
    config: &Config,
    pair: &ModelRef,
) -> Result<ModelSelection, ConfigError> {
    inspect_model(
        config,
        &AgentConfig {
            connection: Some(pair.connection.clone()),
            model: Some(pair.model.clone()),
            ..AgentConfig::default()
        },
    )
    .map_err(|error| invalid_model_settings(pair, error))
}

fn invalid_model_settings(pair: &ModelRef, error: kuku::Error) -> ConfigError {
    ConfigError::new(
        "invalid",
        "invalid_model_settings",
        Some(pair.config_path()),
        error.to_string(),
    )
}

fn print_json(value: &Value) -> CliResult {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
