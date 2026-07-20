use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::error::Result;
use crate::event::ExecutionScope;

pub trait WorkspaceQueryCapability: std::fmt::Debug + Send + Sync {
    fn workspace_id(&self) -> &str;

    fn verify_identity(&self) -> Result<()>;

    fn file_exists(&self, relative_path: &str) -> Result<bool>;

    fn read_file(&self, relative_path: &str, max_bytes: usize) -> Result<Vec<u8>>;

    fn write_file(&self, relative_path: &str, contents: &[u8], max_bytes: usize) -> Result<()>;

    fn list_entries(&self, relative_path: &str, max_entries: usize) -> Result<Vec<WorkspaceEntry>>;

    fn run_command<'a>(
        &'a self,
        request: WorkspaceCommandRequest,
        events: Option<tokio::sync::mpsc::Sender<WorkspaceCommandEvent>>,
        cancellation: WorkspaceCommandCancellation,
    ) -> Pin<Box<dyn Future<Output = Result<WorkspaceCommandOutput>> + Send + 'a>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub path: String,
    pub is_file: bool,
    pub is_dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCommandRequest {
    pub command: String,
    pub timeout: std::time::Duration,
    pub max_output_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceCommandEvent {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCommandOutput {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Default)]
pub struct WorkspaceCommandCancellation {
    cancelled: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl WorkspaceCommandCancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub async fn cancelled(&self) {
        loop {
            if self.is_cancelled() {
                return;
            }
            let notified = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

#[derive(Clone)]
pub struct TaskQueryContext {
    pub(super) execution_scope: ExecutionScope,
    pub(super) event_store: crate::event::EventStore,
    pub(super) workspace: Arc<dyn WorkspaceQueryCapability>,
    pub(super) selected_skills: Vec<String>,
}

impl std::fmt::Debug for TaskQueryContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TaskQueryContext")
            .field("execution_scope", &self.execution_scope)
            .field("workspace", &self.workspace)
            .finish_non_exhaustive()
    }
}

impl TaskQueryContext {
    pub fn new(
        execution_scope: ExecutionScope,
        event_store: crate::event::EventStore,
        workspace: Arc<dyn WorkspaceQueryCapability>,
    ) -> Self {
        Self {
            execution_scope,
            event_store,
            workspace,
            selected_skills: Vec::new(),
        }
    }

    pub fn with_selected_skills(mut self, selected_skills: Vec<String>) -> Self {
        self.selected_skills = selected_skills;
        self
    }

    pub(crate) fn for_nested(&self, execution_scope: ExecutionScope) -> Self {
        Self {
            execution_scope,
            event_store: self.event_store.clone(),
            workspace: self.workspace.clone(),
            selected_skills: self.selected_skills.clone(),
        }
    }
}
