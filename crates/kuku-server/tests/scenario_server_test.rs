#![cfg(feature = "test-scenarios")]

#[allow(dead_code)]
mod common;

const FULL_TASK_FIXTURE: &str = include_str!("fixtures/scenarios/full_task.json");
const HUMAN_ACCEPTANCE_FIXTURE: &str = include_str!("fixtures/scenarios/human_acceptance.json");

use std::net::SocketAddr;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use kuku::event::{
    CommandReceipt, CommandResult, Cursor, MessageFact, MessageRoleFact, TaskActivityBatch,
    TaskEvent, TaskId, TaskLedgerRecord, TaskRevision, TaskTransaction, WorkspaceId,
};
use kuku_server::api::{
    AnnotationBatch, AnnotationDraft, AnnotationSide, AnnotationStatus, ApiError, ApiErrorCode,
    ChangesAvailability, ContextSnapshot, CreateTaskResponse, DiffDocument, ExactContentBlock,
    FileContent, ReviewSnapshot, ReviewSubmissionPage, ReviewSubmissionResult, SubmitRunResponse,
    TaskChange, TaskProjection, TaskState, TimelineItemProjection, TimelinePage,
};
use kuku_server::run_manager::driver::{
    DriverEvent as RuntimeDriverEvent, DriverStart, RunDriverFactory,
};
use kuku_server::run_manager::{DomainError, TaskAggregate};
use kuku_server::testing::{BarrierOutcome, DriverEvent, ScenarioControl, ScenarioDriverFactory};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const STATUS_SKILL_PATH: &str = ".agents/skills/status/SKILL.md";
const STATUS_SKILL_CONTENT: &str = "---\nname: status\ndescription: Inspect status implementation\n---\n\nInspect status implementation and report facts.\n";
static SCENARIO_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FeatureScenario {
    name: String,
    provider: FeatureProviderInput,
    workspace: FeatureWorkspaceInput,
    task: FeatureTaskInput,
    review: FeatureReviewInput,
    barriers: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FeatureProviderInput {
    model: String,
    response: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FeatureWorkspaceInput {
    kind: FeatureWorkspaceKind,
    git_branch: Option<String>,
    baseline_files: Vec<FeatureFileInput>,
    working_files: Vec<FeatureFileInput>,
    revision_update: FeatureFileInput,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FeatureWorkspaceKind {
    Git,
    Directory,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FeatureFileInput {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FeatureTaskInput {
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FeatureReviewInput {
    path: String,
    side: AnnotationSide,
    start_line: u32,
    end_line: u32,
    excerpt: String,
    comment: String,
}

fn parse_feature_scenario(source: &str) -> Result<FeatureScenario, String> {
    let value: Value = serde_json::from_str(source).map_err(|error| error.to_string())?;
    reject_serialized_server_state(&value)?;
    serde_json::from_value(value).map_err(|error| error.to_string())
}

fn reject_serialized_server_state(value: &Value) -> Result<(), String> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                if matches!(
                    key.as_str(),
                    "projection"
                        | "ledger"
                        | "task_revision"
                        | "cursor"
                        | "timeline"
                        | "review_summary"
                ) {
                    return Err(format!("serialized server state is forbidden: {key}"));
                }
                reject_serialized_server_state(child)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                reject_serialized_server_state(item)?;
            }
        }
        Value::String(name) if matches!(name.as_str(), "TaskProjection" | "TaskLedgerRecord") => {
            return Err(format!("serialized server type is forbidden: {name}"));
        }
        _ => {}
    }
    Ok(())
}

#[derive(Default)]
struct FeatureProviderLog {
    requests: Mutex<Vec<Value>>,
}

#[derive(Clone)]
struct FeatureProviderState {
    log: Arc<FeatureProviderLog>,
    response: String,
}

struct FeatureProvider {
    addr: SocketAddr,
    log: Arc<FeatureProviderLog>,
    handle: tokio::task::JoinHandle<()>,
}

impl FeatureProvider {
    async fn start(response: String) -> Self {
        let log = Arc::new(FeatureProviderLog::default());
        let state = FeatureProviderState {
            log: log.clone(),
            response,
        };
        let app = Router::new()
            .route("/v1/messages", post(feature_provider_response))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { addr, log, handle }
    }

    fn port(&self) -> u16 {
        self.addr.port()
    }

    fn requests(&self) -> Vec<Value> {
        self.log.requests.lock().unwrap().clone()
    }
}

impl Drop for FeatureProvider {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn feature_provider_response(
    State(state): State<FeatureProviderState>,
    Json(request): Json<Value>,
) -> Response {
    let request_index = {
        let mut requests = state.log.requests.lock().unwrap();
        requests.push(request);
        requests.len()
    };
    let body = common::mock_provider::anthropic_sse_response(json!({
        "id": format!("msg_feature_{request_index}"),
        "type": "message",
        "role": "assistant",
        "content": [{"type": "text", "text": state.response}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 5, "output_tokens": 10}
    }));
    (
        [
            (header::CONTENT_TYPE, "text/event-stream"),
            (header::CONNECTION, "close"),
        ],
        body,
    )
        .into_response()
}

struct FeatureClient {
    base_url: String,
    client: wreq::Client,
}

impl FeatureClient {
    fn new(base_url: String) -> Self {
        Self {
            base_url,
            client: wreq::Client::new(),
        }
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> T {
        let response = self
            .client
            .get(format!("{}{path}", self.base_url))
            .header("authorization", format!("Bearer {TOKEN}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 200, "GET {path}");
        response.json().await.unwrap()
    }

    async fn post<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        expected_status: u16,
    ) -> T {
        let response = self.post_response(path, body).await;
        assert_eq!(response.status().as_u16(), expected_status, "POST {path}");
        response.json().await.unwrap()
    }

    async fn post_response<B: Serialize + ?Sized>(&self, path: &str, body: &B) -> wreq::Response {
        self.client
            .post(format!("{}{path}", self.base_url))
            .header("authorization", format!("Bearer {TOKEN}"))
            .json(body)
            .send()
            .await
            .unwrap()
    }
}

fn task_id(suffix: char) -> TaskId {
    let mut value = "tsk_0123456789abcdef0123456".to_owned();
    value.push(suffix);
    TaskId::parse(value).unwrap()
}

fn workspace_id() -> WorkspaceId {
    WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap()
}

fn control_record(events: Vec<TaskEvent>, revision: u64) -> TaskLedgerRecord {
    let receipt = CommandReceipt::new(
        format!("scenario-{revision}"),
        format!("digest-{revision}"),
        CommandResult::TaskCreated {
            task_id: task_id('a'),
        },
    )
    .unwrap();
    TaskLedgerRecord::Control(
        TaskTransaction::try_new(TaskRevision::try_new(revision).unwrap(), receipt, events)
            .unwrap(),
    )
}

fn created_record() -> TaskLedgerRecord {
    control_record(
        vec![TaskEvent::TaskCreated {
            task_id: task_id('a'),
            workspace_id: workspace_id(),
            title: "Scenario task".to_owned(),
            created_at: "2026-07-20T00:00:00Z".to_owned(),
        }],
        0,
    )
}

fn message_record(task_id: TaskId, text: String) -> TaskLedgerRecord {
    control_record(
        vec![TaskEvent::MessageAppended {
            message: MessageFact {
                message_id: "message-1".to_owned(),
                task_id,
                run_id: None,
                role: MessageRoleFact::Agent,
                text,
                finalized: true,
                request_ids: Vec::new(),
                file_references: Vec::new(),
            },
        }],
        1,
    )
}

fn fixture_text() -> String {
    let mut fixture = ScenarioDriverFactory::from_fixture("core_task", 7).unwrap();
    std::iter::from_fn(|| fixture.next_event())
        .find_map(|(_, event)| match event {
            DriverEvent::ProviderResponse { text } => Some(text),
            _ => None,
        })
        .unwrap()
}

fn write_feature_file(root: &Path, file: &FeatureFileInput) {
    let relative = Path::new(&file.path);
    assert!(!relative.is_absolute(), "fixture path must be relative");
    assert!(
        relative
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_))),
        "fixture path must stay within the workspace"
    );
    let destination = root.join(relative);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(destination, &file.content).unwrap();
}

fn run_git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn materialize_workspace(root: &Path, input: &FeatureWorkspaceInput) {
    if input.kind == FeatureWorkspaceKind::Git {
        run_git(root, &["init", "--quiet"]);
        run_git(root, &["config", "user.email", "scenario@example.invalid"]);
        run_git(root, &["config", "user.name", "Scenario Test"]);
        let branch = input.git_branch.as_deref().unwrap();
        run_git(root, &["checkout", "--quiet", "-b", branch]);
    } else {
        assert!(input.git_branch.is_none());
        assert!(input.baseline_files.is_empty());
    }

    for file in &input.baseline_files {
        write_feature_file(root, file);
    }
    if input.kind == FeatureWorkspaceKind::Git {
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "--quiet", "-m", "scenario baseline"]);
    }
    for file in &input.working_files {
        write_feature_file(root, file);
    }
}

