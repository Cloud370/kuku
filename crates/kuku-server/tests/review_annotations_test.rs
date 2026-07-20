#![allow(dead_code)]

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Command;
use std::sync::{Arc, Mutex};

use kuku_server::api::{
    AnnotationBatch, AnnotationDraft, AnnotationSide, AnnotationStatus, ApiError, ApiErrorCode,
    ApiVersion, RegisterWorkspaceRequest, ReviewSubmissionId, ReviewSubmissionProjection,
    ReviewSubmissionResult, RunId, SubmittedReviewNote, TaskId, TaskRevision, WorkspaceId,
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

use review::annotations::AnnotationService;
use review::{
    ReplayLookup, ReviewFuture, ReviewLimits, ReviewRuntimePort, TaskReviewContext,
    ValidatedReviewCommand, WorkspaceCapabilityProvider,
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

struct RegistryProvider(Arc<WorkspaceRegistry>);

impl WorkspaceCapabilityProvider for RegistryProvider {
    fn capability(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<kuku_server::platform::WorkspaceCapability, ApiError> {
        self.0.capability(workspace_id)
    }
}

struct Repository {
    _allowed: tempfile::TempDir,
    _home: tempfile::TempDir,
    root: PathBuf,
    workspace_id: WorkspaceId,
    provider: Arc<RegistryProvider>,
}

impl Repository {
    async fn new() -> Self {
        let allowed = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let root = allowed.path().join("project");
        std::fs::create_dir(&root).unwrap();
        git_at(&root, &["init", "-q"]);
        git_at(&root, &["config", "user.email", "review@example.invalid"]);
        git_at(&root, &["config", "user.name", "Review Test"]);
        let roots = RegistrationRootRegistry::from_server_config(
            home.path(),
            vec![RegistrationRootSpec {
                label: "Review fixtures".to_owned(),
                path: allowed.path().to_owned(),
            }],
        )
        .unwrap();
        let root_id = roots.list()[0].root_id.clone();
        let registry = WorkspaceRegistry::open(
            home.path(),
            roots,
            Arc::new(UnusedUsage),
            ServerRevisionCoordinator::open(home.path()),
        )
        .unwrap();
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
            _allowed: allowed,
            _home: home,
            root,
            workspace_id: summary.workspace_id,
            provider: Arc::new(RegistryProvider(registry)),
        }
    }

    fn write(&self, path: &str, contents: &str) {
        std::fs::write(self.root.join(path), contents).unwrap();
    }

    fn commit_all(&self, message: &str) {
        git_at(&self.root, &["add", "--all"]);
        git_at(&self.root, &["commit", "-qm", message]);
    }
}

fn git_at(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env_remove("GIT_CONFIG_GLOBAL")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap();
    assert!(output.status.success(), "git failed: {args:?}");
}

#[derive(Clone)]
struct FakeRuntime {
    context: Arc<Mutex<TaskReviewContext>>,
    lookup: Arc<Mutex<ReplayLookup>>,
    submitted: Arc<Mutex<Vec<ValidatedReviewCommand>>>,
    result: ReviewSubmissionResult,
}

impl FakeRuntime {
    fn new(workspace_id: WorkspaceId) -> Self {
        let task_id = task_id();
        Self {
            context: Arc::new(Mutex::new(TaskReviewContext {
                task_id: task_id.clone(),
                workspace_id,
                task_revision: task_revision(),
                active_run: false,
            })),
            lookup: Arc::new(Mutex::new(ReplayLookup::Missing)),
            submitted: Arc::new(Mutex::new(Vec::new())),
            result: result(task_id),
        }
    }
}

impl ReviewRuntimePort for FakeRuntime {
    fn task_context<'a>(&'a self, _task_id: &'a TaskId) -> ReviewFuture<'a, TaskReviewContext> {
        let context = self.context.lock().unwrap().clone();
        Box::pin(async move { Ok(context) })
    }

    fn lookup_submission<'a>(
        &'a self,
        _task_id: &'a TaskId,
        _idempotency_key: &'a str,
        _payload_hash: &'a str,
    ) -> ReviewFuture<'a, ReplayLookup> {
        let lookup = self.lookup.lock().unwrap().clone();
        Box::pin(async move { Ok(lookup) })
    }

    fn submit_validated<'a>(
        &'a self,
        command: ValidatedReviewCommand,
    ) -> ReviewFuture<'a, ReviewSubmissionResult> {
        self.submitted.lock().unwrap().push(command);
        let result = self.result.clone();
        Box::pin(async move { Ok(result) })
    }

    fn list_submissions<'a>(
        &'a self,
        _task_id: &'a TaskId,
        _cursor: Option<&'a kuku_server::api::PageCursor>,
        _limit: u16,
    ) -> ReviewFuture<'a, kuku_server::api::ReviewSubmissionPage> {
        Box::pin(async {
            Err(ApiError::new(
                ApiErrorCode::Internal,
                "unused",
                "review-annotations-test",
            ))
        })
    }
}

