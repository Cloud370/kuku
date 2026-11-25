mod common;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kuku::event::{EventPayload, ModelStopReason};
use kuku::permission::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer};
use kuku::{
    AdapterFuture, AgentConfig, AgentSnapshot, AgentStatus, Input, ModelAdapter, ModelEvent,
    ModelRequest, ModelStream, Session, SessionOptions,
};
use serde_json::{Value, json};

#[path = "docs_skill/lifecycle.rs"]
mod lifecycle;

const NAME: &str = "kuku-docs";
const TOOL: &str = "skill_resources";
const SKILL_BODY: &str = "Identify the running kuku version if known";
const INDEX: &str = "references/docs/en/index.md";

#[derive(Debug)]
enum Reply {
    Text,
    Hold(
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    ),
    Call(&'static str, &'static str, Value),
    Batch(Vec<(&'static str, &'static str, Value)>),
}

#[derive(Debug)]
struct Model {
    replies: Mutex<VecDeque<Reply>>,
    requests: Mutex<Vec<(Value, bool)>>,
    cache_root: PathBuf,
}

impl Model {
    fn new(data: &Path, replies: impl IntoIterator<Item = Reply>) -> Arc<Self> {
        Arc::new(Self {
            cache_root: data.join("skill-resources").join(NAME),
            replies: Mutex::new(replies.into_iter().collect()),
            requests: Mutex::default(),
        })
    }

    fn extracted(&self) -> bool {
        std::fs::read_dir(&self.cache_root)
            .ok()
            .into_iter()
            .flatten()
            .any(|entry| {
                let entry = entry.unwrap();
                entry.file_name().len() == 64 && entry.file_type().unwrap().is_dir()
            })
    }
}

impl ModelAdapter for Model {
    fn stream(&self, request: ModelRequest) -> AdapterFuture<'_, ModelStream> {
        Box::pin(async move {
            self.requests
                .lock()
                .unwrap()
                .push((request.body, self.extracted()));
            let reply = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected request");
            let mut events = vec![ModelEvent::StreamStart {
                request_id: "docs-fixture".into(),
            }];
            let calls = match reply {
                Reply::Text => Vec::new(),
                Reply::Hold(started, resume) => {
                    started.send(()).expect("fixture is waiting");
                    resume.await.expect("fixture released the request");
                    Vec::new()
                }
                Reply::Call(id, name, args) => vec![(id, name, args)],
                Reply::Batch(calls) => calls,
            };
            let reason = if calls.is_empty() {
                events.push(ModelEvent::TextDelta {
                    text: "fixture reply".into(),
                });
                ModelStopReason::EndTurn
            } else {
                for (index, (id, name, args)) in (0_u64..).zip(calls) {
                    events.extend([
                        ModelEvent::ToolCallStart {
                            index,
                            id: id.into(),
                            name: name.into(),
                        },
                        ModelEvent::ToolCallArgDelta {
                            index,
                            fragment: args.to_string(),
                        },
                        ModelEvent::ToolCallStop { index },
                    ]);
                }
                ModelStopReason::ToolUse
            };
            events.push(ModelEvent::StopReason { reason });
            events.push(ModelEvent::StreamEnd);
            Ok(Box::pin(tokio_stream::iter(events.into_iter().map(Ok))) as ModelStream)
        })
    }
}

#[derive(Debug, Default)]
struct Gate(Mutex<Vec<ToolAuthorization>>);

impl ToolAuthorizer for Gate {
    fn authorize(&self, action: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        self.0.lock().unwrap().push(action.clone());
        Ok(match action.tool_call_id.as_str() {
            "denied" => AuthorizationDecision::Deny,
            "defer" => AuthorizationDecision::Defer,
            _ => AuthorizationDecision::Allow,
        })
    }
}

