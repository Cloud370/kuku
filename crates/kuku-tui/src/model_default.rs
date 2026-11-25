use std::fs;
use std::path::{Path, PathBuf};

use kuku::config::{ModelRef, config_sha256, publish_config};
use kuku_client::local::ServiceIdentity;
use kuku_client::{Connection, ConnectionOptions, ModelDefaultPlan};
use toml_edit::DocumentMut;

use crate::local_settings::{
    LOCAL_SERVICE_REQUIRED, active_service, ensure_same_service, wait_for_config,
};
use crate::setup::{model_entry, set_default_model};

const CONFIG_UNAVAILABLE: &str = "Cannot read the active config; reopen /model and try again.";
const CONFIG_INVALID: &str = "The active config is invalid; no changes were made.";
const ROUTE_UNUSABLE: &str =
    "The active service cannot use this connection and model as a default.";
const CONFIG_CHANGED: &str =
    "The config changed or could not be saved; reopen /model and try again.";
pub(crate) const SAVE_SCOPE: &str = "Sets the starting model for new Sessions.";

pub(crate) struct PreparedDefaultSave {
    route: ModelRef,
    config_path: PathBuf,
    original_bytes: Vec<u8>,
    original_permissions: fs::Permissions,
    updated_bytes: Vec<u8>,
    service: ServiceIdentity,
}

impl PreparedDefaultSave {
    pub(crate) fn route(&self) -> &ModelRef {
        &self.route
    }

    pub(crate) fn confirmation_header(&self) -> String {
        format!(
            "Model: {}\nConnection: {}\nConfig: {}",
            self.route.model,
            self.route.connection,
            self.config_path.display(),
        )
    }
}

pub(crate) async fn prepare(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
    route: ModelRef,
) -> Result<PreparedDefaultSave, String> {
    let service = active_service(options, connection).await?;
    let connection = connection.ok_or_else(|| LOCAL_SERVICE_REQUIRED.to_owned())?;
    let path = service.config_path.clone();
    let (original_bytes, original_permissions) =
        tokio::task::spawn_blocking(move || read_active_config(&path))
            .await
            .map_err(|_| CONFIG_UNAVAILABLE.to_owned())??;
    let config_sha256 = config_sha256(&original_bytes);
    let server_plan = connection
        .client
        .model_default_plan(&route.connection, &route.model, &config_sha256)
        .await
        .map_err(|error| match error.code() {
            Some("config_changed") => CONFIG_CHANGED.to_owned(),
            Some("model_default_unusable") => ROUTE_UNUSABLE.to_owned(),
            _ => CONFIG_INVALID.to_owned(),
        })?;
    let bytes = original_bytes.clone();
    let route_for_edit = route.clone();
    let updated_bytes =
        tokio::task::spawn_blocking(move || edited_config(&bytes, &route_for_edit, &server_plan))
            .await
            .map_err(|_| CONFIG_INVALID.to_owned())??;
    Ok(PreparedDefaultSave {
        route,
        config_path: service.config_path.clone(),
        original_bytes,
        original_permissions,
        updated_bytes,
        service,
    })
}

fn read_active_config(path: &Path) -> Result<(Vec<u8>, fs::Permissions), String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| CONFIG_UNAVAILABLE)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CONFIG_UNAVAILABLE.into());
    }
    let original_bytes = fs::read(path).map_err(|_| CONFIG_UNAVAILABLE)?;
    Ok((original_bytes, metadata.permissions()))
}

pub(crate) async fn save(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
    plan: PreparedDefaultSave,
) -> Result<bool, String> {
    let current = active_service(options, connection).await?;
    ensure_same_service(&plan.service, &current)?;
    let hash = tokio::task::spawn_blocking(move || {
        if plan.updated_bytes == plan.original_bytes {
            if read_active_config(&plan.config_path)
                .map_err(|_| CONFIG_CHANGED)?
                .0
                != plan.original_bytes
            {
                return Err(CONFIG_CHANGED);
            }
        } else {
            publish_config(
                &plan.config_path,
                Some(&plan.original_bytes),
                Some(&plan.original_permissions),
                &plan.updated_bytes,
                || {},
            )
            .map_err(|_| CONFIG_CHANGED)?;
        }
        Ok(config_sha256(&plan.updated_bytes))
    })
    .await
    .map_err(|_| CONFIG_CHANGED.to_owned())?
    .map_err(|_| CONFIG_CHANGED.to_owned())?;
    Ok(wait_for_config(connection, &current, &hash).await)
}

fn edited_config(
    original_bytes: &[u8],
    route: &ModelRef,
    plan: &ModelDefaultPlan,
) -> Result<Vec<u8>, String> {
    if plan.config_sha256 != config_sha256(original_bytes) {
        return Err(CONFIG_CHANGED.into());
    }
    if plan.already_default {
        return Ok(original_bytes.to_vec());
    }
    let raw = std::str::from_utf8(original_bytes).map_err(|_| CONFIG_INVALID)?;
    let mut document = raw.parse::<DocumentMut>().map_err(|_| CONFIG_INVALID)?;
    if let Some(previous) = &plan.register_previous_default {
        model_entry(&mut document, previous).map_err(|_| CONFIG_INVALID)?;
    }
    set_default_model(&mut document, route);
    Ok(document.to_string().into_bytes())
}

#[cfg(test)]
pub(crate) fn test_plan(route: ModelRef) -> PreparedDefaultSave {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.toml");
    let file = fs::File::create(&config_path).unwrap();
    PreparedDefaultSave {
        route,
        config_path: config_path.clone(),
        original_bytes: Vec::new(),
        original_permissions: file.metadata().unwrap().permissions(),
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
