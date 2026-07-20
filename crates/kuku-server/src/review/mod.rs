//! Defines the internal Review service boundary and resource limits.

pub(crate) mod annotations;
pub(crate) mod files;
pub(crate) mod git;
pub(crate) mod runtime;
pub(crate) mod submissions;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::api::{
    AnnotationSide, ApiError, ApiErrorCode, PageCursor, ReviewSubmissionPage,
    ReviewSubmissionResult, RevisionToken, TaskId, TaskRevision, WorkspaceId,
};
use crate::platform::{WorkspaceCapability, WorkspaceRegistry};

/// Holds every fixed Review service budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewLimits {
    pub tree_page_entries: u16,
    pub search_page_matches: u16,
    pub search_scan_entries: u32,
    pub search_query_bytes: usize,
    pub path_bytes: usize,
    pub path_components: usize,
    pub file_bytes: usize,
    pub file_lines: u32,
    pub diff_bytes: usize,
    pub diff_lines: u32,
    pub git_stream_bytes: usize,
    pub git_deadline: Duration,
    pub revision_file_hash_bytes: u64,
    pub revision_listing_entries: u32,
    pub revision_listing_hash_bytes: u64,
    pub revision_git_entries: u32,
    pub revision_git_hash_bytes: u64,
    pub revision_deadline: Duration,
    pub global_scan_permits: usize,
    pub workspace_scan_permits: usize,
    pub global_git_permits: usize,
    pub workspace_git_permits: usize,
    pub submissions_page: u16,
    pub notes_per_batch: u16,
    pub comment_bytes: usize,
    pub excerpt_bytes: usize,
}

impl Default for ReviewLimits {
    fn default() -> Self {
        Self {
            tree_page_entries: 200,
            search_page_matches: 100,
            search_scan_entries: 2_000,
            search_query_bytes: 256,
            path_bytes: 4_096,
            path_components: 64,
            file_bytes: 1024 * 1024,
            file_lines: 2_000,
            diff_bytes: 2 * 1024 * 1024,
            diff_lines: 4_000,
            git_stream_bytes: 8 * 1024 * 1024,
            git_deadline: Duration::from_secs(5),
            revision_file_hash_bytes: 64 * 1024 * 1024,
            revision_listing_entries: 20_000,
            revision_listing_hash_bytes: 256 * 1024 * 1024,
            revision_git_entries: 100_000,
            revision_git_hash_bytes: 1024 * 1024 * 1024,
            revision_deadline: Duration::from_secs(5),
            global_scan_permits: 8,
            workspace_scan_permits: 2,
            global_git_permits: 4,
            workspace_git_permits: 1,
            submissions_page: 50,
            notes_per_batch: 50,
            comment_bytes: 8 * 1024,
            excerpt_bytes: 16 * 1024,
        }
    }
}

/// Tracks one non-resetting revision capture budget.
#[derive(Debug)]
pub struct RevisionBudget {
    remaining_file_hash_bytes: u64,
    remaining_listing_entries: u32,
    remaining_listing_hash_bytes: u64,
    remaining_git_entries: u32,
    remaining_git_hash_bytes: u64,
    deadline: Instant,
    retried: bool,
}

impl RevisionBudget {
    /// Starts one request-scoped budget from the configured Review limits.
    pub fn new(limits: &ReviewLimits) -> Self {
        Self {
            remaining_file_hash_bytes: limits.revision_file_hash_bytes,
            remaining_listing_entries: limits.revision_listing_entries,
            remaining_listing_hash_bytes: limits.revision_listing_hash_bytes,
            remaining_git_entries: limits.revision_git_entries,
            remaining_git_hash_bytes: limits.revision_git_hash_bytes,
            deadline: Instant::now() + limits.revision_deadline,
            retried: false,
        }
    }

