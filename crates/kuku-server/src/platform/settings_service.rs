use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::api::{ApiError, ApiErrorCode, ApiVersion, SettingsSnapshot, UpdateSettingsRequest};

use super::{
    accepted_digest, write_private_atomic, ConfigService, RevisionDomain,
    ServerRevisionCoordinator, WorkspaceRegistry,
};

const SETTINGS_FILE: &str = "settings.json";
const JOURNAL_FILE: &str = "settings.journal.json";
const CONFIG_FILE: &str = "config.toml";
const WORKSPACES_FILE: &str = "workspaces.json";
const STORAGE_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SettingsFile {
    format_version: u8,
    max_concurrent_runs: u8,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsJournal {
    format_version: u8,
    config_toml: Option<String>,
    config_digest: Option<String>,
    workspaces_json: Option<String>,
    workspaces_digest: Option<String>,
    settings_json: String,
    settings_digest: String,
}

pub struct SettingsService {
    home: PathBuf,
    config: Arc<ConfigService>,
    workspaces: Arc<WorkspaceRegistry>,
    revision: Arc<ServerRevisionCoordinator>,
    snapshot_gate: RwLock<()>,
    state: RwLock<SettingsFile>,
}

impl SettingsService {
    pub fn recover(home: &Path) -> Result<(), ApiError> {
        let journal_path = home.join(JOURNAL_FILE);
        if !journal_path.exists() {
            return Ok(());
        }
        let bytes = std::fs::read(&journal_path).map_err(io_error)?;
        let journal: SettingsJournal = serde_json::from_slice(&bytes)
            .map_err(|_| internal_error("settings journal is invalid"))?;
        if journal.format_version != STORAGE_VERSION {
            return Err(internal_error("settings journal has an unsupported format"));
        }
        verify_journal(&journal)?;
        if let Some(config) = journal.config_toml.as_deref() {
            persist_and_verify(
                &home.join(CONFIG_FILE),
                config.as_bytes(),
                journal
                    .config_digest
                    .as_deref()
                    .expect("verified journal config digest is present"),
            )?;
        }
        if let Some(workspaces) = journal.workspaces_json.as_deref() {
            persist_and_verify(
                &home.join(WORKSPACES_FILE),
                workspaces.as_bytes(),
                journal
                    .workspaces_digest
                    .as_deref()
                    .expect("verified journal workspace digest is present"),
            )?;
        }
        persist_and_verify(
            &home.join(SETTINGS_FILE),
            journal.settings_json.as_bytes(),
            &journal.settings_digest,
        )?;
        remove_journal(&journal_path)?;
        Ok(())
    }

    pub async fn open(
        home: &Path,
        config: Arc<ConfigService>,
        workspaces: Arc<WorkspaceRegistry>,
        revision: Arc<ServerRevisionCoordinator>,
        default_max_concurrent_runs: u8,
    ) -> Result<Arc<Self>, ApiError> {
        if !(1..=64).contains(&default_max_concurrent_runs) {
            return Err(invalid_request(
                "max concurrent runs must be between 1 and 64",
            ));
        }
        let path = home.join(SETTINGS_FILE);
        let state = if path.exists() {
            load_settings(&path)?
        } else {
            let state = SettingsFile {
                format_version: STORAGE_VERSION,
                max_concurrent_runs: default_max_concurrent_runs,
            };
            let bytes = encode_settings(&state)?;
            write_private_atomic(&path, &bytes).map_err(io_error)?;
            state
        };
        let bytes = encode_settings(&state)?;
        revision
            .register_initial(RevisionDomain::Settings, accepted_digest(&bytes))
            .await;
        Ok(Arc::new(Self {
            home: home.to_owned(),
            config,
            workspaces,
            revision,
            snapshot_gate: RwLock::new(()),
            state: RwLock::new(state),
        }))
    }

    pub async fn snapshot(&self) -> Result<SettingsSnapshot, ApiError> {
        let _gate = self.snapshot_gate.read().await;
        self.snapshot_unlocked().await
    }

    async fn snapshot_unlocked(&self) -> Result<SettingsSnapshot, ApiError> {
        loop {
            let before = self.revision.current().await?;
            let catalog = self.config.catalog().await;
            let workspaces = self.workspaces.list().await?;
            let max_concurrent_runs = self.state.read().await.max_concurrent_runs;
            let after = self.revision.current().await?;
            if before == after {
                return Ok(SettingsSnapshot {
                    api_version: ApiVersion,
                    server_revision: after,
                    default_tier: catalog.default_tier.tier_id,
                    credentials: catalog.credentials,
                    default_workspace_id: workspaces
                        .items
                        .into_iter()
                        .find(|workspace| workspace.is_default)
                        .map(|workspace| workspace.workspace_id),
                    max_concurrent_runs,
                });
            }
        }
    }

    pub async fn commit(
        &self,
        request: UpdateSettingsRequest,
    ) -> Result<SettingsSnapshot, ApiError> {
        let guard = self.revision.begin(&request.expected_revision).await?;
        let _snapshot_gate = self.snapshot_gate.write().await;
        let prepared_config = if let Some(default_tier) = request.patch.default_tier {
            let mut file = self
                .config
                .snapshot()
                .await?
                .raw
                .ok_or_else(|| invalid_request("providers are not configured"))?;
            if !file.model.contains_key(&default_tier) {
                return Err(invalid_request("default tier is not configured"));
            }
            file.default_model = Some(default_tier);
            Some(self.config.prepare_config(file).await?)
        } else {
            None
        };
        let prepared_workspace = if let Some(workspace_id) = request.patch.default_workspace_id {
            Some(self.workspaces.prepare_default(&workspace_id).await?)
        } else {
            None
        };
        let mut next_settings = self.state.read().await.clone();
        if let Some(max_concurrent_runs) = request.patch.max_concurrent_runs {
            if !(1..=64).contains(&max_concurrent_runs) {
                return Err(invalid_request(
                    "max concurrent runs must be between 1 and 64",
                ));
            }
            next_settings.max_concurrent_runs = max_concurrent_runs;
        }
        let settings_bytes = encode_settings(&next_settings)?;
        let config_bytes = prepared_config
            .as_ref()
            .map(|prepared| self.config.prepared_bytes(prepared))
            .map(std::str::from_utf8)
            .transpose()
            .map_err(|_| internal_error("prepared config is not UTF-8"))?
            .map(str::to_owned);
        let workspace_bytes = prepared_workspace
            .as_ref()
            .map(|prepared| self.workspaces.prepared_default_bytes(prepared))
            .transpose()?;
        let journal = SettingsJournal {
            format_version: STORAGE_VERSION,
            config_digest: config_bytes
                .as_deref()
                .map(|bytes| digest_hex(bytes.as_bytes())),
            config_toml: config_bytes,
            workspaces_digest: workspace_bytes.as_deref().map(digest_hex),
            workspaces_json: workspace_bytes
                .as_ref()
                .map(|bytes| String::from_utf8(bytes.clone()))
                .transpose()
                .map_err(|_| internal_error("prepared workspace state is not UTF-8"))?,
            settings_json: String::from_utf8(settings_bytes.clone())
                .map_err(|_| internal_error("prepared settings state is not UTF-8"))?,
            settings_digest: digest_hex(&settings_bytes),
        };
        let journal_bytes = serde_json::to_vec(&journal)
            .map_err(|_| internal_error("settings journal cannot be encoded"))?;
        let journal_path = self.home.join(JOURNAL_FILE);
        write_private_atomic(&journal_path, &journal_bytes).map_err(io_error)?;

        if let Some(config) = journal.config_toml.as_deref() {
            persist_and_verify(
                &self.home.join(CONFIG_FILE),
                config.as_bytes(),
                journal
                    .config_digest
                    .as_deref()
                    .expect("prepared config digest is present"),
            )?;
        }
        if let Some(workspaces) = journal.workspaces_json.as_deref() {
            persist_and_verify(
                &self.home.join(WORKSPACES_FILE),
                workspaces.as_bytes(),
                journal
                    .workspaces_digest
                    .as_deref()
                    .expect("prepared workspace digest is present"),
            )?;
        }
        persist_and_verify(
            &self.home.join(SETTINGS_FILE),
            &settings_bytes,
            &journal.settings_digest,
        )?;
        remove_journal(&journal_path)?;

        let mut digests = Vec::new();
        if let Some(prepared) = &prepared_config {
            digests.push((
                RevisionDomain::Config,
                self.config.install_prepared(prepared).await,
            ));
        }
        if let Some(prepared) = &prepared_workspace {
            digests.push((
                RevisionDomain::Workspace,
                self.workspaces.install_prepared_default(prepared),
            ));
        }
        let settings_digest = accepted_digest(&settings_bytes);
        *self.state.write().await = next_settings;
        digests.push((RevisionDomain::Settings, settings_digest));
        guard.finish_many(digests).await?;
        drop(prepared_config);
        drop(prepared_workspace);
        drop(_snapshot_gate);
        self.snapshot().await
    }
}

fn load_settings(path: &Path) -> Result<SettingsFile, ApiError> {
    let bytes = std::fs::read(path).map_err(io_error)?;
    let settings: SettingsFile =
        serde_json::from_slice(&bytes).map_err(|_| internal_error("settings state is invalid"))?;
    if settings.format_version != STORAGE_VERSION
        || !(1..=64).contains(&settings.max_concurrent_runs)
    {
        return Err(internal_error("settings state violates its invariants"));
    }
    Ok(settings)
}

fn encode_settings(settings: &SettingsFile) -> Result<Vec<u8>, ApiError> {
    serde_json::to_vec_pretty(settings)
        .map_err(|_| internal_error("settings state cannot be encoded"))
}

fn remove_journal(path: &Path) -> Result<(), ApiError> {
    std::fs::remove_file(path).map_err(io_error)?;
    #[cfg(unix)]
    std::fs::File::open(path.parent().unwrap_or_else(|| Path::new(".")))
        .and_then(|directory| directory.sync_all())
        .map_err(io_error)?;
    Ok(())
}

fn verify_journal(journal: &SettingsJournal) -> Result<(), ApiError> {
    if journal.config_toml.is_some() != journal.config_digest.is_some()
        || journal.workspaces_json.is_some() != journal.workspaces_digest.is_some()
        || journal
            .config_toml
            .as_ref()
            .zip(journal.config_digest.as_ref())
            .is_some_and(|(bytes, digest)| digest_hex(bytes.as_bytes()) != *digest)
        || journal
            .workspaces_json
            .as_ref()
            .zip(journal.workspaces_digest.as_ref())
            .is_some_and(|(bytes, digest)| digest_hex(bytes.as_bytes()) != *digest)
        || digest_hex(journal.settings_json.as_bytes()) != journal.settings_digest
    {
        return Err(internal_error("settings journal digest is invalid"));
    }
    Ok(())
}

fn verify_target(path: &Path, digest: &str) -> Result<(), ApiError> {
    let bytes = std::fs::read(path).map_err(io_error)?;
    if digest_hex(&bytes) != digest {
        return Err(internal_error(
            "settings transaction target digest is invalid",
        ));
    }
    Ok(())
}

fn persist_and_verify(path: &Path, bytes: &[u8], digest: &str) -> Result<(), ApiError> {
    write_private_atomic(path, bytes).map_err(io_error)?;
    verify_target(path, digest)
}

fn digest_hex(bytes: &[u8]) -> String {
    accepted_digest(bytes)
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn invalid_request(message: &'static str) -> ApiError {
    ApiError::new(ApiErrorCode::InvalidRequest, message, "platform-settings")
}

fn internal_error(message: &'static str) -> ApiError {
    ApiError::new(ApiErrorCode::Internal, message, "platform-settings")
}

fn io_error(_error: std::io::Error) -> ApiError {
    internal_error("settings transaction cannot be persisted")
}

#[cfg(test)]
#[path = "settings_service_tests.rs"]
mod tests;
