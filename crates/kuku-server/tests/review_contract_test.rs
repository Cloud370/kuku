#![allow(dead_code, unused_imports, clippy::duplicate_mod)]

use std::future::{pending, Future};
use std::pin::Pin;
use std::sync::Arc;

use kuku_server::api::{
    AnnotationBatch, AnnotationSide, ApiError, ApiErrorCode, FilePage, PageCursor, ReviewSnapshot,
    ReviewSubmissionPage, ReviewSubmissionResult, RevisionToken, TaskId, TaskRevision, WorkspaceId,
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
pub mod review;

use review::{
    ReplayLookup, ReviewAdmission, ReviewFuture, ReviewLimits, ReviewRuntimePort, RevisionBudget,
    TaskReviewContext, ValidatedAnnotation, ValidatedReviewCommand, WorkspaceCapabilityProvider,
};

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/review/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).unwrap()
}

fn round_trip_fixture<T>(name: &str)
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let source = fixture(name);
    let decoded = serde_json::from_str::<T>(&source).unwrap();
    let encoded = format!("{}\n", serde_json::to_string_pretty(&decoded).unwrap());
    assert_eq!(source, encoded, "{name} is not canonical pretty JSON");
}

fn workspace_id(index: usize) -> WorkspaceId {
    WorkspaceId::parse(format!("wsp_{index:024x}")).unwrap()
}

fn task_id() -> TaskId {
    TaskId::parse("tsk_000000000000000000000001").unwrap()
}

fn revision(value: char) -> RevisionToken {
    RevisionToken::parse(value.to_string().repeat(64)).unwrap()
}

#[test]
fn canonical_review_fixtures_are_byte_stable() {
    round_trip_fixture::<FilePage>("file_page.json");
    round_trip_fixture::<ReviewSnapshot>("review_snapshot.json");
    round_trip_fixture::<ReviewSubmissionResult>("review_submission.json");
}

#[test]
fn api_annotation_side_is_the_sdk_type() {
    fn accepts_sdk_side(_: kuku::event::AnnotationSide) {}

    accepts_sdk_side(AnnotationSide::File);
    accepts_sdk_side(AnnotationSide::Old);
    accepts_sdk_side(AnnotationSide::New);
}

#[test]
fn validated_values_preserve_the_canonical_types() {
    let note = ValidatedAnnotation {
        path: "src/lib.rs".to_owned(),
        revision: revision('a'),
        side: AnnotationSide::File,
        start_line: 2,
        end_line: 3,
        excerpt: "first\nsecond".to_owned(),
        comment: "Tighten this boundary.".to_owned(),
    };
    let command = ValidatedReviewCommand {
        task_id: task_id(),
        workspace_id: workspace_id(1),
        expected_task_revision: TaskRevision::try_new(4).unwrap(),
        idempotency_key: "review-command-1".to_owned(),
        payload_hash: "b".repeat(64),
        notes: vec![note.clone()],
    };

    assert_eq!(AnnotationSide::File, note.side);
    assert_eq!("src/lib.rs", command.notes[0].path);
    assert_eq!(4, command.expected_task_revision.get());
}

