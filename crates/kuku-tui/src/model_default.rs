use std::fs;
use std::path::{Path, PathBuf};

use kuku::config::{parse_config_text, Config};
use kuku_client::{Connection, ConnectionOptions};
use toml_edit::{value, DocumentMut, InlineTable, Item, Table, Value};

const LOCAL_SERVICE_REQUIRED: &str = "Set current model as default requires the active local service; reopen /model on a local connection.";
const LOCAL_SERVICE_CHANGED: &str =
    "The local service or config changed; reopen /model before saving.";
const CONFIG_UNAVAILABLE: &str = "Cannot read the active config; reopen /model and try again.";
const CONFIG_INVALID: &str = "The active config is invalid; no changes were made.";
const CONFIG_CHANGED: &str =
    "The config changed or could not be saved; reopen /model and try again.";
pub(crate) const SAVE_SCOPE: &str = "Applies to new sessions and sessions without a separately chosen model; the current session is unchanged.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedModel {
    pub(crate) connection: String,
    pub(crate) model: String,
}

#[derive(Clone, PartialEq, Eq)]
struct ServiceIdentity {
    endpoint: String,
    instance_id: String,
    pid: u32,
    data_dir: String,
    config_path: PathBuf,
}

pub(crate) struct PreparedDefaultSave {
    route: ResolvedModel,
    tier: String,
    config_path: PathBuf,
    original_bytes: Vec<u8>,
    original_permissions: fs::Permissions,
    updated_bytes: Vec<u8>,
    service: ServiceIdentity,
}

impl PreparedDefaultSave {
    pub(crate) fn route(&self) -> &ResolvedModel {
        &self.route
    }

    pub(crate) fn confirmation_header(&self) -> String {
        format!(
            "Connection/model: {}/{}\nConfig: {}\nDefault tier: {}",
            self.route.connection,
            self.route.model,
            self.config_path.display(),
            self.tier
        )
    }
}

pub(crate) async fn prepare(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
    route: ResolvedModel,
) -> Result<PreparedDefaultSave, String> {
    let service = active_service(options, connection).await?;
    tokio::task::spawn_blocking(move || prepare_file(service, route))
        .await
        .map_err(|_| CONFIG_UNAVAILABLE.to_owned())?
}

fn prepare_file(
    service: ServiceIdentity,
    route: ResolvedModel,
) -> Result<PreparedDefaultSave, String> {
    let metadata = fs::symlink_metadata(&service.config_path).map_err(|_| CONFIG_UNAVAILABLE)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CONFIG_UNAVAILABLE.into());
    }
    let original_bytes = fs::read(&service.config_path).map_err(|_| CONFIG_UNAVAILABLE)?;
    let original_text = std::str::from_utf8(&original_bytes).map_err(|_| CONFIG_INVALID)?;
    let (tier, updated_bytes) = edited_config(original_text, &route)?;
    Ok(PreparedDefaultSave {
        route,
        tier,
        config_path: service.config_path.clone(),
        original_bytes,
        original_permissions: metadata.permissions(),
        updated_bytes,
        service,
    })
}

pub(crate) async fn save(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
    plan: PreparedDefaultSave,
) -> Result<(), String> {
    let current = active_service(options, connection).await?;
    ensure_same_service(&plan.service, &current)?;
    tokio::task::spawn_blocking(move || {
        crate::config_file::publish_config(
            &plan.config_path,
            Some(&plan.original_bytes),
            Some(&plan.original_permissions),
            &plan.updated_bytes,
            || {},
        )
    })
    .await
    .map_err(|_| CONFIG_CHANGED.to_owned())?
    .map_err(|_| CONFIG_CHANGED.to_owned())
}

fn edited_config(raw: &str, route: &ResolvedModel) -> Result<(String, Vec<u8>), String> {
    let config_file = parse_config_text(raw).map_err(|_| CONFIG_INVALID)?;
    let config = config_file.resolve().map_err(|_| CONFIG_INVALID)?;
    let matching_tier = matching_tier(&config, route);
    let has_matching_tier = matching_tier.is_some();
    let tier = matching_tier.unwrap_or_else(|| "user-default".to_owned());
    let mut document = raw.parse::<DocumentMut>().map_err(|_| CONFIG_INVALID)?;

    if !has_matching_tier {
        update_user_default(&mut document, route)?;
    }
    if config.default_tier != tier {
        document["default_model"] = value(tier.as_str());
    }

    let output = document.to_string();
    let validated = parse_config_text(&output).map_err(|_| CONFIG_INVALID)?;
    validated.resolve().map_err(|_| CONFIG_INVALID)?;
    Ok((tier, output.into_bytes()))
}

