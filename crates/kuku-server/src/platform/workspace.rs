use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, RwLock as StdRwLock};

use cap_std::ambient_authority;
use cap_std::fs::{Dir, File, OpenOptions, OpenOptionsExt, ReadDir};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tokio::sync::{OnceCell, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};
use typed_path::{Utf8Component, Utf8UnixPath, Utf8WindowsPath};

use crate::api::{
    ApiError, ApiErrorCode, ApiVersion, RegisterWorkspaceRequest, RegistrationRootId,
    RegistrationRootSummary, RemoveWorkspaceRequest, RevisionToken, WorkspaceAvailability,
    WorkspaceId, WorkspacePage, WorkspaceSummary,
};

use super::{accepted_digest, write_private_atomic, RevisionDomain, ServerRevisionCoordinator};

#[path = "workspace_process.rs"]
mod process;
use process::{FileIdentity, IdentityBoundProcessRoot};
pub use process::{
    ProcessChunk, ProcessChunkSink, ProcessLimits, ProcessOutput, ProcessStatus, ProcessStream,
    RootCommand,
};

const ROOTS_FILE: &str = "registration-roots.json";
const WORKSPACES_FILE: &str = "workspaces.json";
const STORAGE_VERSION: u8 = 1;

/// Describes one operator-controlled registration root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationRootSpec {
    /// The nonsecret label shown to clients.
    pub label: String,
    /// The operator-controlled host directory.
    pub path: PathBuf,
}

/// Holds an opened registration root without exposing its ambient path.
#[derive(Clone)]
pub struct RegistrationRootCapability {
    /// The opaque ID assigned to this root.
    pub registration_root_id: RegistrationRootId,
    /// The nonsecret label shown to clients.
    pub label: String,
    root: Arc<Dir>,
    process_path: Arc<PathBuf>,
}

impl std::fmt::Debug for RegistrationRootCapability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RegistrationRootCapability")
            .field("registration_root_id", &self.registration_root_id)
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

/// Owns the current operator allowlist and stable opaque root IDs.
pub struct RegistrationRootRegistry {
    active: Vec<RegistrationRootCapability>,
    digest_material: Vec<u8>,
}

impl RegistrationRootRegistry {
    /// Opens configured roots and persists stable IDs in the private server home.
    pub fn from_server_config(
        kuku_home: &Path,
        specs: Vec<RegistrationRootSpec>,
    ) -> Result<Arc<Self>, ApiError> {
        let path = kuku_home.join(ROOTS_FILE);
        let mut stored = load_root_file(&path)?;
        let mut active = Vec::with_capacity(specs.len());
        let mut active_ids = Vec::with_capacity(specs.len());
        let mut canonical_roots = Vec::with_capacity(specs.len());

        for spec in specs {
            if spec.label.trim().is_empty() {
                return Err(invalid_request("registration root label must not be empty"));
            }
            let canonical = std::fs::canonicalize(&spec.path)
                .map_err(|_| invalid_request("registration root is unavailable"))?;
            if !canonical.is_dir() || canonical_roots.contains(&canonical) {
                return Err(invalid_request(
                    "registration roots must be distinct directories",
                ));
            }
            canonical_roots.push(canonical.clone());

            let root_id = match stored
                .mappings
                .iter()
                .find(|mapping| mapping.canonical_path == canonical)
            {
                Some(mapping) => mapping.root_id.clone(),
                None => {
                    let root_id = generate_root_id()?;
                    stored.mappings.push(StoredRootMapping {
                        canonical_path: canonical.clone(),
                        root_id: root_id.clone(),
                    });
                    root_id
                }
            };
            let root = Dir::open_ambient_dir(&canonical, ambient_authority())
                .map_err(|_| invalid_request("registration root cannot be opened"))?;
            active_ids.push(root_id.clone());
            active.push(RegistrationRootCapability {
                registration_root_id: root_id,
                label: spec.label,
                process_path: Arc::new(canonical.clone()),
                root: Arc::new(root),
            });
        }

        stored.active_root_ids = active_ids;
        let bytes = serde_json::to_vec_pretty(&stored)
            .map_err(|_| internal_error("registration root state cannot be encoded"))?;
        write_private_atomic(&path, &bytes)
            .map_err(|_| internal_error("registration root state cannot be persisted"))?;

        Ok(Arc::new(Self {
            active,
            digest_material: bytes,
        }))
    }

