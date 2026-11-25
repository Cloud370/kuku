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
const TOOL: &str = "read_file";
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

    fn skill_dir(&self) -> PathBuf {
        std::fs::read_dir(&self.cache_root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.join("SKILL.md").is_file())
            .expect("the real Skill directory is ready before the first model request")
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
                for (index, (id, name, mut args)) in (0_u64..).zip(calls) {
                    if let Some(path) = args["path"].as_str()
                        && path.starts_with("references/docs/")
                    {
                        args["path"] = json!(self.skill_dir().join(path));
                    }
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

#[tokio::test]
async fn explicit_skill_admission_survives_reopening_a_waiting_turn() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let model = Model::new(
        data.path(),
        [
            Reply::Call("defer", TOOL, json!({"path": INDEX})),
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
    assert!(model.extracted());
    let original_directory = model.skill_dir().canonicalize().unwrap();
    options.session_id = Some(session.id().into());
    session.close().await.unwrap();
    std::fs::remove_dir_all(&original_directory).unwrap();

    let session = Session::open(options).await.unwrap();
    session
        .reply_permission("approve", "main", &permission.id, true)
        .unwrap();
    settled(&session, AgentStatus::Idle).await;
    let (status, page, _) = tool_result(&session, "defer");
    assert_eq!(status, "ok");
    assert!(page.contains("kuku"), "{page}");
    assert_eq!(
        model.skill_dir().canonicalize().unwrap(),
        original_directory
    );
    let requests = model.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    for (body, _) in &requests {
        assert_eq!(body.to_string().matches(SKILL_BODY).count(), 1);
    }
    assert!(requests[0].1);
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
            Reply::Call("configured-read", TOOL, json!({"path": INDEX})),
            Reply::Text,
            Reply::Call("unloaded-list", "list_skills", json!({})),
            Reply::Call("builtin-reuse", "use_skill", json!({"skill_name":NAME})),
            Reply::Call("unloaded-read", TOOL, json!({"path": INDEX})),
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
async fn first_request_keeps_one_hundred_descriptions_that_fit() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = workspace.path().join(".fixture");
    let description = "短句帮助索引。".repeat(10);
    for index in 0..100 {
        let name = format!("entire-{index:03}");
        let dir = root.join("skills").join(&name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\nDo not preload these instructions.\n"),
        )
        .unwrap();
    }
    let model = Model::new(data.path(), [Reply::Text]);
    let mut options = options(
        workspace.path(),
        data.path(),
        model.clone(),
        Arc::new(Gate::default()),
    );
    options.config.discovery.extra_project_paths = vec![root];
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("first", "main", "First request."))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;
    session.close().await.unwrap();
    let wire = model.requests.lock().unwrap()[0].0.to_string();
    assert!(wire.contains("Complete Skill catalog: 101 names."));
    assert!(!wire.contains("descriptions were shortened"));
    for index in 0..100 {
        assert!(wire.contains(&format!("entire-{index:03} — {description}")));
    }
    assert!(!wire.contains("Do not preload these instructions."));
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
            format!("---\nname: {name}\ndescription: A detailed fixture description. This Skill controls a synthetic revision for the catalog budget and discovery contract.\n---\n{body}"),
        )
        .unwrap();
    };
    let filler_body = "bystander-body-mark ".repeat(120);
    for index in 0..FILLER_SKILLS {
        write_skill(&format!("other-{index:04}"), &filler_body);
    }
    let load = |id| Reply::Call(id, "use_skill", json!({"skill_name":TARGET}));
    write_skill(TARGET, FIRST_REVISION);
    std::fs::File::open(root.join(TARGET).join("SKILL.md"))
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(86_400)),
        )
        .unwrap();
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
            Reply::Call("sort-only", "list_skills", json!({})),
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
    assert_eq!(last_page["skills"][0]["name"], TARGET);
    let (_, _, first_hit) = tool_result(&session, "search-first");
    assert_eq!(first_hit["total"], 1);
    assert_eq!(first_hit["skills"][0]["name"], TARGET);
    assert_eq!(first_hit["skills"][0]["loaded"], false);
    assert_eq!(tool_result(&session, "project-repeat").2["reused"], true);
    write_skill(TARGET, NEXT_REVISION);
    write_skill(
        "added-catalog-skill",
        "A new Skill unrelated to the target.",
    );
    std::fs::remove_file(root.join("other-0001").join("SKILL.md")).unwrap();
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
    std::fs::File::open(root.join(TARGET).join("SKILL.md"))
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(std::time::SystemTime::now() + Duration::from_secs(86_400)),
        )
        .unwrap();
    session
        .send(Input::new("sort-only", "main", "List the latest Skills."))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;
    assert_eq!(
        tool_result(&session, "sort-only").2["skills"][0]["name"],
        TARGET
    );
    {
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 9);
        for pair in requests.windows(2) {
            let [previous, next] = pair else {
                unreachable!()
            };
            assert!(
                next.0["messages"]
                    .as_array()
                    .unwrap()
                    .starts_with(previous.0["messages"].as_array().unwrap())
            );
        }
        let initial = requests[0].0.to_string();
        assert!(initial.contains("Complete Skill catalog: 1202 names."));
        assert!(initial.contains("descriptions were shortened"));
        assert!(initial.contains(TARGET));
        assert!(initial.find("other-1199").unwrap() < initial.find(TARGET).unwrap());
        for index in 0..FILLER_SKILLS {
            assert!(initial.contains(&format!("other-{index:04} — ")));
        }
        assert!(!initial.contains("<kuku_skill_changes>"));
        for (body, _) in &requests[4..] {
            let wire = body.to_string();
            assert_eq!(wire.matches("<kuku_skill_changes>").count(), 1);
            assert!(wire.contains("Added: added-catalog-skill"));
            assert!(wire.contains("Updated: zz-project-fixture (new revision)"));
            assert!(wire.contains("Removed: other-0001"));
        }
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
    assert_eq!(refs.len(), 3);
    assert!(refs.iter().all(|value| value.to_string().len() < 128));
    assert_ne!(refs[0]["snapshot"], refs[1]["snapshot"]);
    assert_ne!(refs[1]["snapshot"], refs[2]["snapshot"]);
    session.close().await.unwrap();
}