fn options(workspace: &Path, data: &Path, model: Arc<Model>, gate: Arc<Gate>) -> SessionOptions {
    let mut config = common::test_config();
    let model_config = common::set_test_model(&mut config, "openai", "docs-fixture");
    model_config.think = kuku::config::ThinkLevel::Off;
    model_config.max_output_tokens = Some(256);
    config.plugin.enabled = false;
    config.discovery.auto_discover = false;
    config.handoff.enabled = false;
    let mut options = SessionOptions::new(workspace, data, config);
    options.model_adapter = Some(model);
    options.authorizer = gate;
    options
}

fn docs_archive_lock(options: &SessionOptions, model: &Model) -> (PathBuf, fslock::LockFile) {
    let registry = kuku::skill::build_registry_snapshot_for_host(
        &options.data_dir,
        &options.workspace,
        &options.config,
    )
    .unwrap();
    let registry = serde_json::to_value(registry).unwrap();
    let version = registry["docs_bundle"].as_str().unwrap();
    std::fs::create_dir_all(&model.cache_root).unwrap();
    let archive = model.cache_root.join(version).with_extension("bundle");
    let lock = fslock::LockFile::open(&archive.with_extension("lock")).unwrap();
    (archive, lock)
}

async fn settled(session: &Session, status: AgentStatus) -> AgentSnapshot {
    let snapshot = tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let snapshot = session.agent("main").unwrap();
            if snapshot.status != AgentStatus::Running {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("docs Agent should settle");
    assert_eq!(snapshot.status, status, "{:?}", snapshot.last_error);
    snapshot
}

fn tool_result(session: &Session, id: &str) -> (String, String, Value) {
    session
        .events(0, 512)
        .unwrap()
        .into_iter()
        .find_map(|event| match event.payload {
            EventPayload::ToolResult {
                tool_call_id,
                status,
                model_content,
                structured,
                ..
            } if tool_call_id == id => {
                Some((status, model_content, structured.unwrap_or(Value::Null)))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing result for {id}"))
}

fn system_messages(body: &Value) -> Vec<&Value> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "system")
        .collect()
}

#[tokio::test]
async fn help_only_admits_requested_skill_material_and_resource_calls_obey_scope_and_authorization()
{
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let model = Model::new(
        data.path(),
        [
            Reply::Text,
            Reply::Call(
                "unloaded",
                TOOL,
                json!({"skill_name": NAME,"action":"read","path":INDEX,"limit":1}),
            ),
            Reply::Call("load", "use_skill", json!({"skill_name":NAME})),
            Reply::Call("denied", TOOL, json!({"skill_name": NAME,"action":"list"})),
            Reply::Call(
                "outside",
                TOOL,
                json!({"skill_name":NAME,"action":"read","path":"references/docs/../../Cargo.toml"}),
            ),
            Reply::Call(
                "list",
                TOOL,
                json!({"skill_name": NAME,"action":"list","path":"references/docs/en/","limit":1}),
            ),
            Reply::Call(
                "search",
                TOOL,
                json!({"skill_name": NAME,"action":"search","path":"references/docs/en/","query":"SESSION","limit":1}),
            ),
            Reply::Call(
                "read",
                TOOL,
                json!({"skill_name":NAME,"action":"read","path":INDEX,"limit":2}),
            ),
            Reply::Call("workspace", "read_file", json!({"path":INDEX})),
            Reply::Text,
        ],
    );
    let gate = Arc::new(Gate::default());
    let session = Session::open(options(
        workspace.path(),
        data.path(),
        model.clone(),
        gate.clone(),
    ))
    .await
    .unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("ordinary", "main", "Say hello."))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;
    assert!(!model.cache_root.exists());
    session
        .send(Input::new("help", "main", "How do I use Sessions in kuku?"))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;

    let (status, unloaded, _) = tool_result(&session, "unloaded");
    assert_eq!(status, "ok");
    assert!(unloaded.contains("Lines 1"));
    let (status, instructions, loaded) = tool_result(&session, "load");
    assert_eq!(status, "ok");
    assert!(instructions.contains(SKILL_BODY));
    assert_eq!(loaded["source"], "builtin");
    assert_eq!(tool_result(&session, "denied").0, "blocked");
    assert_eq!(tool_result(&session, "outside").0, "error");
    assert_eq!(tool_result(&session, "workspace").0, "error");

    let (status, _, list) = tool_result(&session, "list");
    assert_eq!(status, "ok");
    assert_eq!(list["next_offset"], 1);
    assert!(
        list["files"][0]["path"]
            .as_str()
            .unwrap()
            .starts_with("references/docs/en/")
    );
    let (status, _, search) = tool_result(&session, "search");
    assert_eq!(status, "ok");
    assert!(
        search["matches"][0]["excerpt"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("session")
    );
    let (status, page, read) = tool_result(&session, "read");
    assert_eq!(status, "ok");
    assert_eq!(read["version"], loaded["version"]);
    assert_eq!(read["source_revision"], kuku::BUILD_REVISION);
    assert_eq!(read["path"], INDEX);
    assert_eq!(read["next_offset"], 3);
    assert!(page.contains("Lines 1–2"));
    let actions = gate.0.lock().unwrap().clone();
    assert!(actions.iter().any(|action| action.tool.name == TOOL
        && action.args["path"] == INDEX
        && action.workspace == std::fs::canonicalize(workspace.path()).unwrap()));

    let requests = model.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 10);
    let baseline = &requests[0].0;
    assert!(
        baseline
            .to_string()
            .contains("Use list_skills to browse available skills.")
    );
    assert!(
        requests[..3]
            .iter()
            .all(|(body, _)| !body.to_string().contains(SKILL_BODY))
    );
    assert!(requests[3].0.to_string().contains(SKILL_BODY));
    assert!(requests[..2].iter().all(|(_, extracted)| !extracted));
    assert!(requests[2].1);
    assert!(
        requests[..8]
            .iter()
            .all(|(body, _)| !body.to_string().contains("Lines 1–2"))
    );
    assert!(requests[8].0.to_string().contains("Lines 1–2"));
    assert!(
        requests
            .iter()
            .all(|(body, _)| body["tools"] == baseline["tools"]
                && system_messages(body) == system_messages(baseline))
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn explicit_skill_admission_survives_reopening_a_waiting_turn() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let model = Model::new(
        data.path(),
        [
            Reply::Call(
                "defer",
                TOOL,
                json!({"skill_name":NAME,"action":"read","path":INDEX}),
            ),
            Reply::Text,
        ],
    );
    let mut options = options(
        workspace.path(),
        data.path(),
        model.clone(),
        Arc::new(Gate::default()),
    );
    let session = Session::open(options.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    let mut input = Input::new("explicit", "main", "Explain this version of kuku.");
    input.bootstrap_skill = Some(NAME.into());
    session.send(input).unwrap();
    let waiting = settled(&session, AgentStatus::Waiting).await;
    let permission = waiting.permission.unwrap();
    assert!(model.cache_root.is_dir());
    assert!(!model.extracted());
    options.session_id = Some(session.id().into());
    session.close().await.unwrap();

    let session = Session::open(options).await.unwrap();
    session
        .reply_permission("approve", "main", &permission.id, true)
        .unwrap();
    settled(&session, AgentStatus::Idle).await;
    let (status, page, _) = tool_result(&session, "defer");
    assert_eq!(status, "ok");
    assert!(page.contains(INDEX));
    let requests = model.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    for (body, _) in &requests {
        assert_eq!(body.to_string().matches(SKILL_BODY).count(), 1);
    }
    assert!(!requests[0].1);
    assert!(requests[1].1);
    session.close().await.unwrap();
}

#[tokio::test]
async fn configured_docs_skill_remains_loaded_while_the_body_is_in_context() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let model = Model::new(
        data.path(),
        [
            Reply::Text,
            Reply::Call("loaded-list", "list_skills", json!({})),
            Reply::Call(
                "configured-read",
                TOOL,
                json!({"skill_name":NAME,"action":"read","path":INDEX}),
            ),
            Reply::Text,
            Reply::Call("unloaded-list", "list_skills", json!({})),
            Reply::Call("builtin-reuse", "use_skill", json!({"skill_name":NAME})),
            Reply::Call(
                "unloaded-read",
                TOOL,
                json!({"skill_name":NAME,"action":"read","path":INDEX}),
            ),
            Reply::Text,
        ],
    );
    let session = Session::open(options(
        workspace.path(),
        data.path(),
        model.clone(),
        Arc::new(Gate::default()),
    ))
    .await
    .unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("ordinary", "main", "Say hello."))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;
    session
        .configure_agent(
            "enable",
            "main",
            AgentConfig {
                bootstrap_skill: Some(NAME.into()),
                ..AgentConfig::default()
            },
        )
        .unwrap();
    session
        .send(Input::new("selected", "main", "How do I use this version?"))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;
    assert_eq!(tool_result(&session, "configured-read").0, "ok");
    assert_eq!(
        tool_result(&session, "loaded-list").2["skills"][0]["loaded"],
        true
    );

    session
        .configure_agent("disable", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("following", "main", "Say hello again."))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;
    assert_eq!(
        tool_result(&session, "unloaded-list").2["skills"][0]["loaded"],
        true
    );
    assert_eq!(tool_result(&session, "unloaded-read").0, "ok");
    assert_eq!(tool_result(&session, "builtin-reuse").2["reused"], true);
    let requests = model.requests.lock().unwrap().clone();
    assert!(!requests[0].0.to_string().contains(SKILL_BODY));
    assert!(
        requests[1..]
            .iter()
            .all(|(body, _)| body.to_string().matches(SKILL_BODY).count() == 1)
    );
    assert!(session.events(0, 512).unwrap().iter().all(|record| {
        !matches!(&record.payload, EventPayload::Unknown(value) if value["kind"] == "runtime.context_segment"
            && value["prelude"].to_string().contains(SKILL_BODY))
    }));
    session.close().await.unwrap();
}

#[tokio::test]
async fn cancelling_docs_io_while_the_version_is_locked_settles_the_call() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let model = Model::new(
        data.path(),
        [
            Reply::Call("held-load", "use_skill", json!({"skill_name":NAME})),
            Reply::Call("load", "use_skill", json!({"skill_name":NAME})),
            Reply::Text,
            Reply::Call(
                "held-read",
                TOOL,
                json!({"skill_name":NAME,"action":"read","path":INDEX}),
            ),
            Reply::Text,
        ],
    );
    let options = options(
        workspace.path(),
        data.path(),
        model.clone(),
        Arc::new(Gate::default()),
    );
    let (archive, mut lock) = docs_archive_lock(&options, &model);

    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    for call in ["held-load", "held-read"] {
        assert!(lock.try_lock().unwrap());
        let mut input = Input::new(call, "main", "Explain this version of kuku.");
        if call == "held-read" {
            input.bootstrap_skill = Some(NAME.into());
        }
        session.send(input).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let dispatched = session.events(0, 512).unwrap().iter().any(|event| {
                    matches!(&event.payload, EventPayload::Unknown(value)
                        if value["kind"] == "runtime.tool_dispatched" && value["tool_call_id"] == call)
                });
                if dispatched {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("resource operation should dispatch");
        let execution = session.agent("main").unwrap().execution_id.unwrap();
        tokio::time::timeout(
            Duration::from_secs(5),
            session.cancel_execution(&format!("cancel-{call}"), "main", execution),
        )
        .await
        .expect("cancellation should not wait for the cache lock")
        .unwrap();
        settled(&session, AgentStatus::Stopped).await;
        assert_eq!(tool_result(&session, call).0, "cancelled");
        assert!(!model.extracted());
        assert_eq!(archive.exists(), call == "held-read");
        lock.unlock().unwrap();

        session
            .send(Input::new(format!("continue-{call}"), "main", "Say hello."))
            .unwrap();
        assert_eq!(
            settled(&session, AgentStatus::Idle)
                .await
                .last_output
                .as_deref(),
            Some("fixture reply")
        );
        assert!(!model.extracted());
    }
    session.close().await.unwrap();
}

#[tokio::test]
async fn cancelling_a_first_explicit_docs_input_does_not_wait_for_the_version_lock() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let model = Model::new(data.path(), [Reply::Text, Reply::Text]);
    let options = options(
        workspace.path(),
        data.path(),
        model.clone(),
        Arc::new(Gate::default()),
    );
    let (archive, mut lock) = docs_archive_lock(&options, &model);
    assert!(lock.try_lock().unwrap());
    let (release_lock, wait_for_release) = std::sync::mpsc::channel();
    let watchdog = std::thread::spawn(move || {
        // Bound the test even if a regression blocks this single-thread Tokio runtime.
        let released = wait_for_release
            .recv_timeout(Duration::from_secs(12))
            .is_ok();
        drop(lock);
        released
    });

    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    let explicit_input = |command| {
        let mut input = Input::new(command, "main", "Explain this version of kuku.");
        input.bootstrap_skill = Some(NAME.into());
        input
    };
    session.send(explicit_input("first-docs-input")).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if session.events(0, 512).unwrap().iter().any(|event| {
                matches!(&event.payload, EventPayload::ContextSkills { conversation, .. } if conversation == "main")
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the explicit Turn should reach Skill admission");
    assert!(!archive.exists());
    let execution = session.agent("main").unwrap().execution_id.unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        session.cancel_execution("cancel-docs-input", "main", execution),
    )
    .await
    .expect("cancellation should not wait for a retained archive")
    .unwrap();
    settled(&session, AgentStatus::Stopped).await;
    assert!(model.requests.lock().unwrap().is_empty());

    session
        .send(Input::new("continue", "main", "Say hello."))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;
    assert!(!archive.exists());
    assert!(
        !model.requests.lock().unwrap()[0]
            .0
            .to_string()
            .contains(SKILL_BODY)
    );

    release_lock.send(()).unwrap();
    assert!(watchdog.join().unwrap());
    session.send(explicit_input("retry-docs-input")).unwrap();
    settled(&session, AgentStatus::Idle).await;
    assert!(archive.is_file());
    assert!(
        model.requests.lock().unwrap()[1]
            .0
            .to_string()
            .contains(SKILL_BODY)
    );
    assert!(!model.extracted());
    session.close().await.unwrap();
}

#[tokio::test]
async fn dropping_standalone_docs_start_releases_its_session_lock() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let model = Model::new(data.path(), []);
    let options = options(
        workspace.path(),
        data.path(),
        model.clone(),
        Arc::new(Gate::default()),
    );
    let (archive, mut lock) = docs_archive_lock(&options, &model);
    assert!(lock.try_lock().unwrap());
    let query = |prompt| {
        kuku::query(prompt)
            .session("docs-start-cancelled")
            .workspace(workspace.path())
            .kuku_home(data.path())
            .config(options.config.clone())
    };
    let start = query("Explain this version of kuku.")
        .bootstrap_skill(NAME, String::new())
        .start();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), start)
            .await
            .is_err()
    );
    let run = query("Say hello.").start().await.unwrap();
    drop(run);
    assert!(!archive.exists());
    lock.unlock().unwrap();
}

