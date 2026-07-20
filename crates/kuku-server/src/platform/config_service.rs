use std::path::{Path, PathBuf};
use std::sync::Arc;

use kuku::config::{Config, ConfigFile, ProviderFormat, StoredCredential};
use tokio::sync::RwLock;

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
    inner: RwLock<ConfigServiceState>,
}

impl ConfigService {
    pub async fn open(
        path: PathBuf,
        revision: Arc<ServerRevisionCoordinator>,
    ) -> Result<Arc<Self>, ApiError> {
        let (raw, last_good, disk_state, disk_digest) = read_state(&path);
        revision
            .register_initial(
                RevisionDomain::Config,
                disk_digest
                    .clone()
                    .unwrap_or_else(|| accepted_digest(b"missing-config")),
            )
            .await;
        Ok(Arc::new(Self {
            path,
            revision,
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
        let state = self.inner.read().await;
        Ok(ConfigSnapshot {
            path: self.path.clone(),
            state: state.disk_state.clone(),
            raw: state.raw.clone(),
            resolved: state.last_good.clone(),
        })
    }

    pub async fn catalog(&self) -> PlatformCatalog {
        let revision = self
            .revision()
            .await
            .unwrap_or_else(|_| RevisionToken::parse("0".repeat(64)).expect("valid zero revision"));
        let state = self.inner.read().await;
        let tiers = state
            .last_good
            .as_ref()
            .map(|config| {
                config
                    .tiers
                    .iter()
                    .map(|(name, tier)| TierSummary {
                        tier_id: name.clone(),
                        label: name.clone(),
                        purpose: tier.purpose.clone(),
                        provider: tier.provider.clone(),
                        model: tier.model.clone(),
                        think: Some(tier.think.as_str().to_string()),
                        is_default: config.default_tier() == name,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let credentials = state
            .last_good
            .as_ref()
            .map(|config| {
                config
                    .providers
                    .iter()
                    .map(|(provider_id, provider)| {
                        let (source, environment_reference) = match &provider.credential {
                            StoredCredential::DirectValue(_) => {
                                (Some(CredentialSource::DirectValue), None)
                            }
                            StoredCredential::EnvironmentReference(name) => (
                                Some(CredentialSource::EnvironmentReference),
                                Some(name.clone()),
                            ),
                        };
                        CredentialStatus {
                            provider_id: provider_id.clone(),
                            present: true,
                            source,
                            environment_reference,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
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
        let candidate = patch
            .file
            .resolve()
            .map_err(|error| config_error(error.to_string()))?;
        let encoded =
            toml::to_string_pretty(&patch.file).map_err(|error| config_error(error.to_string()))?;
        let mut state = self.inner.write().await;
        if state.disk_digest.is_some() {
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
        write_private_atomic(&self.path, encoded.as_bytes()).map_err(io_error)?;
        let digest = accepted_digest(encoded.as_bytes());
        let _revision = guard.finish(RevisionDomain::Config, digest.clone()).await?;
        state.raw = Some(patch.file);
        state.last_good = Some(Arc::new(candidate));
        state.disk_state = PlatformState::Ready;
        state.disk_digest = Some(digest);
        Ok(ConfigSnapshot {
            path: self.path.clone(),
            state: state.disk_state.clone(),
            raw: state.raw.clone(),
            resolved: state.last_good.clone(),
        })
    }

    pub async fn reload_from_disk(&self) -> Result<PlatformState, ApiError> {
        let (raw, last_good, disk_state, disk_digest) = read_state(&self.path);
        let changed = {
            let state = self.inner.read().await;
            state.disk_digest != disk_digest || state.disk_state != disk_state
        };
        if changed && matches!(disk_state, PlatformState::Ready | PlatformState::Missing) {
            let digest = disk_digest
                .clone()
                .unwrap_or_else(|| accepted_digest(b"missing-config"));
            let current = self.revision.current().await?;
            let guard = self.revision.begin(&current).await?;
            let (_, _, _, latest_digest) = read_state(&self.path);
            if latest_digest != disk_digest {
                return Ok(self.state().await);
            }
            let mut state = self.inner.write().await;
            if state.disk_digest != disk_digest {
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
            let mut state = self.inner.write().await;
            state.disk_state = disk_state.clone();
            return Ok(state.disk_state.clone());
        }
        Ok(self.state().await)
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
    match raw.resolve() {
        Ok(config) => (
            Some(raw),
            Some(Arc::new(config)),
            PlatformState::Ready,
            Some(accepted_digest(b"invalid-config")),
        ),
        Err(error) => (
            Some(raw),
            None,
            PlatformState::Invalid {
                diagnostics: vec![error.to_string()],
            },
            Some(digest),
        ),
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