    /// Lists only opaque IDs and configured labels.
    pub fn list(&self) -> Vec<RegistrationRootSummary> {
        self.active
            .iter()
            .map(|root| RegistrationRootSummary {
                root_id: root.registration_root_id.clone(),
                label: root.label.clone(),
            })
            .collect()
    }

    /// Resolves an active root ID to its opened capability.
    pub fn resolve(&self, id: &RegistrationRootId) -> Result<RegistrationRootCapability, ApiError> {
        self.active
            .iter()
            .find(|root| root.registration_root_id == *id)
            .cloned()
            .ok_or_else(|| invalid_request("registration root is not active"))
    }
}

/// Reports whether Runtime has durable references to a workspace.
pub trait WorkspaceUsagePort: Send + Sync {
    /// Returns true when removing the workspace would orphan durable Tasks.
    fn has_durable_tasks<'a>(
        &'a self,
        id: &'a WorkspaceId,
    ) -> Pin<Box<dyn Future<Output = Result<bool, ApiError>> + Send + 'a>>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WorkspaceRecord {
    workspace_id: WorkspaceId,
    label: String,
    registration_root_id: RegistrationRootId,
    relative_path: NormalizedRelativePath,
}

/// Provides handle-relative access to one registered workspace.
#[derive(Clone)]
pub struct WorkspaceCapability {
    workspace_id: WorkspaceId,
    root: Arc<Dir>,
    process_root: IdentityBoundProcessRoot,
}

impl std::fmt::Debug for WorkspaceCapability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceCapability")
            .field("workspace_id", &self.workspace_id)
            .finish_non_exhaustive()
    }
}

impl WorkspaceCapability {
    /// Returns the opaque workspace ID associated with this capability.
    pub fn workspace_id(&self) -> &WorkspaceId {
        &self.workspace_id
    }

    /// Validates a portable workspace-relative POSIX path.
    pub fn resolve(&self, relative: &str) -> Result<NormalizedRelativePath, ApiError> {
        NormalizedRelativePath::parse(relative)
    }

    /// Opens a regular file beneath the capability without following symlinks.
    pub fn open_file(&self, path: &NormalizedRelativePath) -> Result<File, ApiError> {
        let components: Vec<_> = path.as_path().components().collect();
        let (last, parents) = components
            .split_last()
            .ok_or_else(|| invalid_request("workspace path must not be empty"))?;
        let parent = open_directory_components(&self.root, parents)?;
        let std::path::Component::Normal(segment) = last else {
            return Err(invalid_request("workspace path is not normalized"));
        };
        let metadata = parent
            .symlink_metadata(segment)
            .map_err(|_| unavailable("workspace path is unavailable"))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(unavailable("workspace path is not a regular file"));
        }
        let file = open_file_no_follow(&parent, segment)
            .map_err(|_| unavailable("workspace file is unavailable"))?;
        let opened = file
            .metadata()
            .map_err(|_| unavailable("workspace file is unavailable"))?;
        if FileIdentity::from_metadata(&metadata)? != FileIdentity::from_metadata(&opened)? {
            return Err(unavailable("workspace file identity changed"));
        }
        Ok(file)
    }

    /// Reads a directory beneath the capability without following symlinks.
    pub fn read_dir(&self, path: &NormalizedRelativePath) -> Result<ReadDir, ApiError> {
        open_directory_relative(&self.root, path)?
            .read_dir(".")
            .map_err(|_| unavailable("workspace directory entries are unavailable"))
    }

    /// Opens a directory beneath the capability without following symlinks.
    pub fn open_dir(&self, path: &NormalizedRelativePath) -> Result<Dir, ApiError> {
        open_directory_relative(&self.root, path)
    }

    /// Runs a bounded shell-free command at the identity-bound workspace root.
    pub async fn run_at_root(
        &self,
        command: RootCommand,
        limits: ProcessLimits,
    ) -> Result<ProcessOutput, ApiError> {
        self.process_root.run(command, limits).await
    }

    /// Streams a bounded shell-free command at the identity-bound workspace root.
    pub async fn stream_at_root(
        &self,
        command: RootCommand,
        limits: ProcessLimits,
        sink: &mut dyn ProcessChunkSink,
    ) -> Result<ProcessStatus, ApiError> {
        self.process_root.stream(command, limits, sink).await
    }

    /// Verifies a command's reported root resolves to this same filesystem identity.
    pub fn reported_root_is_self(&self, output: &ProcessOutput) -> bool {
        self.process_root.reported_root_is_self(output)
    }

    async fn git_branch(&self) -> Option<String> {
        let limits = ProcessLimits::new(std::time::Duration::from_secs(1), 4096).ok()?;
        let root = self
            .run_at_root(
                RootCommand::new("git").args(["rev-parse", "--show-toplevel"]),
                limits.clone(),
            )
            .await
            .ok()?;
        if !root.status().success() || !self.reported_root_is_self(&root) {
            return None;
        }
        let branch = self
            .run_at_root(
                RootCommand::new("git").args(["symbolic-ref", "--short", "HEAD"]),
                limits,
            )
            .await
            .ok()?;
        if !branch.status().success() {
            return None;
        }
        let value = std::str::from_utf8(branch.stdout())
            .ok()?
            .trim_end_matches(['\r', '\n']);
        if value.is_empty()
            || value.len() > 256
            || value.bytes().any(|byte| byte.is_ascii_control())
        {
            return None;
        }
        Some(value.to_owned())
    }
}