#[test]
fn abandoned_standalone_docs_slot_releases_the_blocking_worker() {
    let server = httpmock::MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/chat/completions");
        then.status(200).body(common::openai_sse_response(json!({
            "choices": [{"finish_reason": "tool_calls", "message": {"tool_calls": [{
                "id": "held-load", "type": "function",
                "function": {"name": "use_skill", "arguments": json!({"skill_name": NAME}).to_string()}
            }]}}]
        })));
    });
    for drop_run_first in [true, false] {
        let workspace = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let model = Model::new(data.path(), []);
        let mut options = options(
            workspace.path(),
            data.path(),
            model.clone(),
            Arc::new(Gate::default()),
        );
        options.config.providers.get_mut("openai").unwrap().base_url = server.base_url();
        let (archive, mut lock) = docs_archive_lock(&options, &model);
        assert!(lock.try_lock().unwrap());
        let (release_lock, waiting) = std::sync::mpsc::channel::<()>();
        let watchdog = std::thread::spawn(move || {
            let released = waiting.recv_timeout(Duration::from_secs(10)).is_ok();
            drop(lock);
            released
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let run = runtime.block_on(async {
            let mut run = kuku::query("Explain this version of kuku.")
                .config(options.config)
                .kuku_home(data.path())
                .workspace(workspace.path())
                .start()
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let kuku::UiEvent::ToolStart { tool, .. } =
                        run.next().await.unwrap().expect("Skill tool start")
                    {
                        assert_eq!(tool, "use_skill");
                        break;
                    }
                }
            })
            .await
            .expect("standalone Skill call should dispatch");
            assert!(
                tokio::time::timeout(Duration::from_millis(50), run.next())
                    .await
                    .is_err()
            );
            run
        });
        if drop_run_first {
            drop(run);
            // A standalone caller can leave its current-thread scheduler idle after dropping a Run.
            let (progress, observed) = std::sync::mpsc::channel();
            runtime.spawn_blocking(move || progress.send(()).unwrap());
            observed
                .recv_timeout(Duration::from_secs(5))
                .expect("blocking worker should be available");
            drop(runtime);
        } else {
            drop(runtime);
            drop(run);
        }
        let _ = release_lock.send(());
        assert!(
            watchdog.join().unwrap(),
            "abandoned worker kept waiting on the cache lock"
        );
        assert!(!archive.exists());
    }
}

