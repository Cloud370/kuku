#![allow(dead_code, unused_imports)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use kuku_server::api::{
    AnnotationSide, AnnotationStatus, ApiError, ApiErrorCode, ApiVersion, RegisterWorkspaceRequest,
    ReviewSubmissionId, ReviewSubmissionPage, ReviewSubmissionProjection, ReviewSubmissionResult,
    RunId, SubmittedReviewNote, TaskId, TaskRevision, WorkspaceId,
};
use kuku_server::platform::{
    RegistrationRootRegistry, RegistrationRootSpec, ServerRevisionCoordinator, WorkspaceRegistry,
    WorkspaceUsagePort,
};

mod api {
    pub use kuku_server::api::*;
}

mod platform {
    pub use kuku_server::platform::*;
}

mod run_manager {
    pub use kuku_server::run_manager::*;
}

#[path = "../src/review/mod.rs"]
mod review_contract;

#[path = "common/review_modules.rs"]
mod review;

use review::{
    ReplayLookup, ReviewFuture, ReviewLimits, ReviewRuntimePort, TaskReviewContext,
    WorkspaceCapabilityProvider,
};

use review::submissions::ReviewSubmissionService;

struct UnusedUsage;

impl WorkspaceUsagePort for UnusedUsage {
    fn has_durable_tasks<'a>(
        &'a self,
        _id: &'a WorkspaceId,
    ) -> Pin<Box<dyn Future<Output = Result<bool, ApiError>> + Send + 'a>> {
        Box::pin(async { Ok(false) })
    }
}

struct Provider(Arc<WorkspaceRegistry>);

impl WorkspaceCapabilityProvider for Provider {
    fn capability(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<kuku_server::platform::WorkspaceCapability, ApiError> {
        self.0.capability(workspace_id)
    }
}

struct Workspace {
    _home: tempfile::TempDir,
    _allowed: tempfile::TempDir,
    root: std::path::PathBuf,
    id: WorkspaceId,
    provider: Arc<Provider>,
}

impl Workspace {
    async fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let allowed = tempfile::tempdir().unwrap();
        let root = allowed.path().join("project");
        std::fs::create_dir(&root).unwrap();
        let roots = RegistrationRootRegistry::from_server_config(
            home.path(),
            vec![RegistrationRootSpec {
                label: "Review".to_owned(),
                path: allowed.path().to_owned(),
            }],
        )
        .unwrap();
        let registry = WorkspaceRegistry::open(
            home.path(),
            roots,
            Arc::new(UnusedUsage),
            ServerRevisionCoordinator::open(home.path()),
        )
        .unwrap();
        let root_id = registry.registration_roots().list()[0].root_id.clone();
        let summary = registry
            .register(RegisterWorkspaceRequest {
                root_id,
                relative_path: "project".to_owned(),
                label: "Project".to_owned(),
                expected_revision: registry.revision().await.unwrap(),
            })
            .await
            .unwrap();
        Self {
            _home: home,
            _allowed: allowed,
            root,
            id: summary.workspace_id,
            provider: Arc::new(Provider(registry)),
        }
    }
}

#[derive(Clone)]
struct FakeRuntime {
    workspace_id: WorkspaceId,
    page: ReviewSubmissionPage,
}

impl ReviewRuntimePort for FakeRuntime {
    fn task_context<'a>(&'a self, task_id: &'a TaskId) -> ReviewFuture<'a, TaskReviewContext> {
        let task_id = task_id.clone();
        Box::pin(async move {
            Ok(TaskReviewContext {
                task_id,
                workspace_id: self.workspace_id.clone(),
                task_revision: TaskRevision::try_new(4).unwrap(),
                active_run: false,
            })
        })
    }

    fn lookup_submission<'a>(
        &'a self,
        _task_id: &'a TaskId,
        _idempotency_key: &'a str,
        _payload_hash: &'a str,
    ) -> ReviewFuture<'a, ReplayLookup> {
        Box::pin(async { Ok(ReplayLookup::Missing) })
    }

    fn submit_validated<'a>(
        &'a self,
        _command: review::ValidatedReviewCommand,
    ) -> ReviewFuture<'a, ReviewSubmissionResult> {
        Box::pin(async {
            Err(ApiError::new(
                ApiErrorCode::Internal,
                "unused",
                "review-submissions-test",
            ))
        })
    }

    fn list_submissions<'a>(
        &'a self,
        _task_id: &'a TaskId,
        _cursor: Option<&'a kuku_server::api::PageCursor>,
        _limit: u16,
    ) -> ReviewFuture<'a, ReviewSubmissionPage> {
        let page = self.page.clone();
        Box::pin(async move { Ok(page) })
    }
}

#[tokio::test]
async fn durable_submissions_rebuild_and_mark_changed_anchors_outdated() {
    let workspace = Workspace::new().await;
    std::fs::write(workspace.root.join("note.txt"), "one\ntwo\n").unwrap();
    let note = ReviewSubmissionProjection {
        submission_id: ReviewSubmissionId::parse("rsub_000000000000000000000001").unwrap(),
        task_id: task_id(),
        run_id: RunId::parse("run_000000000000000000000001").unwrap(),
        task_revision: TaskRevision::try_new(4).unwrap(),
        submitted_at: "2026-07-21T00:00:00Z".to_owned(),
        notes: vec![SubmittedReviewNote {
            path: "note.txt".to_owned(),
            revision: review::files::WorkspaceReadService::new(workspace.provider.clone())
                .current_revision(&workspace.id, "note.txt")
                .await
                .unwrap(),
            side: AnnotationSide::File,
            start_line: 1,
            end_line: 1,
            excerpt: "one".to_owned(),
            comment: "Keep this clear.".to_owned(),
            status: AnnotationStatus::Current,
        }],
    };
    let runtime = Arc::new(FakeRuntime {
        workspace_id: workspace.id.clone(),
        page: ReviewSubmissionPage {
            api_version: ApiVersion,
            task_id: task_id(),
            items: vec![note.clone()],
            next_cursor: None,
        },
    });
    let service =
        ReviewSubmissionService::new(workspace.provider.clone(), runtime, ReviewLimits::default());

    let first = service.list(&task_id(), None, 50).await.unwrap();
    assert_eq!(AnnotationStatus::Current, first.items[0].notes[0].status);
    std::fs::write(workspace.root.join("note.txt"), "changed\ntwo\n").unwrap();
    let second = service.list(&task_id(), None, 50).await.unwrap();
    assert_eq!(AnnotationStatus::Outdated, second.items[0].notes[0].status);
    assert_eq!("one", note.notes[0].excerpt);
}

fn task_id() -> TaskId {
    TaskId::parse("tsk_000000000000000000000001").unwrap()
}

fn workspace_id() -> WorkspaceId {
    WorkspaceId::parse("wsp_000000000000000000000001").unwrap()
}