/// A validated path relative to a workspace capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedRelativePath(PathBuf);

impl NormalizedRelativePath {
    /// Parses a canonical nonempty POSIX relative path.
    pub fn parse(value: &str) -> Result<Self, ApiError> {
        let unix = Utf8UnixPath::new(value);
        let windows = Utf8WindowsPath::new(value);
        if value.is_empty()
            || value.contains('\\')
            || unix.is_absolute()
            || unix.normalize().as_str() != value
            || windows.is_absolute()
            || windows.components().has_prefix()
            || !unix.components().all(|component| component.is_normal())
            || !windows.components().all(|component| component.is_normal())
        {
            return Err(invalid_request(
                "workspace path must be a normalized relative POSIX path",
            ));
        }
        Ok(Self(PathBuf::from(value)))
    }

    /// Returns the normalized path for capability-relative operations.
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl Serialize for NormalizedRelativePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(
            self.0
                .to_str()
                .expect("normalized workspace paths are always valid UTF-8"),
        )
    }
}

impl<'de> Deserialize<'de> for NormalizedRelativePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value)
            .map_err(|_| serde::de::Error::custom("invalid normalized workspace path"))
    }
}

/// Holds a workspace capability and the shared side of the removal gate.
pub struct WorkspaceTaskLease {
    /// The capability Runtime may use while creating a durable Task.
    pub capability: WorkspaceCapability,
    _gate: OwnedRwLockReadGuard<()>,
}

/// Holds a validated workspace default mutation while the registry write gate is held.
pub(crate) struct PreparedWorkspaceDefault {
    next: WorkspaceFile,
    _gate: OwnedRwLockWriteGuard<()>,
}

/// Owns private workspace records and their mutation ordering.
pub struct WorkspaceRegistry {
    home: PathBuf,
    roots: Arc<RegistrationRootRegistry>,
    usage: Arc<dyn WorkspaceUsagePort>,
    revision: Arc<ServerRevisionCoordinator>,
    revision_registered: OnceCell<()>,
    gate: Arc<RwLock<()>>,
    state: StdRwLock<WorkspaceFile>,
}

impl WorkspaceRegistry {
    /// Opens the private registry without broadening the active root allowlist.
    pub fn open(
        kuku_home: &Path,
        roots: Arc<RegistrationRootRegistry>,
        usage: Arc<dyn WorkspaceUsagePort>,
        revision: Arc<ServerRevisionCoordinator>,
    ) -> Result<Arc<Self>, ApiError> {
        let state = load_workspace_file(&kuku_home.join(WORKSPACES_FILE))?;
        validate_workspace_file(&state)?;
        Ok(Arc::new(Self {
            home: kuku_home.to_owned(),
            roots,
            usage,
            revision,
            revision_registered: OnceCell::new(),
            gate: Arc::new(RwLock::new(())),
            state: StdRwLock::new(state),
        }))
    }

