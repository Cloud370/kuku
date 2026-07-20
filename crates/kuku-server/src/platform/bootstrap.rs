use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use kuku::config::{ConfigFile, ModelEntry, ProviderEntry, ProviderFormat, StoredCredential};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::api::{
    ApiError, ApiErrorCode, ApiVersion, CompleteInitRequest, CredentialInput, InitPhase,
    InitStatus, RegisterInitialWorkspaceRequest, RegistrationRootPage, TestProviderRequest,
    TestProviderResult, TierSummary, UpdateDefaultTierRequest, UpdateProvidersRequest,
};

use super::{
    accepted_digest, write_private_atomic, ConfigService, RevisionDomain,
    ServerRevisionCoordinator, WorkspaceRegistry,
};

const IDENTITY_FILE: &str = "platform.json";
const INIT_FILE: &str = "init.json";
const STORAGE_VERSION: u8 = 1;
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerIdentityRecord {
    pub server_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredIdentity {
    format_version: u8,
    server_id: String,
    display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct InitFile {
    format_version: u8,
    probe_input_revision: Option<String>,
    probe_passed: bool,
    complete: bool,
}

impl Default for InitFile {
    fn default() -> Self {
        Self {
            format_version: STORAGE_VERSION,
            probe_input_revision: None,
            probe_passed: false,
            complete: false,
        }
    }
}

pub trait ProviderProbe: Send + Sync {
    fn probe<'a>(
        &'a self,
        tier: &'a TierSummary,
    ) -> Pin<Box<dyn Future<Output = Result<TestProviderResult, ApiError>> + Send + 'a>>;
}

pub struct BootstrapService {
    home: std::path::PathBuf,
    identity: ServerIdentityRecord,
    config: Arc<ConfigService>,
    workspaces: Arc<WorkspaceRegistry>,
    probe: Arc<dyn ProviderProbe>,
    revision: Arc<ServerRevisionCoordinator>,
    init: RwLock<InitFile>,
}

impl BootstrapService {
    pub async fn open(
        home: &Path,
        config: Arc<ConfigService>,
        workspaces: Arc<WorkspaceRegistry>,
        probe: Arc<dyn ProviderProbe>,
        revision: Arc<ServerRevisionCoordinator>,
    ) -> Result<Arc<Self>, ApiError> {
        let identity = load_or_create_identity(home)?;
        workspaces.revision().await?;
        let init_path = home.join(INIT_FILE);
        let (init, digest) = load_init(&init_path)?;
        revision
            .register_initial(
                RevisionDomain::Init,
                digest.unwrap_or_else(|| accepted_digest(b"missing-init")),
            )
            .await;
        Ok(Arc::new(Self {
            home: home.to_owned(),
            identity,
            config,
            workspaces,
            probe,
            revision,
            init: RwLock::new(init),
        }))
    }

    pub async fn status(&self) -> InitStatus {
        loop {
            let before = self
                .revision
                .current()
                .await
                .expect("revision coordinator always returns a canonical token");
            let catalog = self.config.catalog().await;
            let workspace_registered = self
                .workspaces
                .list()
                .await
                .map(|page| !page.items.is_empty())
                .unwrap_or(false);
            let probe_inputs = self.revision.probe_inputs().await.ok();
            let init = self.init.read().await;
            let provider_test_passed = probe_inputs.as_ref().is_some_and(|current| {
                init.probe_passed
                    && init.probe_input_revision.as_deref() == Some(current.token().as_str())
            });
            let providers_configured = !catalog.credentials.is_empty() && !catalog.tiers.is_empty();
            let default_tier_configured = !catalog.default_tier.tier_id.is_empty();
            let complete = init.complete && provider_test_passed;
            let phase = derive_phase(
                providers_configured,
                default_tier_configured,
                workspace_registered,
                provider_test_passed,
                complete,
            );
            let after = self
                .revision
                .current()
                .await
                .expect("revision coordinator always returns a canonical token");
            drop(init);
            if before == after {
                return InitStatus {
                    api_version: ApiVersion,
                    phase,
                    server_revision: after,
                    providers_configured,
                    default_tier_configured,
                    workspace_registered,
                    provider_test_passed,
                    complete,
                };
            }
        }
    }

    pub async fn update_providers(
        &self,
        request: UpdateProvidersRequest,
    ) -> Result<InitStatus, ApiError> {
        let guard = self.revision.begin(&request.expected_revision).await?;
        let mut file = self
            .config
            .snapshot()
            .await?
            .raw
            .unwrap_or_else(empty_config_file);
        let mut provider_ids = BTreeSet::new();
        let mut providers = BTreeMap::new();
        for draft in request.providers {
            if draft.provider_id.trim().is_empty()
                || !provider_ids.insert(draft.provider_id.clone())
            {
                return Err(invalid_request("provider IDs must be nonempty and unique"));
            }
            let format = ProviderFormat::from_str(&draft.format)
                .map_err(|_| invalid_request("provider format is unsupported"))?;
            let credential = match draft.credential {
                CredentialInput::DirectValue(value) => StoredCredential::DirectValue(value),
                CredentialInput::EnvironmentReference(name) => {
                    StoredCredential::EnvironmentReference(name)
                }
            };
            providers.insert(
                draft.provider_id,
                ProviderEntry {
                    format,
                    base_url: draft.base_url,
                    credential,
                },
            );
        }
        let mut tier_ids = BTreeSet::new();
        let mut tiers = BTreeMap::new();
        for draft in request.tiers {
            if draft.tier_id.trim().is_empty() || !tier_ids.insert(draft.tier_id.clone()) {
                return Err(invalid_request("tier IDs must be nonempty and unique"));
            }
            tiers.insert(
                draft.tier_id,
                ModelEntry {
                    provider: draft.provider_id,
                    model: draft.model,
                    think: draft.think,
                    context_window: None,
                    max_output_tokens: None,
                    purpose: Some(draft.purpose),
                },
            );
        }
        if file
            .default_model
            .as_ref()
            .is_some_and(|default| !tiers.contains_key(default))
        {
            file.default_model = None;
        }
        file.provider = providers;
        file.model = tiers;
        let prepared = self.config.prepare_config(file).await?;
        self.config.persist_prepared(&prepared)?;
        let digest = self.config.install_prepared(&prepared).await;
        guard.finish(RevisionDomain::Config, digest).await?;
        drop(prepared);
        Ok(self.status().await)
    }

    pub async fn update_default_tier(
        &self,
        request: UpdateDefaultTierRequest,
    ) -> Result<InitStatus, ApiError> {
        let guard = self.revision.begin(&request.expected_revision).await?;
        let mut file = self
            .config
            .snapshot()
            .await?
            .raw
            .ok_or_else(|| invalid_request("providers must be configured first"))?;
        if !file.model.contains_key(&request.tier_id) {
            return Err(invalid_request("default tier is not configured"));
        }
        file.default_model = Some(request.tier_id);
        let prepared = self.config.prepare_config(file).await?;
        self.config.persist_prepared(&prepared)?;
        let digest = self.config.install_prepared(&prepared).await;
        guard.finish(RevisionDomain::Config, digest).await?;
        drop(prepared);
        Ok(self.status().await)
    }

    pub async fn register_initial_workspace(
        &self,
        request: RegisterInitialWorkspaceRequest,
    ) -> Result<InitStatus, ApiError> {
        self.workspaces.register(request.workspace).await?;
        Ok(self.status().await)
    }

    pub async fn test_provider(
        &self,
        request: TestProviderRequest,
    ) -> Result<TestProviderResult, ApiError> {
        let guard = self.revision.begin(&request.expected_revision).await?;
        let catalog = self.config.catalog().await;
        let tier = catalog
            .tiers
            .into_iter()
            .find(|tier| tier.tier_id == request.tier_id)
            .ok_or_else(|| invalid_request("probe tier is not configured"))?;
        let probe_inputs = self.revision.probe_inputs().await?;
        let result = tokio::time::timeout(PROBE_TIMEOUT, self.probe.probe(&tier))
            .await
            .map_err(|_| provider_unavailable())?
            .map_err(|_| provider_unavailable())?;
        if !result.reachable {
            return Err(provider_unavailable());
        }
        let next = InitFile {
            format_version: STORAGE_VERSION,
            probe_input_revision: Some(probe_inputs.token().as_str().to_owned()),
            probe_passed: true,
            complete: false,
        };
        let (bytes, digest) = encode_init(&next)?;
        write_private_atomic(&self.home.join(INIT_FILE), &bytes).map_err(io_error)?;
        let mut init = self.init.write().await;
        *init = next;
        guard.finish(RevisionDomain::Init, digest).await?;
        drop(init);
        Ok(TestProviderResult {
            message: None,
            ..result
        })
    }

    pub async fn complete(&self, request: CompleteInitRequest) -> Result<InitStatus, ApiError> {
        let guard = self.revision.begin(&request.expected_revision).await?;
        let status = self.status().await;
        if !status.providers_configured
            || !status.default_tier_configured
            || !status.workspace_registered
            || !status.provider_test_passed
        {
            return Err(ApiError::new(
                ApiErrorCode::InitIncomplete,
                "initialization prerequisites are incomplete",
                "platform-init",
            ));
        }
        let probe_inputs = self.revision.probe_inputs().await?;
        let next = InitFile {
            format_version: STORAGE_VERSION,
            probe_input_revision: Some(probe_inputs.token().as_str().to_owned()),
            probe_passed: true,
            complete: true,
        };
        let (bytes, digest) = encode_init(&next)?;
        write_private_atomic(&self.home.join(INIT_FILE), &bytes).map_err(io_error)?;
        let mut init = self.init.write().await;
        *init = next;
        guard.finish(RevisionDomain::Init, digest).await?;
        drop(init);
        Ok(self.status().await)
    }

    pub fn identity(&self) -> ServerIdentityRecord {
        self.identity.clone()
    }

    pub fn registration_roots(&self) -> RegistrationRootPage {
        RegistrationRootPage {
            api_version: ApiVersion,
            items: self.workspaces.registration_roots().list(),
        }
    }
}

pub fn derive_phase(
    providers: bool,
    default_tier: bool,
    workspace: bool,
    probe: bool,
    complete: bool,
) -> InitPhase {
    if !providers {
        InitPhase::Required
    } else if !default_tier {
        InitPhase::ProvidersReady
    } else if !workspace {
        InitPhase::DefaultTierReady
    } else if !probe {
        InitPhase::WorkspaceReady
    } else if !complete {
        InitPhase::ProbePassed
    } else {
        InitPhase::Complete
    }
}

fn load_or_create_identity(home: &Path) -> Result<ServerIdentityRecord, ApiError> {
    let path = home.join(IDENTITY_FILE);
    let stored = if path.exists() {
        let bytes = std::fs::read(&path).map_err(io_error)?;
        let stored: StoredIdentity = serde_json::from_slice(&bytes)
            .map_err(|_| internal_error("platform identity is invalid"))?;
        if stored.format_version != STORAGE_VERSION
            || stored.server_id.trim().is_empty()
            || stored.display_name.trim().is_empty()
        {
            return Err(internal_error("platform identity violates its invariants"));
        }
        stored
    } else {
        let suffix = random_hex(12)?;
        let stored = StoredIdentity {
            format_version: STORAGE_VERSION,
            server_id: format!("srv_{suffix}"),
            display_name: format!("kuku {}", &suffix[..8]),
        };
        let bytes = serde_json::to_vec_pretty(&stored)
            .map_err(|_| internal_error("platform identity cannot be encoded"))?;
        write_private_atomic(&path, &bytes).map_err(io_error)?;
        stored
    };
    Ok(ServerIdentityRecord {
        server_id: stored.server_id,
        display_name: stored.display_name,
    })
}

fn load_init(path: &Path) -> Result<(InitFile, Option<super::AcceptedDigest>), ApiError> {
    if !path.exists() {
        return Ok((InitFile::default(), None));
    }
    let bytes = std::fs::read(path).map_err(io_error)?;
    let init: InitFile =
        serde_json::from_slice(&bytes).map_err(|_| internal_error("init state is invalid"))?;
    if init.format_version != STORAGE_VERSION
        || (init.complete && !init.probe_passed)
        || init
            .probe_input_revision
            .as_ref()
            .is_some_and(|token| crate::api::RevisionToken::parse(token.clone()).is_err())
    {
        return Err(internal_error("init state violates its invariants"));
    }
    Ok((init, Some(accepted_digest(&bytes))))
}

fn encode_init(init: &InitFile) -> Result<(Vec<u8>, super::AcceptedDigest), ApiError> {
    let bytes = serde_json::to_vec_pretty(init)
        .map_err(|_| internal_error("init state cannot be encoded"))?;
    let digest = accepted_digest(&bytes);
    Ok((bytes, digest))
}

fn empty_config_file() -> ConfigFile {
    kuku::config::parse_config_file("").expect("an empty raw config is structurally valid")
}

fn random_hex(bytes: usize) -> Result<String, ApiError> {
    let mut random = vec![0_u8; bytes];
    getrandom::fill(&mut random)
        .map_err(|_| internal_error("platform identity cannot be generated"))?;
    Ok(random.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn invalid_request(message: &'static str) -> ApiError {
    ApiError::new(ApiErrorCode::InvalidRequest, message, "platform-init")
}

fn provider_unavailable() -> ApiError {
    ApiError::new(
        ApiErrorCode::ProviderUnavailable,
        "provider probe failed",
        "platform-init",
    )
}

fn internal_error(message: &'static str) -> ApiError {
    ApiError::new(ApiErrorCode::Internal, message, "platform-init")
}

fn io_error(_error: std::io::Error) -> ApiError {
    internal_error("platform state cannot be persisted")
}
