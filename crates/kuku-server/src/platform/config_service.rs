use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kuku::config::{Config, ConfigFile, ProviderFormat, StoredCredential};
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};

use crate::api::{
    ApiError, ApiErrorCode, ApiVersion, CredentialSource, CredentialStatus, PlatformCatalog,
    RevisionToken, TierSummary,
};

use super::persistence::write_private_atomic;
use super::revision::{accepted_digest, RevisionDomain, ServerRevisionCoordinator};
use super::types::{ConfigPatch, ConfigSnapshot, PlatformState};

struct ConfigServiceState {
    raw: Option<ConfigFile>,
    last_good: Option<Arc<Config>>,
    disk_state: PlatformState,
    disk_digest: Option<super::revision::AcceptedDigest>,
}

pub struct ConfigService {
    path: PathBuf,
    revision: Arc<ServerRevisionCoordinator>,
    gate: Arc<RwLock<()>>,
    inner: RwLock<ConfigServiceState>,
}

pub(crate) struct PreparedConfig {
    file: ConfigFile,
    resolved: Option<Config>,
    encoded: Vec<u8>,
    digest: super::AcceptedDigest,
    _gate: OwnedRwLockWriteGuard<()>,
}

impl ConfigService {
    pub async fn open(
        path: PathBuf,
        revision: Arc<ServerRevisionCoordinator>,
    ) -> Result<Arc<Self>, ApiError> {
        let (raw, last_good, disk_state, disk_digest) = read_state(&path);
        let accepted = accepted_state_digest(&raw, &disk_state);
        revision
            .register_initial(RevisionDomain::Config, accepted)
            .await;
        Ok(Arc::new(Self {
            path,
            revision,
            gate: Arc::new(RwLock::new(())),
            inner: RwLock::new(ConfigServiceState {
                raw,
                last_good,
                disk_state,
                disk_digest,
            }),
        }))
    }

    pub async fn state(&self) -> PlatformState {
        self.inner.read().await.disk_state.clone()
    }

    pub async fn revision(&self) -> Result<RevisionToken, ApiError> {
        self.revision.current().await
    }

    pub async fn snapshot(&self) -> Result<ConfigSnapshot, ApiError> {
        let _gate = self.gate.read().await;
        let state = self.inner.read().await;
        Ok(ConfigSnapshot {
            path: self.path.clone(),
            state: state.disk_state.clone(),
            raw: state.raw.clone(),
            resolved: state.last_good.clone(),
        })
    }