    /// Debits exact bytes read while hashing file content.
    pub fn debit_file_bytes(&mut self, bytes: u64) -> Result<(), ApiError> {
        self.check_deadline()?;
        if bytes > self.remaining_file_hash_bytes {
            return Err(payload_too_large());
        }
        self.remaining_file_hash_bytes -= bytes;
        Ok(())
    }

    /// Debits one group of canonical listing records and their framed bytes.
    pub fn debit_listing(&mut self, entries: u32, bytes: u64) -> Result<(), ApiError> {
        self.check_deadline()?;
        if entries > self.remaining_listing_entries || bytes > self.remaining_listing_hash_bytes {
            return Err(payload_too_large());
        }
        self.remaining_listing_entries -= entries;
        self.remaining_listing_hash_bytes -= bytes;
        Ok(())
    }

    /// Debits one group of canonical Git records and their exact bytes.
    pub fn debit_git(&mut self, entries: u32, bytes: u64) -> Result<(), ApiError> {
        self.check_deadline()?;
        if entries > self.remaining_git_entries || bytes > self.remaining_git_hash_bytes {
            return Err(payload_too_large());
        }
        self.remaining_git_entries -= entries;
        self.remaining_git_hash_bytes -= bytes;
        Ok(())
    }

    /// Begins the sole retry without restoring any previously debited capacity.
    pub fn begin_retry(&mut self) -> Result<(), ApiError> {
        self.check_deadline()?;
        if self.retried {
            return Err(outdated());
        }
        self.retried = true;
        Ok(())
    }

    /// Rejects work after the request-scoped revision deadline.
    pub fn check_deadline(&self) -> Result<(), ApiError> {
        if Instant::now() > self.deadline {
            Err(payload_too_large())
        } else {
            Ok(())
        }
    }

    /// Returns the remaining request-scoped wall-clock budget.
    pub fn remaining_deadline(&self) -> Result<Duration, ApiError> {
        self.check_deadline()?;
        Ok(self.deadline.saturating_duration_since(Instant::now()))
    }
}

/// Owns hierarchical non-waiting capacity for Review scans and Git commands.
#[derive(Debug)]
pub struct ReviewAdmission {
    global_scan: Arc<Semaphore>,
    workspace_scan: Mutex<HashMap<WorkspaceId, Weak<Semaphore>>>,
    workspace_scan_permits: usize,
    global_git: Arc<Semaphore>,
    workspace_git: Mutex<HashMap<WorkspaceId, Weak<Semaphore>>>,
    workspace_git_permits: usize,
}

impl ReviewAdmission {
    /// Creates independent global and per-workspace permit pools.
    pub fn new(limits: &ReviewLimits) -> Self {
        Self {
            global_scan: Arc::new(Semaphore::new(limits.global_scan_permits)),
            workspace_scan: Mutex::new(HashMap::new()),
            workspace_scan_permits: limits.workspace_scan_permits,
            global_git: Arc::new(Semaphore::new(limits.global_git_permits)),
            workspace_git: Mutex::new(HashMap::new()),
            workspace_git_permits: limits.workspace_git_permits,
        }
    }

    /// Acquires scan capacity immediately or returns `server_busy`.
    pub fn try_acquire_scan(&self, workspace_id: &WorkspaceId) -> Result<ReviewPermit, ApiError> {
        Self::try_acquire(
            &self.global_scan,
            &self.workspace_scan,
            self.workspace_scan_permits,
            workspace_id,
        )
    }

    /// Acquires Git capacity immediately or returns `server_busy`.
    pub fn try_acquire_git(&self, workspace_id: &WorkspaceId) -> Result<ReviewPermit, ApiError> {
        Self::try_acquire(
            &self.global_git,
            &self.workspace_git,
            self.workspace_git_permits,
            workspace_id,
        )
    }

