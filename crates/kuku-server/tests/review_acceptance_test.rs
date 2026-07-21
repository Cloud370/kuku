#![allow(dead_code, unused_imports, clippy::duplicate_mod)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::http::StatusCode;

use kuku_server::api::{
    AnnotationStatus, ApiError, ApiErrorCode, ApiVersion, ReviewSubmissionPage,
    ReviewSubmissionProjection, ReviewSubmissionResult, RunId, SubmittedReviewNote, TaskId,
    TaskRevision, WorkspaceId,
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

#[path = "common/review_modules.rs"]
mod review;
#[path = "../src/review/mod.rs"]
mod review_contract;
#[path = "../src/routes/review.rs"]
mod review_routes;

use review::{
    ReplayLookup, ReviewFuture, ReviewRuntimePort, TaskReviewContext, WorkspaceCapabilityProvider,
};

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

#[derive(Clone)]
struct Runtime {
    workspace_id: WorkspaceId,
}

impl ReviewRuntimePort for Runtime {
    fn task_context<'a>(&'a self, task_id: &'a TaskId) -> ReviewFuture<'a, TaskReviewContext> {
        Box::pin(async move {
            Ok(TaskReviewContext {
                task_id: task_id.clone(),
                workspace_id: self.workspace_id.clone(),
                task_revision: TaskRevision::try_new(0).unwrap(),
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
                "review-acceptance-test",
            ))
        })
    }

    fn list_submissions<'a>(
        &'a self,
        task_id: &'a TaskId,
        _cursor: Option<&'a kuku_server::api::PageCursor>,
        _limit: u16,
    ) -> ReviewFuture<'a, ReviewSubmissionPage> {
        Box::pin(async move {
            Ok(ReviewSubmissionPage {
                api_version: ApiVersion,
                task_id: task_id.clone(),
                items: Vec::<ReviewSubmissionProjection>::new(),
                next_cursor: None,
            })
        })
    }
}

#[tokio::test]
async fn real_router_reads_workspace_content_and_returns_canonical_json() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    let project = allowed.path().join("project");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(project.join("README.md"), "hello\nworld\n").unwrap();
    let roots = RegistrationRootRegistry::from_server_config(
        home.path(),
        vec![RegistrationRootSpec {
            label: "Acceptance".to_owned(),
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
    let workspace = registry
        .register(kuku_server::api::RegisterWorkspaceRequest {
            root_id,
            relative_path: "project".to_owned(),
            label: "Project".to_owned(),
            expected_revision: registry.revision().await.unwrap(),
        })
        .await
        .unwrap();
    let provider: Arc<dyn WorkspaceCapabilityProvider> = Arc::new(Provider(registry));
    let runtime: Arc<dyn ReviewRuntimePort> = Arc::new(Runtime {
        workspace_id: workspace.workspace_id.clone(),
    });
    let limits = review::ReviewLimits::default();
    let files = Arc::new(review::files::WorkspaceReadService::new(provider.clone()));
    let annotations = Arc::new(review::annotations::AnnotationService::new(
        provider.clone(),
        runtime.clone(),
        limits.clone(),
    ));
    let submissions = Arc::new(review::submissions::ReviewSubmissionService::new(
        provider.clone(),
        runtime,
        limits.clone(),
    ));
    let state = Arc::new(review_routes::ReviewRouteState::new(
        provider,
        files,
        annotations,
        submissions,
        limits,
    ));
    let app = review_routes::router::<()>(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let response = wreq::Client::new()
        .get(format!(
            "http://{address}/workspaces/{}/files/content?path=README.md&start_line=1&end_line=2",
            workspace.workspace_id.as_str()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, response.status());
    let body = response
        .json::<kuku_server::api::FileContent>()
        .await
        .unwrap();
    assert_eq!(Some("hello\nworld"), body.text.as_deref());
    server.abort();
}