    pub async fn catalog(&self) -> PlatformCatalog {
        let _gate = self.gate.read().await;
        let revision = self
            .revision()
            .await
            .unwrap_or_else(|_| RevisionToken::parse("0".repeat(64)).expect("valid zero revision"));
        let state = self.inner.read().await;
        let ready_raw = matches!(state.disk_state, PlatformState::Ready)
            .then_some(state.raw.as_ref())
            .flatten();
        let tiers = if let Some(config) = &state.last_good {
            config
                .tiers
                .iter()
                .map(|(name, tier)| TierSummary {
                    tier_id: name.clone(),
                    label: name.clone(),
                    purpose: tier.purpose.clone(),
                    provider: tier.provider.clone(),
                    model: tier.model.clone(),
                    think: Some(tier.think.as_str().to_owned()),
                    is_default: config.default_tier() == name,
                })
                .collect::<Vec<_>>()
        } else {
            ready_raw
                .map(|config| {
                    config
                        .model
                        .iter()
                        .map(|(name, tier)| TierSummary {
                            tier_id: name.clone(),
                            label: name.clone(),
                            purpose: tier.purpose.clone().unwrap_or_default(),
                            provider: tier.provider.clone(),
                            model: tier.model.clone(),
                            think: tier.think.clone(),
                            is_default: config.default_model.as_ref() == Some(name),
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        let credentials = if let Some(config) = &state.last_good {
            config
                .providers
                .iter()
                .map(|(provider_id, provider)| credential_status(provider_id, &provider.credential))
                .collect()
        } else {
            ready_raw
                .map(|config| {
                    config
                        .provider
                        .iter()
                        .map(|(provider_id, provider)| {
                            credential_status(provider_id, &provider.credential)
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let default_tier = tiers
            .iter()
            .find(|tier| tier.is_default)
            .cloned()
            .unwrap_or_else(|| TierSummary {
                tier_id: String::new(),
                label: String::new(),
                purpose: String::new(),
                provider: String::new(),
                model: String::new(),
                think: None,
                is_default: false,
            });
        PlatformCatalog {
            api_version: ApiVersion,
            revision,
            default_tier,
            tiers,
            credentials,
        }
    }

    pub async fn commit_config(
        &self,
        patch: ConfigPatch,
        expected: RevisionToken,
    ) -> Result<ConfigSnapshot, ApiError> {
        let guard = self.revision.begin(&expected).await?;
        let prepared = self.prepare_config(patch.file).await?;
        self.persist_prepared(&prepared)?;
        let digest = self.install_prepared(&prepared).await;
        guard.finish(RevisionDomain::Config, digest).await?;
        drop(prepared);
        self.snapshot().await
    }

    pub(crate) async fn prepare_config(
        &self,
        file: ConfigFile,
    ) -> Result<PreparedConfig, ApiError> {
        let gate = self.gate.clone().write_owned().await;
        let resolved = resolve_for_state(&file)?;
        let encoded =
            toml::to_string_pretty(&file).map_err(|error| config_error(error.to_string()))?;
        let state = self.inner.read().await;
        if state.disk_digest.is_some() || self.path.exists() {
            let current = std::fs::read(&self.path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    ApiError::new(
                        ApiErrorCode::Outdated,
                        "config changed on disk",
                        "platform-config",
                    )
                } else {
                    io_error(error)
                }
            })?;
            let current_digest = accepted_digest(&current);
            if state.disk_digest.as_ref() != Some(&current_digest) {
                return Err(ApiError::new(
                    ApiErrorCode::Outdated,
                    "config changed on disk",
                    "platform-config",
                ));
            }
        }
        drop(state);
        let digest = canonical_config_digest(&file)?;
        Ok(PreparedConfig {
            file,
            resolved,
            encoded: encoded.into_bytes(),
            digest,
            _gate: gate,
        })
    }

    pub(crate) fn persist_prepared(&self, prepared: &PreparedConfig) -> Result<(), ApiError> {
        write_private_atomic(&self.path, &prepared.encoded).map_err(io_error)
    }

    pub(crate) fn prepared_bytes<'a>(&self, prepared: &'a PreparedConfig) -> &'a [u8] {
        &prepared.encoded
    }

    pub(crate) async fn install_prepared(
        &self,
        prepared: &PreparedConfig,
    ) -> super::AcceptedDigest {
        let mut state = self.inner.write().await;
        state.raw = Some(prepared.file.clone());
        state.last_good = prepared.resolved.clone().map(Arc::new);
        state.disk_state = PlatformState::Ready;
        state.disk_digest = Some(accepted_digest(&prepared.encoded));
        prepared.digest.clone()
    }

    pub async fn reload_from_disk(&self) -> Result<PlatformState, ApiError> {
        self.reload_from_disk_with(|| {}).await
    }

    async fn reload_from_disk_with(
        &self,
        after_read: impl FnOnce(),
    ) -> Result<PlatformState, ApiError> {
        let (raw, last_good, disk_state, disk_digest) = read_state(&self.path);
        after_read();
        let (old_digest, old_state, changed) = {
            let state = self.inner.read().await;
            (
                state.disk_digest.clone(),
                state.disk_state.clone(),
                state.disk_digest != disk_digest || state.disk_state != disk_state,
            )
        };
        if changed && matches!(disk_state, PlatformState::Ready | PlatformState::Missing) {
            let digest = accepted_state_digest(&raw, &disk_state);
            let current = self.revision.current().await?;
            let guard = self.revision.begin(&current).await?;
            let _gate = self.gate.write().await;
            let (_, _, _, latest_digest) = read_state(&self.path);
            if latest_digest != disk_digest {
                return Ok(self.state().await);
            }
            let mut state = self.inner.write().await;
            if state.disk_digest != old_digest || state.disk_state != old_state {
                return Ok(state.disk_state.clone());
            }
            guard.finish(RevisionDomain::Config, digest).await?;
            state.raw = raw;
            state.last_good = last_good;
            state.disk_state = disk_state.clone();
            state.disk_digest = disk_digest;
            return Ok(state.disk_state.clone());
        }
        if matches!(disk_state, PlatformState::Invalid { .. }) {
            let current = self.revision.current().await?;
            let _guard = self.revision.begin(&current).await?;
            let _gate = self.gate.write().await;
            let (_, _, _, latest_digest) = read_state(&self.path);
            if latest_digest != disk_digest {
                return Ok(self.state().await);
            }
            let mut state = self.inner.write().await;
            if state.disk_digest != old_digest || state.disk_state != old_state {
                return Ok(state.disk_state.clone());
            }
            state.disk_state = disk_state.clone();
            state.disk_digest = disk_digest;
            return Ok(state.disk_state.clone());
        }
        Ok(self.state().await)
    }
}

fn credential_status(provider_id: &str, credential: &StoredCredential) -> CredentialStatus {
    let (source, environment_reference) = match credential {
        StoredCredential::DirectValue(_) => (Some(CredentialSource::DirectValue), None),
        StoredCredential::EnvironmentReference(name) => (
            Some(CredentialSource::EnvironmentReference),
            Some(name.clone()),
        ),
    };
    CredentialStatus {
        provider_id: provider_id.to_owned(),
        present: true,
        source,
        environment_reference,
    }
}

fn read_state(
    path: &Path,
) -> (
    Option<ConfigFile>,
    Option<Arc<Config>>,
    PlatformState,
    Option<super::revision::AcceptedDigest>,
) {
    let Ok(bytes) = std::fs::read(path) else {
        return (None, None, PlatformState::Missing, None);
    };
    let digest = accepted_digest(&bytes);
    let raw = match std::str::from_utf8(&bytes)
        .map_err(|error| kuku::Error::ConfigLoad(format!("invalid UTF-8 config: {error}")))
        .and_then(kuku::config::parse_config_file)
    {
        Ok(raw) => raw,
        Err(error) => {
            return (
                None,
                None,
                PlatformState::Invalid {
                    diagnostics: vec![error.to_string()],
                },
                Some(accepted_digest(b"invalid-config")),
            )
        }
    };
    match resolve_for_state(&raw) {
        Ok(config) => (
            Some(raw),
            config.map(Arc::new),
            PlatformState::Ready,
            Some(digest),
        ),
        Err(error) => (
            Some(raw),
            None,
            PlatformState::Invalid {
                diagnostics: vec![error.message],
            },
            Some(accepted_digest(b"invalid-config")),
        ),
    }
}

fn resolve_for_state(raw: &ConfigFile) -> Result<Option<Config>, ApiError> {
    match raw.resolve() {
        Ok(config) => Ok(Some(config)),
        Err(error) if raw.default_model.is_none() && !raw.model.is_empty() => {
            let mut candidate = raw.clone();
            candidate.default_model = candidate.model.keys().next().cloned();
            candidate
                .resolve()
                .map(|_| None)
                .map_err(|_| config_error(error.to_string()))
        }
        Err(error) => Err(config_error(error.to_string())),
    }
}

fn canonical_config_digest(raw: &ConfigFile) -> Result<super::AcceptedDigest, ApiError> {
    let canonical = match raw.resolve() {
        Ok(config) => {
            let tiers = config
                .tiers
                .iter()
                .map(|(id, tier)| {
                    (
                        id,
                        serde_json::json!({
                            "provider": tier.provider,
                            "model": tier.model,
                            "think": tier.think.as_str(),
                            "context_window": tier.context_window,
                            "max_output_tokens": tier.max_output_tokens,
                            "purpose": tier.purpose,
                        }),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            let providers = config
                .providers
                .iter()
                .map(|(id, provider)| {
                    (
                        id,
                        serde_json::json!({
                            "format": provider.format.as_str(),
                            "base_url": provider.base_url,
                            "credential": provider.credential,
                        }),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            serde_json::to_vec(&serde_json::json!({
                "default_tier": config.default_tier,
                "tiers": tiers,
                "providers": providers,
                "discovery": config.discovery,
                "handoff": config.handoff,
                "logs": config.logs,
                "plugin": config.plugin,
                "update": config.update,
            }))
            .map_err(|error| config_error(error.to_string()))?
        }
        Err(_) => toml::to_string(raw)
            .map_err(|error| config_error(error.to_string()))?
            .into_bytes(),
    };
    Ok(accepted_digest(&canonical))
}

fn accepted_state_digest(raw: &Option<ConfigFile>, state: &PlatformState) -> super::AcceptedDigest {
    match (raw, state) {
        (Some(raw), PlatformState::Ready) => canonical_config_digest(raw)
            .expect("a parsed config always has a canonical TOML encoding"),
        (_, PlatformState::Missing) => accepted_digest(b"missing-config"),
        _ => accepted_digest(b"invalid-config"),
    }
}

fn config_error(message: String) -> ApiError {
    ApiError::new(ApiErrorCode::InvalidRequest, message, "platform-config")
}

fn io_error(error: std::io::Error) -> ApiError {
    ApiError::new(ApiErrorCode::Internal, error.to_string(), "platform-config")
}

#[allow(dead_code)]
fn _provider_format_name(format: ProviderFormat) -> &'static str {
    format.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INITIAL: &str = r#"default_model = "initial"
[model.initial]
provider = "local"
model = "model-a"
[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "key" }
"#;

    const COMMITTED: &str = r#"default_model = "committed"
[model.committed]
provider = "local"
model = "model-b"
[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "key" }
"#;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stale_invalid_reload_cannot_overwrite_a_concurrent_commit() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("config.toml");
        std::fs::write(&path, INITIAL).unwrap();
        let revisions = ServerRevisionCoordinator::open(home.path());
        let service = ConfigService::open(path.clone(), revisions).await.unwrap();
        let expected = service.revision().await.unwrap();
        std::fs::write(&path, "not valid [[[\n").unwrap();

        let (read_tx, read_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let reload_service = service.clone();
        let reload = tokio::spawn(async move {
            reload_service
                .reload_from_disk_with(move || {
                    read_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                })
                .await
        });
        tokio::task::spawn_blocking(move || read_rx.recv().unwrap())
            .await
            .unwrap();

        std::fs::write(&path, INITIAL).unwrap();
        let committed = kuku::config::parse_config_file(COMMITTED).unwrap();
        service
            .commit_config(ConfigPatch::replace(committed), expected)
            .await
            .unwrap();
        release_tx.send(()).unwrap();
        reload.await.unwrap().unwrap();

        assert_eq!(service.state().await, PlatformState::Ready);
        assert_eq!(
            service
                .snapshot()
                .await
                .unwrap()
                .resolved
                .unwrap()
                .default_tier(),
            "committed"
        );
    }
}