    fn try_acquire(
        global: &Arc<Semaphore>,
        keyed: &Mutex<HashMap<WorkspaceId, Weak<Semaphore>>>,
        keyed_permits: usize,
        workspace_id: &WorkspaceId,
    ) -> Result<ReviewPermit, ApiError> {
        let global = Arc::clone(global)
            .try_acquire_owned()
            .map_err(|_| server_busy())?;
        let workspace_pool = {
            let mut pools = keyed.lock().expect("review admission lock is not poisoned");
            match pools.get(workspace_id).and_then(Weak::upgrade) {
                Some(pool) => pool,
                None => {
                    let pool = Arc::new(Semaphore::new(keyed_permits));
                    pools.insert(workspace_id.clone(), Arc::downgrade(&pool));
                    pool
                }
            }
        };
        let workspace = workspace_pool
            .try_acquire_owned()
            .map_err(|_| server_busy())?;
        Ok(ReviewPermit {
            _global: global,
            _workspace: workspace,
        })
    }
}

/// Holds both levels of one Review admission grant for its request lifetime.
#[derive(Debug)]
pub struct ReviewPermit {
    _global: OwnedSemaphorePermit,
    _workspace: OwnedSemaphorePermit,
}

/// Resolves opaque workspace IDs into Platform-owned capabilities.
pub trait WorkspaceCapabilityProvider: Send + Sync {
    fn capability(&self, workspace_id: &WorkspaceId) -> Result<WorkspaceCapability, ApiError>;
}

impl WorkspaceCapabilityProvider for WorkspaceRegistry {
    fn capability(&self, workspace_id: &WorkspaceId) -> Result<WorkspaceCapability, ApiError> {
        WorkspaceRegistry::capability(self, workspace_id)
    }
}

/// Captures the Task facts required before validating a review command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskReviewContext {
    pub task_id: TaskId,
    pub workspace_id: WorkspaceId,
    pub task_revision: TaskRevision,
    pub active_run: bool,
}

/// Describes the result of replay-first idempotency lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayLookup {
    Missing,
    Replay(ReviewSubmissionResult),
    Conflict,
}

/// Holds one side-aware annotation after content validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedAnnotation {
    pub path: String,
    pub revision: RevisionToken,
    pub side: AnnotationSide,
    pub start_line: u32,
    pub end_line: u32,
    pub excerpt: String,
    pub comment: String,
}

/// Holds one normalized annotation batch ready for the Runtime transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedReviewCommand {
    pub task_id: TaskId,
    pub workspace_id: WorkspaceId,
    pub expected_task_revision: TaskRevision,
    pub idempotency_key: String,
    pub payload_hash: String,
    pub notes: Vec<ValidatedAnnotation>,
}

/// Represents an owned asynchronous Runtime operation.
pub type ReviewFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ApiError>> + Send + 'a>>;

/// Defines the transaction boundary implemented later by the Task runtime.
pub trait ReviewRuntimePort: Send + Sync {
    fn task_context<'a>(&'a self, task_id: &'a TaskId) -> ReviewFuture<'a, TaskReviewContext>;

    fn lookup_submission<'a>(
        &'a self,
        task_id: &'a TaskId,
        idempotency_key: &'a str,
        payload_hash: &'a str,
    ) -> ReviewFuture<'a, ReplayLookup>;

    fn submit_validated<'a>(
        &'a self,
        command: ValidatedReviewCommand,
    ) -> ReviewFuture<'a, ReviewSubmissionResult>;

    fn list_submissions<'a>(
        &'a self,
        task_id: &'a TaskId,
        cursor: Option<&'a PageCursor>,
        limit: u16,
    ) -> ReviewFuture<'a, ReviewSubmissionPage>;
}

fn payload_too_large() -> ApiError {
    ApiError::new(
        ApiErrorCode::PayloadTooLarge,
        "review revision budget is exhausted",
        "review-service",
    )
}

fn outdated() -> ApiError {
    ApiError::new(
        ApiErrorCode::Outdated,
        "review content changed during capture",
        "review-service",
    )
}

fn server_busy() -> ApiError {
    ApiError::new(
        ApiErrorCode::ServerBusy,
        "review capacity is busy",
        "review-service",
    )
}