async fn initialize_feature_server(
    client: &FeatureClient,
    server: &common::TestServer,
    scenario: &FeatureScenario,
    provider_port: u16,
) -> String {
    let status: Value = client.get("/api/v1/status").await;
    let mut revision = status["init"]["server_revision"]
        .as_str()
        .unwrap()
        .to_owned();
    let providers: Value = client
        .post(
            "/api/v1/init/providers",
            &json!({
                "providers": [{
                    "provider_id": "scenario",
                    "format": "anthropic",
                    "base_url": format!("http://127.0.0.1:{provider_port}"),
                    "credential": {"source": "direct_value", "value": "scenario-key"}
                }],
                "tiers": [{
                    "tier_id": "balanced",
                    "provider_id": "scenario",
                    "model": scenario.provider.model,
                    "purpose": "balanced",
                    "think": null
                }],
                "expected_revision": revision
            }),
            200,
        )
        .await;
    revision = providers["server_revision"].as_str().unwrap().to_owned();
    let tier: Value = client
        .post(
            "/api/v1/init/default-tier",
            &json!({"tier_id": "balanced", "expected_revision": revision}),
            200,
        )
        .await;
    revision = tier["server_revision"].as_str().unwrap().to_owned();
    let roots: Value = client.get("/api/v1/registration-roots").await;
    let relative_path = server
        .workspace
        .path()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    let workspace: Value = client
        .post(
            "/api/v1/init/workspace",
            &json!({
                "workspace": {
                    "root_id": roots["items"][0]["root_id"],
                    "relative_path": relative_path,
                    "label": scenario.name,
                    "expected_revision": revision
                }
            }),
            200,
        )
        .await;
    revision = workspace["server_revision"].as_str().unwrap().to_owned();
    let _: Value = client
        .post(
            "/api/v1/init/test",
            &json!({"tier_id": "balanced", "expected_revision": revision}),
            200,
        )
        .await;
    let status: Value = client.get("/api/v1/init/status").await;
    let _: Value = client
        .post(
            "/api/v1/init/complete",
            &json!({"expected_revision": status["server_revision"]}),
            200,
        )
        .await;
    let workspaces: Value = client.get("/api/v1/workspaces").await;
    workspaces["items"][0]["workspace_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn wait_for_terminal_projection(client: &FeatureClient, task_id: &TaskId) -> TaskProjection {
    for _ in 0..200 {
        let projection: TaskProjection = client.get(&format!("/api/v1/tasks/{task_id}")).await;
        if matches!(
            projection.task.state,
            TaskState::Completed | TaskState::Stopped | TaskState::Failed | TaskState::Interrupted
        ) {
            return projection;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("feature scenario task did not reach a terminal state");
}

fn exact_request_contains(snapshot: &ContextSnapshot, expected: &str) -> bool {
    snapshot
        .exact_request
        .as_ref()
        .into_iter()
        .flat_map(|request| &request.messages)
        .flat_map(|message| &message.content)
        .any(|content| {
            matches!(content, ExactContentBlock::Text { text } if text.contains(expected))
        })
}

fn json_contains(value: &Value, expected: &str) -> bool {
    match value {
        Value::String(text) => text.contains(expected),
        Value::Array(items) => items.iter().any(|item| json_contains(item, expected)),
        Value::Object(object) => object.values().any(|item| json_contains(item, expected)),
        _ => false,
    }
}

async fn current_review_revision(
    client: &FeatureClient,
    scenario: &FeatureScenario,
    workspace_id: &str,
) -> kuku_server::api::RevisionToken {
    let content: FileContent = client
        .get(&format!(
            "/api/v1/workspaces/{workspace_id}/files/content?path={}&start_line=1&end_line=100",
            scenario.review.path
        ))
        .await;
    assert!(
        content
            .text
            .as_deref()
            .unwrap()
            .contains(&scenario.review.excerpt),
        "{} file read omitted the annotated excerpt",
        scenario.name
    );

    let changes: ReviewSnapshot = client
        .get(&format!(
            "/api/v1/workspaces/{workspace_id}/changes?limit=100"
        ))
        .await;
    match scenario.workspace.kind {
        FeatureWorkspaceKind::Git => {
            assert_eq!(ChangesAvailability::Available, changes.availability);
            let entry = changes
                .entries
                .iter()
                .find(|entry| entry.path == scenario.review.path)
                .unwrap();
            let diff: DiffDocument = client
                .get(&format!(
                    "/api/v1/workspaces/{workspace_id}/changes/diff?path={}&revision={}&limit=100",
                    scenario.review.path, entry.revision
                ))
                .await;
            assert_eq!(entry.revision, diff.revision);
            assert!(diff
                .hunks
                .iter()
                .flat_map(|hunk| &hunk.lines)
                .any(|line| line.text == scenario.review.excerpt));
            entry.revision.clone()
        }
        FeatureWorkspaceKind::Directory => {
            assert_eq!(ChangesAvailability::NotGitRepository, changes.availability);
            assert!(changes.entries.is_empty());
            content.revision
        }
    }
}

fn annotation_batch(
    scenario: &FeatureScenario,
    task_revision: TaskRevision,
    revision: kuku_server::api::RevisionToken,
    idempotency_key: &str,
) -> AnnotationBatch {
    AnnotationBatch {
        expected_task_revision: task_revision,
        idempotency_key: idempotency_key.to_owned(),
        notes: vec![AnnotationDraft {
            path: scenario.review.path.clone(),
            revision,
            side: scenario.review.side,
            start_line: scenario.review.start_line,
            end_line: scenario.review.end_line,
            excerpt: scenario.review.excerpt.clone(),
            comment: scenario.review.comment.clone(),
        }],
    }
}

async fn run_feature_scenario(scenario: FeatureScenario) {
    assert!(scenario.barriers.iter().any(|name| name == "after-tool"));
    let provider = FeatureProvider::start(scenario.provider.response.clone()).await;
    let server = common::TestServer::start_unconfigured_with_token(Some(TOKEN.to_owned())).await;
    materialize_workspace(server.workspace.path(), &scenario.workspace);
    let client = FeatureClient::new(server.base_url.clone());
    let workspace_id =
        initialize_feature_server(&client, &server, &scenario, provider.port()).await;

    let created: CreateTaskResponse = client
        .post(
            "/api/v1/tasks",
            &json!({
                "workspace_id": workspace_id,
                "idempotency_key": format!("{}-create", scenario.name)
            }),
            201,
        )
        .await;
    let task_id = created.projection.task.task_id.clone();
    let _: SubmitRunResponse = client
        .post(
            &format!("/api/v1/tasks/{task_id}/runs"),
            &json!({
                "expected_task_revision": created.projection.task_revision,
                "idempotency_key": format!("{}-run", scenario.name),
                "message": scenario.task.message,
                "tier_id": "tier:balanced",
                "skill_ids": []
            }),
            202,
        )
        .await;
    let terminal = wait_for_terminal_projection(&client, &task_id).await;

    let first_context: ContextSnapshot = client
        .get(&format!("/api/v1/tasks/{task_id}/context"))
        .await;
    assert_eq!(1, first_context.request_history.len());
    assert_eq!(
        scenario.provider.model,
        first_context
            .exact_request
            .as_ref()
            .unwrap()
            .parameters
            .model
    );
    assert!(exact_request_contains(
        &first_context,
        &scenario.task.message
    ));
    let first_request_id = first_context
        .selected_request
        .as_ref()
        .unwrap()
        .request_id
        .clone();
    let historical: ContextSnapshot = client
        .get(&format!(
            "/api/v1/tasks/{task_id}/context/{first_request_id}"
        ))
        .await;
    assert_eq!(first_context, historical);

    let stale_revision = current_review_revision(&client, &scenario, &workspace_id).await;
    assert_eq!(
        scenario.review.path,
        scenario.workspace.revision_update.path
    );
    write_feature_file(server.workspace.path(), &scenario.workspace.revision_update);
    let stale = annotation_batch(
        &scenario,
        terminal.task_revision,
        stale_revision,
        &format!("{}-stale-review", scenario.name),
    );
    let stale_response = client
        .post_response(
            &format!("/api/v1/tasks/{task_id}/review/annotations"),
            &stale,
        )
        .await;
    assert_eq!(409, stale_response.status().as_u16());
    let stale_error: ApiError = stale_response.json().await.unwrap();
    assert_eq!(ApiErrorCode::Outdated, stale_error.code());

    let current_revision = current_review_revision(&client, &scenario, &workspace_id).await;
    let batch = annotation_batch(
        &scenario,
        terminal.task_revision,
        current_revision.clone(),
        &format!("{}-review", scenario.name),
    );
    let submitted: ReviewSubmissionResult = client
        .post(
            &format!("/api/v1/tasks/{task_id}/review/annotations"),
            &batch,
            201,
        )
        .await;
    assert!(!submitted.replayed);
    assert_eq!(current_revision, submitted.submission.notes[0].revision);
    assert_eq!(
        AnnotationStatus::Current,
        submitted.submission.notes[0].status
    );

    let after_follow_up = wait_for_terminal_projection(&client, &task_id).await;
    assert_eq!(1, after_follow_up.review_summary.total_submissions);
    assert_eq!(
        Some(&submitted.submission.run_id),
        after_follow_up.task.latest_run_id.as_ref()
    );
    let submissions: ReviewSubmissionPage = client
        .get(&format!(
            "/api/v1/tasks/{task_id}/review/submissions?limit=50"
        ))
        .await;
    assert_eq!(1, submissions.items.len());
    assert_eq!(
        scenario.review.comment,
        submissions.items[0].notes[0].comment
    );

    let follow_up_context: ContextSnapshot = client
        .get(&format!("/api/v1/tasks/{task_id}/context"))
        .await;
    assert_eq!(2, follow_up_context.request_history.len());
    assert!(exact_request_contains(
        &follow_up_context,
        &scenario.review.comment
    ));
    let preserved_first: ContextSnapshot = client
        .get(&format!(
            "/api/v1/tasks/{task_id}/context/{first_request_id}"
        ))
        .await;
    assert_eq!(
        first_context.exact_request, preserved_first.exact_request,
        "historical exact request changed after Review follow-up"
    );

    let provider_requests = provider.requests();
    assert!(provider_requests.len() >= 3);
    assert!(json_contains(
        provider_requests.last().unwrap(),
        &scenario.review.comment
    ));
}

#[test]
fn scenario_fixture_is_provider_input_not_a_projection_script() {
    let factory = ScenarioDriverFactory::from_fixture("core_task", 7).unwrap();
    assert!(!factory.fixture_source().contains("TaskProjection"));
    assert!(!factory.fixture_source().contains("ledger"));
}

#[test]
fn feature_scenarios_are_input_fragments_not_serialized_server_state() {
    for source in [FULL_TASK_FIXTURE, HUMAN_ACCEPTANCE_FIXTURE] {
        parse_feature_scenario(source).unwrap();
        assert!(!source.contains("TaskProjection"));
        assert!(!source.contains("TaskLedgerRecord"));
        assert!(!source.contains("\"projection\""));
        assert!(!source.contains("\"ledger\""));
    }
    assert!(parse_feature_scenario(r#"{"projection":{"task_revision":1}}"#).is_err());
    assert!(parse_feature_scenario(r#"{"kind":"TaskLedgerRecord"}"#).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn feature_scenarios_drive_real_context_and_review_services() {
    let _environment = SCENARIO_ENV_LOCK.lock().await;
    for source in [FULL_TASK_FIXTURE, HUMAN_ACCEPTANCE_FIXTURE] {
        run_feature_scenario(parse_feature_scenario(source).unwrap()).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scenario_uses_commands_runtime_ledger_and_authenticated_controls() {
    let _environment = SCENARIO_ENV_LOCK.lock().await;
    std::env::set_var("KUKU_TEST_SCENARIO", "full_task");
    std::env::set_var("KUKU_TEST_SEED", "7");
    let server = common::TestServer::start_unconfigured_with_token(Some(TOKEN.to_owned())).await;
    std::env::remove_var("KUKU_TEST_SCENARIO");
    std::env::remove_var("KUKU_TEST_SEED");

    let scenario = parse_feature_scenario(FULL_TASK_FIXTURE).unwrap();
    materialize_workspace(server.workspace.path(), &scenario.workspace);
    let client = FeatureClient::new(server.base_url.clone());
    let unauthenticated = wreq::Client::new()
        .post(format!(
            "{}/api/v1/testing/barriers/after-tool/release",
            server.base_url
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(401, unauthenticated.status().as_u16());

    let workspace_id = initialize_feature_server(&client, &server, &scenario, 1).await;
    let created: CreateTaskResponse = client
        .post(
            "/api/v1/tasks",
            &json!({
                "workspace_id": workspace_id,
                "idempotency_key": "scenario-runtime-create"
            }),
            201,
        )
        .await;
    let task_id = created.projection.task.task_id;
    let _: SubmitRunResponse = client
        .post(
            &format!("/api/v1/tasks/{task_id}/runs"),
            &json!({
                "expected_task_revision": created.projection.task_revision,
                "idempotency_key": "scenario-runtime-run",
                "message": scenario.task.message,
                "tier_id": "tier:balanced",
                "skill_ids": []
            }),
            202,
        )
        .await;

    let after_tool = client
        .post_response("/api/v1/testing/barriers/after-tool/release", &json!({}))
        .await;
    assert_eq!(204, after_tool.status().as_u16());

    let mut pending = None;
    let mut last_observed = None;
    for _ in 0..200 {
        let projection: TaskProjection = client.get(&format!("/api/v1/tasks/{task_id}")).await;
        last_observed = Some((
            projection.task.state,
            projection.cursor,
            projection.timeline.len(),
        ));
        if let Some(interaction) = projection.timeline.iter().find_map(|item| match item {
            TimelineItemProjection::Interaction(interaction)
                if interaction.selected_choice_id.is_none() =>
            {
                Some(interaction.clone())
            }
            _ => None,
        }) {
            assert_eq!(TaskState::NeedsAttention, projection.task.state);
            pending = Some((projection.task_revision, interaction));
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let pending = pending.unwrap_or_else(|| {
        panic!("feature scenario exposed no pending interaction; last={last_observed:?}")
    });
    assert_eq!("Permission request", pending.1.prompt);
    let _: Value = client
        .post(
            &format!(
                "/api/v1/tasks/{task_id}/interactions/{}",
                pending.1.interaction_id
            ),
            &json!({
                "expected_task_revision": pending.0,
                "idempotency_key": "scenario-runtime-interaction",
                "choice_id": pending.1.choices[0].choice_id
            }),
            202,
        )
        .await;
    let continuity = client
        .post_response(
            "/api/v1/testing/barriers/continuity-before-finish/release",
            &json!({}),
        )
        .await;
    assert_eq!(204, continuity.status().as_u16());
    let terminal = wait_for_terminal_projection(&client, &task_id).await;
    assert_eq!(TaskState::Completed, terminal.task.state);
    assert!(terminal.cursor.get() > created.projection.cursor.get());
    assert!(terminal
        .loaded_skills
        .iter()
        .any(|skill| { skill.skill_id == "skill:project:status" && skill.loaded_by == "agent" }));

    let context: ContextSnapshot = client
        .get(&format!("/api/v1/tasks/{task_id}/context"))
        .await;
    assert_eq!(1, context.request_history.len());
    assert!(exact_request_contains(&context, &scenario.task.message));
    let expected_skill_hash = format!("sha256:{:x}", Sha256::digest(STATUS_SKILL_CONTENT));
    assert_eq!(1, context.sections.skills.len());
    assert_eq!("skill:project:status", context.sections.skills[0].skill_id);
    assert_eq!(
        kuku::event::SkillLoadOrigin::Agent,
        context.sections.skills[0].origin
    );
    assert_eq!(expected_skill_hash, context.sections.skills[0].content_hash);
    assert_eq!(
        kuku::event::SourceScope::Project,
        context.sections.skills[0].source.scope
    );
    assert_eq!(
        "skill-source:project:status",
        context.sections.skills[0].source.id
    );
    assert_eq!(
        Some(STATUS_SKILL_PATH),
        context.sections.skills[0]
            .source
            .relative_path
            .as_ref()
            .map(kuku::event::WorkspaceRelativePath::as_str)
    );
    let request_id = &context.request_history[0].request_id;
    let historical: ContextSnapshot = client
        .get(&format!("/api/v1/tasks/{task_id}/context/{request_id}"))
        .await;
    assert_eq!(context.sections.skills, historical.sections.skills);
    let expected_file_path = &scenario.workspace.working_files[0].path;
    assert_eq!(1, context.sections.observations.len());
    assert_eq!(
        Some(expected_file_path.as_str()),
        context.sections.observations[0]
            .relative_path
            .as_ref()
            .map(kuku::event::WorkspaceRelativePath::as_str)
    );

    let mut before = None;
    let mut history_ids = std::collections::BTreeSet::new();
    let mut tool_file_reference = None;
    loop {
        let path = before.as_ref().map_or_else(
            || format!("/api/v1/tasks/{task_id}/timeline?limit=500"),
            |cursor: &kuku_server::api::PageCursor| {
                format!(
                    "/api/v1/tasks/{task_id}/timeline?limit=500&before={}",
                    cursor.as_str()
                )
            },
        );
        let page: TimelinePage = client.get(&path).await;
        for item in page.items {
            if let TimelineItemProjection::Activity(activity) = item {
                if activity.activity_id.starts_with("scenario-history-7-") {
                    assert!(history_ids.insert(activity.activity_id));
                } else if activity.activity_id.starts_with("scenario-tool-7-") {
                    assert_eq!(1, activity.file_references.len());
                    tool_file_reference = Some(activity.file_references[0].relative_path.clone());
                }
            }
        }
        before = page.next_cursor;
        if before.is_none() {
            break;
        }
    }
    assert_eq!(10_000, history_ids.len());
    assert_eq!(Some(expected_file_path.clone()), tool_file_reference);
}

#[test]
fn equal_seed_produces_equal_deterministic_driver_events() {
    let mut left = ScenarioDriverFactory::from_fixture("core_task", 7).unwrap();
    let mut right = ScenarioDriverFactory::from_fixture("core_task", 7).unwrap();
    let left_events: Vec<_> = std::iter::from_fn(|| left.next_event()).collect();
    let right_events: Vec<_> = std::iter::from_fn(|| right.next_event()).collect();
    assert_eq!(left_events, right_events);
}

#[tokio::test]
async fn control_releases_only_declared_barriers() {
    let control = ScenarioControl::new(["after-tool"]);
    control.release("after-tool").await.unwrap();
    assert_eq!(
        control.wait("after-tool").await.unwrap(),
        BarrierOutcome::Released
    );
}

#[tokio::test]
async fn scenario_factory_drives_the_production_driver_contract() {
    fn assert_driver_factory<T: RunDriverFactory>() {}
    assert_driver_factory::<ScenarioDriverFactory>();

    let directory = tempfile::tempdir().unwrap();
    let event_store = kuku::event::EventStore::open(directory.path().join("events.jsonl")).unwrap();
    let mut ids = kuku_server::testing::ScenarioIds::seeded(7);
    let start = DriverStart {
        task_id: ids.task_id(),
        run_id: ids.run_id(),
        workspace_id: ids.workspace_id(),
        prompt: "Run the deterministic scenario".to_owned(),
        tier_id: "tier:default".to_owned(),
        selected_skills: Vec::new(),
        agent_message_id: "msg_agent_scenario".to_owned(),
        execution_scope: kuku::event::ExecutionScope {
            workspace_id: ids.workspace_id(),
            task_id: ids.task_id(),
            run_id: ids.run_id(),
            turn_id: ids.turn_id(),
            conversation_id: ids.conversation_id(),
            turn_index: 1,
        },
        event_store,
    };
    let factory = ScenarioDriverFactory::from_fixture("core_task", 7).unwrap();
    let control = factory.control();
    let mut handle = factory.start(start).await.unwrap();

    assert_eq!(
        handle.events.recv().await,
        Some(RuntimeDriverEvent::Started)
    );
    while let Some(event) = handle.events.recv().await {
        if matches!(event, RuntimeDriverEvent::Activity(_)) {
            break;
        }
    }
    control.release("after-tool").await.unwrap();
}

#[test]
fn feature_scenarios_include_required_runtime_inputs() {
    for name in ["full_task", "human_acceptance"] {
        let factory = ScenarioDriverFactory::from_fixture(name, 11).unwrap();
        assert_eq!(factory.fixture().name, name);
        assert!(factory
            .fixture()
            .barriers
            .iter()
            .any(|name| name == "after-tool"));
        assert!(factory
            .fixture()
            .barriers
            .iter()
            .any(|name| name == "continuity-before-finish"));

        let tool_position = factory
            .fixture()
            .events
            .iter()
            .position(|event| matches!(event, DriverEvent::ToolCall { .. }))
            .unwrap();
        let interaction_position = factory
            .fixture()
            .events
            .iter()
            .position(|event| matches!(event, DriverEvent::Interaction { .. }))
            .unwrap();
        assert!(interaction_position > tool_position);

        let skill = factory
            .fixture()
            .events
            .iter()
            .find_map(|event| match event {
                DriverEvent::AgentSkillLoaded { skill_id, source } => Some((skill_id, source)),
                _ => None,
            })
            .unwrap();
        assert_eq!("skill:project:status", skill.0);
        assert_eq!(STATUS_SKILL_PATH, skill.1.relative_path.as_str());
        assert_eq!(STATUS_SKILL_CONTENT, skill.1.content);

        let fixture = serde_json::to_value(factory.fixture()).unwrap();
        let history = fixture["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["kind"] == "timeline_history")
            .unwrap();
        assert_eq!(history["count"], 10_000);
        assert!(history["batch_size"].as_u64().unwrap() <= 250);
    }
}

#[test]
fn ledger_replay_rebuilds_projection_before_newer_record() {
    let records = [
        (Cursor::try_new(1).unwrap(), created_record()),
        (
            Cursor::try_new(3).unwrap(),
            message_record(task_id('a'), fixture_text()),
        ),
    ];
    let mut before_replay = TaskAggregate::default();
    for (cursor, record) in &records {
        before_replay.apply_record(*cursor, record).unwrap();
    }
    let projection_before_replay = before_replay.projection().unwrap();

    let mut replayed = TaskAggregate::default();
    for (cursor, record) in &records {
        replayed.apply_record(*cursor, record).unwrap();
    }
    assert_eq!(replayed.projection().unwrap(), projection_before_replay);

    let next_cursor = Cursor::try_new(7).unwrap();
    let changes = replayed
        .apply_record(
            next_cursor,
            &TaskLedgerRecord::Activity(
                TaskActivityBatch::try_new(vec![TaskEvent::MessagePatched {
                    message_id: "message-1".to_owned(),
                    append_text: " Done.".to_owned(),
                    finalized: true,
                    request_ids: None,
                }])
                .unwrap(),
            ),
        )
        .unwrap();
    assert!(replayed.cursor().get() > projection_before_replay.cursor.get());
    assert_eq!(replayed.cursor(), next_cursor);
    assert!(matches!(
        changes.as_slice(),
        [TaskChange::MessagePatched { .. }]
    ));
}

#[test]
fn reducer_uses_record_cursor_for_new_timeline_items() {
    let mut aggregate = TaskAggregate::default();
    aggregate
        .apply_record(Cursor::try_new(1).unwrap(), &created_record())
        .unwrap();
    let cursor = Cursor::try_new(3).unwrap();
    let changes = aggregate
        .apply_record(cursor, &message_record(task_id('a'), fixture_text()))
        .unwrap();
    assert!(matches!(
        changes.as_slice(),
        [TaskChange::MessageAppended { item }]
            if matches!(item, TimelineItemProjection::Message(message) if message.order_key == cursor)
    ));
}

#[test]
fn stale_cursor_is_rejected_before_projection_mutation() {
    let mut aggregate = TaskAggregate::default();
    let cursor = Cursor::try_new(1).unwrap();
    aggregate.apply_record(cursor, &created_record()).unwrap();
    let before = aggregate.projection().unwrap();
    let result = aggregate.apply_record(cursor, &message_record(task_id('a'), fixture_text()));
    assert!(matches!(result, Err(DomainError::LedgerCorrupt)));
    assert_eq!(aggregate.projection().unwrap(), before);
}

#[test]
fn cross_task_record_is_rejected_before_cursor_advances() {
    let mut aggregate = TaskAggregate::default();
    aggregate
        .apply_record(Cursor::try_new(1).unwrap(), &created_record())
        .unwrap();
    let before = aggregate.projection().unwrap();
    let result = aggregate.apply_record(
        Cursor::try_new(2).unwrap(),
        &control_record(
            vec![
                TaskEvent::MessageAppended {
                    message: MessageFact {
                        message_id: "local-message".to_owned(),
                        task_id: task_id('a'),
                        run_id: None,
                        role: MessageRoleFact::Agent,
                        text: "must roll back".to_owned(),
                        finalized: true,
                        request_ids: Vec::new(),
                        file_references: Vec::new(),
                    },
                },
                TaskEvent::MessageAppended {
                    message: MessageFact {
                        message_id: "foreign-message".to_owned(),
                        task_id: task_id('b'),
                        run_id: None,
                        role: MessageRoleFact::Agent,
                        text: "foreign task".to_owned(),
                        finalized: true,
                        request_ids: Vec::new(),
                        file_references: Vec::new(),
                    },
                },
            ],
            1,
        ),
    );
    assert!(matches!(result, Err(DomainError::LedgerCorrupt)));
    assert_eq!(aggregate.projection().unwrap(), before);
}