#[tokio::test]
async fn catalog_announces_a_relocated_skill_without_changing_the_frozen_prefix() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let first_root = workspace.path().join(".first");
    let second_root = workspace.path().join(".second");
    let skill = first_root.join("skills/relocated");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::create_dir_all(second_root.join("skills")).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: relocated\ndescription: Relocation fixture.\n---\nUse the files beside this Skill.\n",
    )
    .unwrap();
    let model = Model::new(data.path(), [Reply::Text, Reply::Text]);
    let mut options = options(
        workspace.path(),
        data.path(),
        model.clone(),
        Arc::new(Gate::default()),
    );
    options.config.discovery.extra_project_paths = vec![first_root, second_root.clone()];
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("first", "main", "First request."))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;

    std::fs::rename(skill, second_root.join("skills/relocated")).unwrap();
    session
        .send(Input::new("second", "main", "Second request."))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;
    session.close().await.unwrap();

    let requests = model.requests.lock().unwrap();
    let first = requests[0].0["messages"].as_array().unwrap();
    let second = requests[1].0["messages"].as_array().unwrap();
    assert!(second.starts_with(first));
    let wire = requests[1].0.to_string();
    assert_eq!(wire.matches("<kuku_skill_catalog>").count(), 1);
    assert_eq!(wire.matches("<kuku_skill_changes>").count(), 1);
    assert!(wire.contains("Updated: relocated (new revision)"));
    assert_eq!(wire.matches("Relocation fixture.").count(), 1);
}

#[tokio::test]
async fn a_skill_disabled_successor_drops_catalog_notices_after_reopening() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = workspace.path().join(".fixture");
    let model = Model::new(
        data.path(),
        [Reply::Text, Reply::Text, Reply::Text, Reply::Text],
    );
    let mut options = options(
        workspace.path(),
        data.path(),
        model.clone(),
        Arc::new(Gate::default()),
    );
    options.config.discovery.extra_project_paths = vec![root.clone()];
    let session = Session::open(options.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("first", "main", "Initial request."))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;
    let skill = root.join("skills/late-skill");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: late-skill\ndescription: Late arrival.\n---\nA new Skill.\n",
    )
    .unwrap();
    session
        .send(Input::new("addition", "main", "Request after addition."))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;
    options.session_id = Some(session.id().into());
    session.close().await.unwrap();

    let session = Session::open(options).await.unwrap();
    session
        .configure_agent(
            "disable",
            "main",
            AgentConfig {
                no_skills: true,
                ..AgentConfig::default()
            },
        )
        .unwrap();
    session
        .send(Input::new("disabled", "main", "Request without Skills."))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;
    session
        .configure_agent("enable", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("enabled", "main", "Request with Skills again."))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;
    session.close().await.unwrap();

    let requests = model.requests.lock().unwrap();
    assert!(requests[1].0.to_string().contains("Added: late-skill"));
    let disabled = requests[2].0.to_string();
    assert!(disabled.contains("Initial request.") && disabled.contains("Request after addition."));
    assert!(!disabled.contains("<kuku_skill_catalog>"));
    assert!(!disabled.contains("<kuku_skill_changes>"));
    let enabled = requests[3].0.to_string();
    assert_eq!(enabled.matches("<kuku_skill_catalog>").count(), 1);
    assert!(enabled.contains("late-skill — Late arrival."));
    assert!(!enabled.contains("<kuku_skill_changes>"));
}