#[tokio::test]
async fn mixed_file_old_new_batch_validates_once_and_submits_atomically() {
    let repo = Repository::new().await;
    repo.write("tracked.txt", "old\nshared\n");
    repo.commit_all("base");
    repo.write("tracked.txt", "new\nshared\n");
    repo.write("file.txt", "alpha\nbeta\ngamma\n");
    let runtime = Arc::new(FakeRuntime::new(repo.workspace_id.clone()));
    let service = AnnotationService::new(
        repo.provider.clone(),
        runtime.clone(),
        ReviewLimits::default(),
    );
    let file_revision = review::files::WorkspaceReadService::new(repo.provider.clone())
        .current_revision(&repo.workspace_id, "file.txt")
        .await
        .unwrap();
    let capability = repo.provider.capability(&repo.workspace_id).unwrap();
    let snapshot = review::git::GitReviewService::new(capability, ReviewLimits::default())
        .snapshot(None, 100)
        .await
        .unwrap();
    let change_revision = snapshot
        .entries
        .iter()
        .find(|entry| entry.path == "tracked.txt")
        .unwrap()
        .revision
        .clone();
    let batch = AnnotationBatch {
        expected_task_revision: task_revision(),
        idempotency_key: "review-mixed-1".to_owned(),
        notes: vec![
            draft(
                "file.txt",
                file_revision,
                AnnotationSide::File,
                2,
                3,
                "beta\ngamma",
            ),
            draft(
                "tracked.txt",
                change_revision.clone(),
                AnnotationSide::Old,
                1,
                1,
                "old",
            ),
            draft(
                "tracked.txt",
                change_revision,
                AnnotationSide::New,
                1,
                1,
                "new",
            ),
        ],
    };

    let submitted = service.submit(&task_id(), batch).await.unwrap();

    assert!(!submitted.replayed);
    let commands = runtime.submitted.lock().unwrap();
    assert_eq!(1, commands.len());
    assert_eq!(3, commands[0].notes.len());
    assert_eq!(64, commands[0].payload_hash.len());
}

#[tokio::test]
async fn invalid_batches_never_submit_and_keep_error_codes_distinct() {
    let repo = Repository::new().await;
    repo.write("file.txt", "one\ntwo\n");
    let runtime = Arc::new(FakeRuntime::new(repo.workspace_id.clone()));
    let service = AnnotationService::new(
        repo.provider.clone(),
        runtime.clone(),
        ReviewLimits::default(),
    );
    let revision = review::files::WorkspaceReadService::new(repo.provider.clone())
        .current_revision(&repo.workspace_id, "file.txt")
        .await
        .unwrap();

    let cases = [
        AnnotationBatch {
            expected_task_revision: task_revision(),
            idempotency_key: "empty".to_owned(),
            notes: Vec::new(),
        },
        AnnotationBatch {
            expected_task_revision: task_revision(),
            idempotency_key: "reversed".to_owned(),
            notes: vec![draft(
                "file.txt",
                revision.clone(),
                AnnotationSide::File,
                2,
                1,
                "",
            )],
        },
        AnnotationBatch {
            expected_task_revision: task_revision(),
            idempotency_key: "wrong-excerpt".to_owned(),
            notes: vec![draft(
                "file.txt",
                revision.clone(),
                AnnotationSide::File,
                1,
                1,
                "wrong",
            )],
        },
    ];
    for batch in cases {
        assert!(service.submit(&task_id(), batch).await.is_err());
    }
    let too_many = AnnotationBatch {
        expected_task_revision: task_revision(),
        idempotency_key: "too-many".to_owned(),
        notes: (0..51)
            .map(|_| {
                draft(
                    "file.txt",
                    revision.clone(),
                    AnnotationSide::File,
                    1,
                    1,
                    "one",
                )
            })
            .collect(),
    };
    assert_eq!(
        ApiErrorCode::PayloadTooLarge,
        service
            .submit(&task_id(), too_many)
            .await
            .unwrap_err()
            .code()
    );
    assert!(runtime.submitted.lock().unwrap().is_empty());

    runtime.context.lock().unwrap().task_revision = TaskRevision::try_new(8).unwrap();
    let stale = valid_file_batch(revision.clone());
    assert_eq!(
        ApiErrorCode::StaleCommand,
        service.submit(&task_id(), stale).await.unwrap_err().code()
    );
    runtime.context.lock().unwrap().task_revision = task_revision();
    runtime.context.lock().unwrap().active_run = true;
    assert_eq!(
        ApiErrorCode::TaskBusy,
        service
            .submit(&task_id(), valid_file_batch(revision))
            .await
            .unwrap_err()
            .code()
    );
}