    /// Returns the current canonical server revision.
    pub async fn revision(&self) -> Result<RevisionToken, ApiError> {
        self.ensure_revision_registered().await;
        self.revision.current().await
    }

    /// Lists all persisted workspaces without exposing ambient paths.
    pub async fn list(&self) -> Result<WorkspacePage, ApiError> {
        self.ensure_revision_registered().await;
        let _registry_gate = self.gate.read().await;
        let (records, default_workspace_id) = {
            let state = self
                .state
                .read()
                .expect("workspace state lock is not poisoned");
            (state.records.clone(), state.default_workspace_id.clone())
        };
        let mut items = Vec::with_capacity(records.len());
        for record in &records {
            items.push(self.summary(record, &default_workspace_id).await);
        }
        Ok(WorkspacePage {
            api_version: ApiVersion,
            server_revision: self.revision.current().await?,
            items,
        })
    }

    /// Returns the current operator-controlled registration roots.
    pub fn registration_roots(&self) -> &RegistrationRootRegistry {
        &self.roots
    }

    /// Registers one existing directory beneath an active root.
    pub async fn register(
        self: &Arc<Self>,
        request: RegisterWorkspaceRequest,
    ) -> Result<WorkspaceSummary, ApiError> {
        self.ensure_revision_registered().await;
        let guard = self.revision.begin(&request.expected_revision).await?;
        if request.label.trim().is_empty() {
            return Err(invalid_request("workspace label must not be empty"));
        }
        let relative_path = NormalizedRelativePath::parse(&request.relative_path)?;
        let _registry_gate = self.gate.write().await;
        let root = self.roots.resolve(&request.root_id)?;
        let opened = open_workspace_root(&root.root, &relative_path)?;
        let candidate_identity = FileIdentity::from_metadata(
            &opened
                .dir_metadata()
                .map_err(|_| unavailable("workspace identity cannot be read"))?,
        )?;

        let (record, default_workspace_id, digest) = {
            let mut state = self
                .state
                .write()
                .expect("workspace state lock is not poisoned");
            for record in &state.records {
                if record.registration_root_id == request.root_id
                    && record.relative_path == relative_path
                {
                    return Err(invalid_request("workspace is already registered"));
                }
                if let Ok(existing) = self.capability_for(record) {
                    if existing.process_root.identity() == candidate_identity {
                        return Err(invalid_request("workspace is already registered"));
                    }
                }
            }
            let workspace_id = WorkspaceId::try_new()
                .map_err(|_| internal_error("workspace ID cannot be generated"))?;
            let first = state.records.is_empty();
            let record = WorkspaceRecord {
                workspace_id: workspace_id.clone(),
                label: request.label,
                registration_root_id: request.root_id,
                relative_path,
            };
            let mut next = state.clone();
            next.records.push(record.clone());
            if first {
                next.default_workspace_id = Some(workspace_id);
            }
            self.persist(&next)?;
            let digest = self.digest_state(&next);
            let default_workspace_id = next.default_workspace_id.clone();
            *state = next;
            (record, default_workspace_id, digest)
        };
        drop(_registry_gate);
        guard.finish(RevisionDomain::Workspace, digest).await?;
        let summary = self.summary(&record, &default_workspace_id).await;
        Ok(summary)
    }

    /// Resolves a persisted workspace into a fresh capability.
    pub fn capability(&self, id: &WorkspaceId) -> Result<WorkspaceCapability, ApiError> {
        let state = self
            .state
            .read()
            .expect("workspace state lock is not poisoned");
        let record = state
            .records
            .iter()
            .find(|record| record.workspace_id == *id)
            .ok_or_else(not_found)?
            .clone();
        self.capability_for(&record)
    }

