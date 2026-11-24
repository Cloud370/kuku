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

const NAME: &str = "kuku-docs";
const TOOL: &str = "skill_resources";
const SKILL_BODY: &str = "Identify the running kuku version if known";
const INDEX: &str = "references/docs/en/index.md";

#[derive(Debug)]
enum Reply {
    Text,
    Call(&'static str, &'static str, Value),
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
            let reason = match reply {
                Reply::Text => {
                    events.push(ModelEvent::TextDelta {
                        text: "fixture reply".into(),
                    });
                    ModelStopReason::EndTurn
                }
                Reply::Call(id, name, args) => {
                    events.extend([
                        ModelEvent::ToolCallStart {
                            index: 0,
                            id: id.into(),
                            name: name.into(),
                        },
                        ModelEvent::ToolCallArgDelta {
                            index: 0,
                            fragment: args.to_string(),
                        },
                        ModelEvent::ToolCallStop { index: 0 },
                    ]);
                    ModelStopReason::ToolUse
                }
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
                json!({"skill_name": NAME,"action":"read","path":INDEX}),
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
    assert_eq!(status, "error");
    assert!(unloaded.contains("Load kuku-docs"));
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
            .contains("kuku-docs - Explain how to use this version")
    );
    assert!(
        requests[..3]
            .iter()
            .all(|(body, _)| !body.to_string().contains(SKILL_BODY))
    );
    assert!(requests[3].0.to_string().contains(SKILL_BODY));
    assert!(requests[..6].iter().all(|(_, extracted)| !extracted));
    assert!(requests[6].1);
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
async fn configured_docs_skill_uses_turn_admissions_and_reports_when_released() {
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
        false
    );
    assert_eq!(tool_result(&session, "unloaded-read").0, "error");
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