#[tokio::test]
async fn replay_and_conflict_return_before_workspace_validation() {
    let repo = Repository::new().await;
    repo.write("file.txt", "one\n");
    let runtime = Arc::new(FakeRuntime::new(repo.workspace_id.clone()));
    let service = AnnotationService::new(
        repo.provider.clone(),
        runtime.clone(),
        ReviewLimits::default(),
    );
    let revision = review::files::WorkspaceReadService::new(repo.provider.clone())
        .current_revision(&repo.workspace_id, "file.txt")
        .await
        .unwrap();
    let batch = valid_file_batch(revision);
    std::fs::remove_file(repo.root.join("file.txt")).unwrap();
    *runtime.lookup.lock().unwrap() = ReplayLookup::Replay(runtime.result.clone());

    let replay = service.submit(&task_id(), batch.clone()).await.unwrap();
    assert!(replay.replayed);

    *runtime.lookup.lock().unwrap() = ReplayLookup::Conflict;
    assert_eq!(
        ApiErrorCode::IdempotencyConflict,
        service.submit(&task_id(), batch).await.unwrap_err().code()
    );
    assert!(runtime.submitted.lock().unwrap().is_empty());
}

fn task_id() -> TaskId {
    TaskId::parse("tsk_000000000000000000000001").unwrap()
}

fn task_revision() -> TaskRevision {
    TaskRevision::try_new(3).unwrap()
}

fn draft(
    path: &str,
    revision: kuku_server::api::RevisionToken,
    side: AnnotationSide,
    start_line: u32,
    end_line: u32,
    excerpt: &str,
) -> AnnotationDraft {
    AnnotationDraft {
        path: path.to_owned(),
        revision,
        side,
        start_line,
        end_line,
        excerpt: excerpt.to_owned(),
        comment: "Please revise this.".to_owned(),
    }
}

fn valid_file_batch(revision: kuku_server::api::RevisionToken) -> AnnotationBatch {
    AnnotationBatch {
        expected_task_revision: task_revision(),
        idempotency_key: "review-file-1".to_owned(),
        notes: vec![draft(
            "file.txt",
            revision,
            AnnotationSide::File,
            1,
            1,
            "one",
        )],
    }
}

fn result(task_id: TaskId) -> ReviewSubmissionResult {
    ReviewSubmissionResult {
        api_version: ApiVersion,
        submission: ReviewSubmissionProjection {
            submission_id: ReviewSubmissionId::parse("rsub_000000000000000000000001").unwrap(),
            task_id,
            run_id: RunId::parse("run_000000000000000000000001").unwrap(),
            task_revision: TaskRevision::try_new(4).unwrap(),
            submitted_at: "2026-07-21T00:00:00Z".to_owned(),
            notes: vec![SubmittedReviewNote {
                path: "file.txt".to_owned(),
                revision: kuku_server::api::RevisionToken::parse("a".repeat(64)).unwrap(),
                side: AnnotationSide::File,
                start_line: 1,
                end_line: 1,
                excerpt: "one".to_owned(),
                comment: "Please revise this.".to_owned(),
                status: AnnotationStatus::Current,
            }],
        },
        replayed: false,
    }
}