    /// Acquires a capability and prevents removal until the lease is dropped.
    pub async fn lease_for_task(
        self: &Arc<Self>,
        id: &WorkspaceId,
    ) -> Result<WorkspaceTaskLease, ApiError> {
        let gate = self.gate.clone().read_owned().await;
        let capability = self.capability(id)?;
        Ok(WorkspaceTaskLease {
            capability,
            _gate: gate,
        })
    }

    /// Changes the default workspace under the shared revision transaction.
    pub async fn set_default(
        self: &Arc<Self>,
        id: &WorkspaceId,
        expected: RevisionToken,
    ) -> Result<WorkspacePage, ApiError> {
        self.ensure_revision_registered().await;
        let guard = self.revision.begin(&expected).await?;
        let prepared = self.prepare_default(id).await?;
        let digest = self.apply_prepared_default(prepared)?;
        guard.finish(RevisionDomain::Workspace, digest).await?;
        self.list().await
    }

    pub(crate) async fn prepare_default(
        self: &Arc<Self>,
        id: &WorkspaceId,
    ) -> Result<PreparedWorkspaceDefault, ApiError> {
        let gate = self.gate.clone().write_owned().await;
        let state = self
            .state
            .read()
            .expect("workspace state lock is not poisoned");
        if !state
            .records
            .iter()
            .any(|record| record.workspace_id == *id)
        {
            return Err(not_found());
        }
        let mut next = state.clone();
        next.default_workspace_id = Some(id.clone());
        Ok(PreparedWorkspaceDefault { next, _gate: gate })
    }

    pub(crate) fn apply_prepared_default(
        &self,
        prepared: PreparedWorkspaceDefault,
    ) -> Result<super::AcceptedDigest, ApiError> {
        self.persist(&prepared.next)?;
        let digest = self.digest_state(&prepared.next);
        let mut state = self
            .state
            .write()
            .expect("workspace state lock is not poisoned");
        *state = prepared.next;
        drop(state);
        Ok(digest)
    }

    /// Removes only the registry record after checking durable Task usage.
    pub async fn remove(
        self: &Arc<Self>,
        id: &WorkspaceId,
        request: RemoveWorkspaceRequest,
    ) -> Result<(), ApiError> {
        self.ensure_revision_registered().await;
        let guard = self.revision.begin(&request.expected_revision).await?;
        let _registry_gate = self.gate.write().await;
        if self.usage.has_durable_tasks(id).await? {
            return Err(ApiError::new(
                ApiErrorCode::WorkspaceInUse,
                "workspace has durable tasks",
                "platform-workspace",
            ));
        }
        let digest = {
            let mut state = self
                .state
                .write()
                .expect("workspace state lock is not poisoned");
            let position = state
                .records
                .iter()
                .position(|record| record.workspace_id == *id)
                .ok_or_else(not_found)?;
            let mut next = state.clone();
            next.records.remove(position);
            if next.default_workspace_id.as_ref() == Some(id) {
                next.default_workspace_id = next
                    .records
                    .first()
                    .map(|record| record.workspace_id.clone());
            }
            self.persist(&next)?;
            let digest = self.digest_state(&next);
            *state = next;
            digest
        };
        guard.finish(RevisionDomain::Workspace, digest).await?;
        Ok(())
    }

    async fn ensure_revision_registered(&self) {
        self.revision_registered
            .get_or_init(|| async {
                let digest = {
                    let state = self
                        .state
                        .read()
                        .expect("workspace state lock is not poisoned");
                    self.digest_state(&state)
                };
                self.revision
                    .register_initial(RevisionDomain::Workspace, digest)
                    .await;
            })
            .await;
    }

    fn capability_for(&self, record: &WorkspaceRecord) -> Result<WorkspaceCapability, ApiError> {
        let root = self
            .roots
            .resolve(&record.registration_root_id)
            .map_err(|_| unavailable("workspace registration root is unavailable"))?;
        let opened = open_workspace_root(&root.root, &record.relative_path)
            .map_err(|_| unavailable("workspace directory is unavailable"))?;
        Ok(WorkspaceCapability {
            workspace_id: record.workspace_id.clone(),
            process_root: IdentityBoundProcessRoot::new(
                Arc::new(
                    opened
                        .try_clone()
                        .map_err(|_| unavailable("workspace identity cannot be cloned"))?,
                ),
                root.process_path.join(&record.relative_path.0),
            )?,
            root: Arc::new(opened),
        })
    }