#[test]
fn review_limit_defaults_match_the_contract() {
    let limits = ReviewLimits::default();

    assert_eq!(200, limits.tree_page_entries);
    assert_eq!(100, limits.search_page_matches);
    assert_eq!(2_000, limits.search_scan_entries);
    assert_eq!(256, limits.search_query_bytes);
    assert_eq!(4_096, limits.path_bytes);
    assert_eq!(64, limits.path_components);
    assert_eq!(1024 * 1024, limits.file_bytes);
    assert_eq!(2_000, limits.file_lines);
    assert_eq!(2 * 1024 * 1024, limits.diff_bytes);
    assert_eq!(4_000, limits.diff_lines);
    assert_eq!(8 * 1024 * 1024, limits.git_stream_bytes);
    assert_eq!(std::time::Duration::from_secs(5), limits.git_deadline);
    assert_eq!(64 * 1024 * 1024, limits.revision_file_hash_bytes);
    assert_eq!(20_000, limits.revision_listing_entries);
    assert_eq!(256 * 1024 * 1024, limits.revision_listing_hash_bytes);
    assert_eq!(100_000, limits.revision_git_entries);
    assert_eq!(1024 * 1024 * 1024, limits.revision_git_hash_bytes);
    assert_eq!(std::time::Duration::from_secs(5), limits.revision_deadline);
    assert_eq!(8, limits.global_scan_permits);
    assert_eq!(2, limits.workspace_scan_permits);
    assert_eq!(4, limits.global_git_permits);
    assert_eq!(1, limits.workspace_git_permits);
    assert_eq!(50, limits.submissions_page);
    assert_eq!(50, limits.notes_per_batch);
    assert_eq!(8 * 1024, limits.comment_bytes);
    assert_eq!(16 * 1024, limits.excerpt_bytes);
}

#[test]
fn revision_budget_retry_keeps_prior_debits() {
    let limits = ReviewLimits {
        revision_file_hash_bytes: 3,
        revision_listing_entries: 2,
        revision_listing_hash_bytes: 4,
        ..ReviewLimits::default()
    };
    let mut budget = RevisionBudget::new(&limits);

    budget.debit_file_bytes(2).unwrap();
    budget.begin_retry().unwrap();
    budget.debit_file_bytes(1).unwrap();
    assert_eq!(
        ApiErrorCode::PayloadTooLarge,
        budget.debit_file_bytes(1).unwrap_err().code()
    );
    budget.debit_listing(1, 3).unwrap();
    assert_eq!(
        ApiErrorCode::PayloadTooLarge,
        budget.debit_listing(1, 2).unwrap_err().code()
    );
    assert_eq!(
        ApiErrorCode::Outdated,
        budget.begin_retry().unwrap_err().code()
    );
}