#[tokio::test]
async fn same_batch_resources_do_not_depend_on_skill_body_admission() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let model = Model::new(
        data.path(),
        [
            Reply::Batch(vec![
                ("batch-load", "use_skill", json!({"skill_name":NAME})),
                (
                    "defer",
                    "write_file",
                    json!({"path":"permission-check.txt","content":"fixture"}),
                ),
                ("batch-list", "list_skills", json!({})),
                (
                    "batch-read",
                    TOOL,
                    json!({"skill_name":NAME,"action":"read","path":INDEX}),
                ),
            ]),
            Reply::Text,
        ],
    );
    let session = Session::open(options(
        workspace.path(),
        data.path(),
        model.clone(),
        Arc::new(Gate::default()),
    ))
    .await
    .unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new(
            "mixed-batch",
            "main",
            "Explain this version of kuku.",
        ))
        .unwrap();
    let waiting = settled(&session, AgentStatus::Waiting).await;
    let permission = waiting.permission.unwrap();
    assert_eq!(permission.tool_call_id, "defer");
    assert_eq!(tool_result(&session, "batch-load").0, "ok");

    session
        .reply_permission("deny", "main", &permission.id, false)
        .unwrap();
    assert_eq!(
        settled(&session, AgentStatus::Idle).await.turn,
        waiting.turn
    );
    assert_eq!(
        tool_result(&session, "batch-list").2["skills"][0]["loaded"],
        false
    );
    assert_eq!(tool_result(&session, "batch-read").0, "ok");
    assert!(!workspace.path().join("permission-check.txt").exists());
    session.close().await.unwrap();
}