fn matching_tier(config: &Config, route: &ResolvedModel) -> Option<String> {
    let matches_route = |tier: &kuku::config::TierConfig| {
        tier.provider == route.connection && tier.model == route.model
    };
    if config
        .tiers
        .get(&config.default_tier)
        .is_some_and(matches_route)
    {
        return Some(config.default_tier.clone());
    }
    config
        .tiers
        .iter()
        .find_map(|(name, tier)| matches_route(tier).then(|| name.clone()))
}

fn update_user_default(document: &mut DocumentMut, route: &ResolvedModel) -> Result<(), String> {
    let inline_model = document.get("model").is_some_and(Item::is_inline_table);
    let models = document
        .get_mut("model")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| CONFIG_INVALID.to_owned())?;
    if !models.contains_key("user-default") {
        let entry = if inline_model {
            Item::Value(Value::InlineTable(InlineTable::new()))
        } else {
            Item::Table(Table::new())
        };
        models.insert("user-default", entry);
    }
    let tier = models
        .get_mut("user-default")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| CONFIG_INVALID.to_owned())?;
    tier.clear();
    tier.insert("provider", value(route.connection.as_str()));
    tier.insert("model", value(route.model.as_str()));
    tier.insert("purpose", value("Saved by /model"));
    Ok(())
}

async fn active_service(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
) -> Result<ServiceIdentity, String> {
    let connection = connection.ok_or_else(|| LOCAL_SERVICE_REQUIRED.to_owned())?;
    require_local(
        options.remote.is_some(),
        options.token.is_some(),
        connection.local.is_some(),
    )?;
    let service = options
        .local_service()
        .map_err(|_| LOCAL_SERVICE_REQUIRED.to_owned())?;
    let active = service
        .status()
        .await
        .map_err(|_| LOCAL_SERVICE_CHANGED.to_owned())?
        .ok_or_else(|| LOCAL_SERVICE_CHANGED.to_owned())?;
    if connection.client.endpoint() != active.endpoint() {
        return Err(LOCAL_SERVICE_CHANGED.into());
    }
    let status = active.status();
    let config_path = active_config_path(
        options.config.is_some(),
        service.config_path(),
        status.config_path.as_deref(),
    )?;
    Ok(ServiceIdentity {
        endpoint: active.endpoint().to_owned(),
        instance_id: status
            .instance_id
            .clone()
            .ok_or_else(|| LOCAL_SERVICE_CHANGED.to_owned())?,
        pid: status.pid.ok_or_else(|| LOCAL_SERVICE_CHANGED.to_owned())?,
        data_dir: status
            .data_dir
            .clone()
            .ok_or_else(|| LOCAL_SERVICE_CHANGED.to_owned())?,
        config_path,
    })
}

fn active_config_path(
    explicit_config: bool,
    requested_path: &Path,
    active_path: Option<&str>,
) -> Result<PathBuf, String> {
    let active_path = active_path.ok_or_else(|| LOCAL_SERVICE_CHANGED.to_owned())?;
    let active_path = PathBuf::from(active_path);
    if !active_path.is_absolute() || (explicit_config && requested_path != active_path.as_path()) {
        return Err(LOCAL_SERVICE_CHANGED.into());
    }
    Ok(active_path)
}

fn require_local(remote: bool, token: bool, connected_local: bool) -> Result<(), String> {
    if remote || token || !connected_local {
        Err(LOCAL_SERVICE_REQUIRED.into())
    } else {
        Ok(())
    }
}

fn same_service(expected: &ServiceIdentity, current: &ServiceIdentity) -> bool {
    expected == current
}

fn ensure_same_service(
    expected: &ServiceIdentity,
    current: &ServiceIdentity,
) -> Result<(), String> {
    if same_service(expected, current) {
        Ok(())
    } else {
        Err(LOCAL_SERVICE_CHANGED.into())
    }
}

#[cfg(test)]
pub(crate) fn test_plan(route: ResolvedModel) -> PreparedDefaultSave {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.toml");
    PreparedDefaultSave {
        route,
        tier: "balanced".into(),
        config_path: config_path.clone(),
        original_bytes: Vec::new(),
        original_permissions: fs::metadata("Cargo.toml").unwrap().permissions(),
        updated_bytes: Vec::new(),
        service: ServiceIdentity {
            endpoint: "http://127.0.0.1:1".into(),
            instance_id: "test-instance".into(),
            pid: 1,
            data_dir: std::env::temp_dir().to_string_lossy().into_owned(),
            config_path,
        },
    }
}

#[cfg(test)]
#[path = "model_default_tests.rs"]
mod tests;