#[tokio::test]
async fn scan_admission_rejects_global_and_workspace_n_plus_one() {
    let limits = ReviewLimits::default();
    let admission = ReviewAdmission::new(&limits);
    let workspace = workspace_id(1);
    let _first = admission.try_acquire_scan(&workspace).unwrap();
    let _second = admission.try_acquire_scan(&workspace).unwrap();
    assert_eq!(
        ApiErrorCode::ServerBusy,
        admission.try_acquire_scan(&workspace).unwrap_err().code()
    );

    let admission = ReviewAdmission::new(&limits);
    let permits = (0..limits.global_scan_permits)
        .map(|index| {
            admission
                .try_acquire_scan(&workspace_id(index + 1))
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        ApiErrorCode::ServerBusy,
        admission
            .try_acquire_scan(&workspace_id(20))
            .unwrap_err()
            .code()
    );
    drop(permits);
}

#[tokio::test]
async fn git_admission_rejects_global_and_workspace_n_plus_one() {
    let limits = ReviewLimits::default();
    let admission = ReviewAdmission::new(&limits);
    let workspace = workspace_id(1);
    let _first = admission.try_acquire_git(&workspace).unwrap();
    assert_eq!(
        ApiErrorCode::ServerBusy,
        admission.try_acquire_git(&workspace).unwrap_err().code()
    );

    let admission = ReviewAdmission::new(&limits);
    let permits = (0..limits.global_git_permits)
        .map(|index| admission.try_acquire_git(&workspace_id(index + 1)).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        ApiErrorCode::ServerBusy,
        admission
            .try_acquire_git(&workspace_id(20))
            .unwrap_err()
            .code()
    );
    drop(permits);
}

#[tokio::test]
async fn aborted_holder_releases_both_admission_permits() {
    let limits = ReviewLimits {
        global_scan_permits: 1,
        workspace_scan_permits: 1,
        ..ReviewLimits::default()
    };
    let admission = Arc::new(ReviewAdmission::new(&limits));
    let workspace = workspace_id(1);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let holder_admission = Arc::clone(&admission);
    let holder_workspace = workspace.clone();
    let holder = tokio::spawn(async move {
        let _permit = holder_admission
            .try_acquire_scan(&holder_workspace)
            .unwrap();
        ready_tx.send(()).unwrap();
        pending::<()>().await;
    });
    ready_rx.await.unwrap();
    assert_eq!(
        ApiErrorCode::ServerBusy,
        admission.try_acquire_scan(&workspace).unwrap_err().code()
    );

    holder.abort();
    let _ = holder.await;
    let _released = admission.try_acquire_scan(&workspace).unwrap();
}

struct FakeCapabilityProvider;

impl WorkspaceCapabilityProvider for FakeCapabilityProvider {
    fn capability(
        &self,
        _workspace_id: &WorkspaceId,
    ) -> Result<kuku_server::platform::WorkspaceCapability, ApiError> {
        Err(ApiError::new(
            ApiErrorCode::WorkspaceNotFound,
            "workspace is unavailable",
            "review-contract-test",
        ))
    }
}

#[derive(Clone)]
struct FakeRuntime {
    result: ReviewSubmissionResult,
}

impl ReviewRuntimePort for FakeRuntime {
    fn task_context<'a>(&'a self, task_id: &'a TaskId) -> ReviewFuture<'a, TaskReviewContext> {
        Box::pin(async move {
            Ok(TaskReviewContext {
                task_id: task_id.clone(),
                workspace_id: workspace_id(1),
                task_revision: TaskRevision::try_new(3).unwrap(),
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
        Box::pin(async move { Ok(ReplayLookup::Replay(self.result.clone())) })
    }

    fn submit_validated<'a>(
        &'a self,
        _command: ValidatedReviewCommand,
    ) -> ReviewFuture<'a, ReviewSubmissionResult> {
        Box::pin(async move { Ok(self.result.clone()) })
    }

    fn list_submissions<'a>(
        &'a self,
        _task_id: &'a TaskId,
        _cursor: Option<&'a PageCursor>,
        _limit: u16,
    ) -> ReviewFuture<'a, ReviewSubmissionPage> {
        Box::pin(async move {
            Ok(ReviewSubmissionPage {
                api_version: kuku_server::api::ApiVersion,
                task_id: task_id(),
                items: vec![self.result.submission.clone()],
                next_cursor: None,
            })
        })
    }
}

#[tokio::test]
async fn fake_ports_compile_against_the_review_boundary() {
    fn assert_future<T>(_: Pin<Box<dyn Future<Output = Result<T, ApiError>> + Send + '_>>) {}
    fn assert_provider<T: WorkspaceCapabilityProvider + Send + Sync>() {}
    fn assert_runtime<T: ReviewRuntimePort + Send + Sync>() {}

    assert_provider::<FakeCapabilityProvider>();
    assert_runtime::<FakeRuntime>();
    let runtime = FakeRuntime {
        result: serde_json::from_str(&fixture("review_submission.json")).unwrap(),
    };
    let task_id = task_id();
    let payload_hash = "b".repeat(64);
    let future = runtime.lookup_submission(&task_id, "review-command-1", &payload_hash);
    assert_future(future);
    assert!(matches!(
        runtime
            .lookup_submission(&task_id, "review-command-1", &payload_hash)
            .await
            .unwrap(),
        ReplayLookup::Replay(_)
    ));
}

#[test]
fn canonical_annotation_batch_remains_consumable() {
    let batch: AnnotationBatch = serde_json::from_value(serde_json::json!({
        "expected_task_revision": 1,
        "idempotency_key": "review-command-1",
        "notes": [{
            "path": "src/lib.rs",
            "revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "side": "file",
            "start_line": 1,
            "end_line": 1,
            "excerpt": "pub mod api;",
            "comment": "Keep this public."
        }]
    }))
    .unwrap();
    assert_eq!(AnnotationSide::File, batch.notes[0].side);
}