#[tokio::test]
async fn thousand_plus_skills_are_discovered_on_demand_and_revision_reuse_survives() {
    const FILLER_SKILLS: usize = 1_200;
    const TARGET: &str = "zz-project-fixture";
    const FIRST_REVISION: &str = "unique project revision AAA with catalogneedle-aaa";
    const NEXT_REVISION: &str = "unique project revision BBB with catalogneedle-bbb";
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = workspace.path().join(".kuku/skills");
    let write_skill = |name: &str, body: &str| {
        let path = root.join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Fixture Skill\n---\n{body}"),
        )
        .unwrap();
    };
    let filler_body = "bystander-body-mark ".repeat(120);
    for index in 0..FILLER_SKILLS {
        write_skill(&format!("other-{index:04}"), &filler_body);
    }
    let load = |id| Reply::Call(id, "use_skill", json!({"skill_name":TARGET}));
    write_skill(TARGET, FIRST_REVISION);
    let model = Model::new(
        data.path(),
        [
            Reply::Batch(vec![
                ("front-page", "list_skills", json!({})),
                ("tail-page", "list_skills", json!({"offset": FILLER_SKILLS})),
                (
                    "search-first",
                    "search_skills",
                    json!({"query": "catalogneedle-aaa"}),
                ),
            ]),
            load("project-first"),
            load("project-repeat"),
            Reply::Text,
            Reply::Call(
                "search-next",
                "search_skills",
                json!({"query": "catalogneedle-bbb"}),
            ),
            load("project-next"),
            Reply::Text,
        ],
    );
    let mut options = options(
        workspace.path(),
        data.path(),
        model.clone(),
        Arc::new(Gate::default()),
    );
    options.config.discovery.extra_project_paths = vec![workspace.path().join(".kuku")];
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("first", "main", "Read project fixture."))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;
    let (_, _, first_page) = tool_result(&session, "front-page");
    assert_eq!(first_page["total"], FILLER_SKILLS + 2);
    assert_eq!(first_page["skills"].as_array().unwrap().len(), 20);
    assert!(
        first_page["skills"]
            .as_array()
            .unwrap()
            .iter()
            .all(|skill| skill["name"] != TARGET)
    );
    let (_, _, last_page) = tool_result(&session, "tail-page");
    assert_eq!(last_page["skills"].as_array().unwrap().len(), 2);
    assert_eq!(last_page["skills"][1]["name"], TARGET);
    let (_, _, first_hit) = tool_result(&session, "search-first");
    assert_eq!(first_hit["total"], 1);
    assert_eq!(first_hit["skills"][0]["name"], TARGET);
    assert_eq!(first_hit["skills"][0]["loaded"], false);
    assert_eq!(tool_result(&session, "project-repeat").2["reused"], true);
    write_skill(TARGET, NEXT_REVISION);
    session
        .send(Input::new(
            "second",
            "main",
            "Read updated project fixture.",
        ))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;
    let (_, _, next_hit) = tool_result(&session, "search-next");
    assert_eq!(next_hit["total"], 1);
    assert_eq!(next_hit["skills"][0]["name"], TARGET);
    assert_eq!(next_hit["skills"][0]["loaded"], false);
    assert_eq!(tool_result(&session, "project-next").2["reused"], false);
    {
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 7);
        let initial = requests[0].0.to_string();
        assert!(!initial.contains("other-0000") && !initial.contains(TARGET));
        assert!(
            requests
                .iter()
                .all(|(body, _)| !body.to_string().contains("bystander-body-mark"))
        );
        assert_eq!(requests[3].0.to_string().matches(FIRST_REVISION).count(), 1);
        assert!(!requests[4].0.to_string().contains(NEXT_REVISION));
        assert_eq!(requests[6].0.to_string().matches(NEXT_REVISION).count(), 1);
    }
    let records = session.events(0, 512).unwrap();
    let refs: Vec<_> = records
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ContextSkills { registry, .. } => Some(registry),
            _ => None,
        })
        .collect();
    assert_eq!(refs.len(), 2);
    assert!(refs.iter().all(|value| value.to_string().len() < 128));
    assert_ne!(refs[0]["snapshot"], refs[1]["snapshot"]);
    session.close().await.unwrap();
}