    async fn summary(
        &self,
        record: &WorkspaceRecord,
        default: &Option<WorkspaceId>,
    ) -> WorkspaceSummary {
        let (availability, branch) = match self.capability_for(record) {
            Ok(capability) => (
                WorkspaceAvailability::Available,
                capability.git_branch().await,
            ),
            Err(error) if error.code() == ApiErrorCode::WorkspaceNotFound => {
                (WorkspaceAvailability::Missing, None)
            }
            Err(_) => (WorkspaceAvailability::Inaccessible, None),
        };
        WorkspaceSummary {
            workspace_id: record.workspace_id.clone(),
            label: record.label.clone(),
            is_default: default.as_ref() == Some(&record.workspace_id),
            availability,
            branch,
        }
    }

    fn persist(&self, state: &WorkspaceFile) -> Result<(), ApiError> {
        let bytes = serde_json::to_vec_pretty(state)
            .map_err(|_| internal_error("workspace registry cannot be encoded"))?;
        write_private_atomic(&self.home.join(WORKSPACES_FILE), &bytes)
            .map_err(|_| internal_error("workspace registry cannot be persisted"))?;
        Ok(())
    }

    fn digest_state(&self, state: &WorkspaceFile) -> super::AcceptedDigest {
        let workspace_bytes = serde_json::to_vec(state)
            .expect("validated workspace state always has a canonical JSON encoding");
        let mut material =
            Vec::with_capacity(self.roots.digest_material.len() + workspace_bytes.len() + 16);
        material.extend_from_slice(&(self.roots.digest_material.len() as u64).to_le_bytes());
        material.extend_from_slice(&self.roots.digest_material);
        material.extend_from_slice(&(workspace_bytes.len() as u64).to_le_bytes());
        material.extend_from_slice(&workspace_bytes);
        accepted_digest(&material)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredRootMapping {
    canonical_path: PathBuf,
    root_id: RegistrationRootId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RegistrationRootFile {
    format_version: u8,
    mappings: Vec<StoredRootMapping>,
    active_root_ids: Vec<RegistrationRootId>,
}

impl Default for RegistrationRootFile {
    fn default() -> Self {
        Self {
            format_version: STORAGE_VERSION,
            mappings: Vec::new(),
            active_root_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WorkspaceFile {
    format_version: u8,
    default_workspace_id: Option<WorkspaceId>,
    records: Vec<WorkspaceRecord>,
}

impl Default for WorkspaceFile {
    fn default() -> Self {
        Self {
            format_version: STORAGE_VERSION,
            default_workspace_id: None,
            records: Vec::new(),
        }
    }
}

fn load_root_file(path: &Path) -> Result<RegistrationRootFile, ApiError> {
    if !path.exists() {
        return Ok(RegistrationRootFile::default());
    }
    let bytes = std::fs::read(path)
        .map_err(|_| internal_error("registration root state cannot be read"))?;
    let stored: RegistrationRootFile = serde_json::from_slice(&bytes)
        .map_err(|_| internal_error("registration root state is invalid"))?;
    if stored.format_version != STORAGE_VERSION {
        return Err(internal_error(
            "registration root state has an unsupported format",
        ));
    }
    for (index, mapping) in stored.mappings.iter().enumerate() {
        if !mapping.canonical_path.is_absolute()
            || stored.mappings[..index].iter().any(|prior| {
                prior.canonical_path == mapping.canonical_path || prior.root_id == mapping.root_id
            })
        {
            return Err(internal_error(
                "registration root state violates its invariants",
            ));
        }
    }
    Ok(stored)
}

fn load_workspace_file(path: &Path) -> Result<WorkspaceFile, ApiError> {
    if !path.exists() {
        return Ok(WorkspaceFile::default());
    }
    let bytes =
        std::fs::read(path).map_err(|_| internal_error("workspace registry cannot be read"))?;
    let state: WorkspaceFile = serde_json::from_slice(&bytes)
        .map_err(|_| internal_error("workspace registry is invalid"))?;
    if state.format_version != STORAGE_VERSION {
        return Err(internal_error(
            "workspace registry has an unsupported format",
        ));
    }
    Ok(state)
}

fn validate_workspace_file(state: &WorkspaceFile) -> Result<(), ApiError> {
    for (index, record) in state.records.iter().enumerate() {
        if record.label.trim().is_empty()
            || state.records[..index]
                .iter()
                .any(|prior| prior.workspace_id == record.workspace_id)
            || state.records[..index].iter().any(|prior| {
                prior.registration_root_id == record.registration_root_id
                    && prior.relative_path == record.relative_path
            })
        {
            return Err(internal_error("workspace registry violates its invariants"));
        }
    }
    if let Some(default) = &state.default_workspace_id {
        if !state
            .records
            .iter()
            .any(|record| record.workspace_id == *default)
        {
            return Err(internal_error("workspace default is not registered"));
        }
    } else if !state.records.is_empty() {
        return Err(internal_error("workspace registry has no default"));
    }
    Ok(())
}

fn open_workspace_root(root: &Dir, relative: &NormalizedRelativePath) -> Result<Dir, ApiError> {
    open_directory_relative(root, relative)
}

fn open_directory_relative(root: &Dir, relative: &NormalizedRelativePath) -> Result<Dir, ApiError> {
    let components: Vec<_> = relative.as_path().components().collect();
    open_directory_components(root, &components)
}

fn open_directory_components(
    root: &Dir,
    components: &[std::path::Component<'_>],
) -> Result<Dir, ApiError> {
    let mut current = root
        .try_clone()
        .map_err(|_| unavailable("workspace root is unavailable"))?;
    for component in components {
        let std::path::Component::Normal(segment) = component else {
            return Err(invalid_request("workspace path is not normalized"));
        };
        let metadata = current.symlink_metadata(segment).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                not_found()
            } else {
                unavailable("workspace directory is unavailable")
            }
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(invalid_request(
                "workspace path must name a real directory without symlinks",
            ));
        }
        let opened = open_directory_no_follow(&current, segment)
            .map_err(|_| unavailable("workspace directory is unavailable"))?;
        let opened_metadata = opened
            .dir_metadata()
            .map_err(|_| unavailable("workspace directory is unavailable"))?;
        if FileIdentity::from_metadata(&metadata)? != FileIdentity::from_metadata(&opened_metadata)?
        {
            return Err(unavailable("workspace directory identity changed"));
        }
        current = opened;
    }
    Ok(current)
}

fn open_file_no_follow(parent: &Dir, segment: &std::ffi::OsStr) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    #[cfg(windows)]
    options.custom_flags(0x0020_0000);
    parent.open_with(segment, &options)
}

fn open_directory_no_follow(parent: &Dir, segment: &std::ffi::OsStr) -> std::io::Result<Dir> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options
        .custom_flags((rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::NOFOLLOW).bits() as i32);
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

        options
            .custom_flags(0x0220_0000)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    let file = parent.open_with(segment, &options)?;
    Ok(Dir::from_std_file(file.into_std()))
}

fn generate_root_id() -> Result<RegistrationRootId, ApiError> {
    let mut bytes = [0_u8; 12];
    getrandom::fill(&mut bytes)
        .map_err(|_| internal_error("registration root ID cannot be generated"))?;
    let suffix = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    RegistrationRootId::try_new(format!("root_{suffix}"))
        .map_err(|_| internal_error("registration root ID cannot be generated"))
}

fn invalid_request(message: &'static str) -> ApiError {
    ApiError::new(ApiErrorCode::InvalidRequest, message, "platform-workspace")
}

fn not_found() -> ApiError {
    ApiError::new(
        ApiErrorCode::WorkspaceNotFound,
        "workspace not found",
        "platform-workspace",
    )
}

fn unavailable(message: &'static str) -> ApiError {
    ApiError::new(
        ApiErrorCode::WorkspaceUnavailable,
        message,
        "platform-workspace",
    )
}

fn internal_error(message: &'static str) -> ApiError {
    ApiError::new(ApiErrorCode::Internal, message, "platform-workspace")
}
