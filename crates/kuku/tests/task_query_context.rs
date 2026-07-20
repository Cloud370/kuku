use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::{future::Future, pin::Pin};

use httpmock::{prelude::HttpMockRequest, MockServer};
use kuku::event::{EventPayload, EventStore, ExecutionScope};
use kuku::query::{TaskQueryContext, WorkspaceQueryCapability};

mod common;

fn configured_query(prompt: &str) -> kuku::Query {
    kuku::query(prompt)
        .provider(kuku::Provider::Anthropic)
        .model("test-model")
        .base_url("http://127.0.0.1:1")
        .api_key("test-key")
}

#[derive(Debug)]
struct TestWorkspaceCapability {
    root: std::path::PathBuf,
    valid: Arc<AtomicBool>,
}

impl WorkspaceQueryCapability for TestWorkspaceCapability {
    fn workspace_id(&self) -> &str {
        "wsp_111111111111111111111111"
    }

    fn verify_identity(&self) -> kuku::Result<()> {
        self.valid
            .load(Ordering::SeqCst)
            .then_some(())
            .ok_or_else(|| kuku::Error::WorkspaceUnavailable("workspace identity changed".into()))
    }

    fn file_exists(&self, relative_path: &str) -> kuku::Result<bool> {
        self.verify_identity()?;
        Ok(self.root.join(relative_path).is_file())
    }

    fn read_file(&self, relative_path: &str, max_bytes: usize) -> kuku::Result<Vec<u8>> {
        self.verify_identity()?;
        let bytes = std::fs::read(self.root.join(relative_path))?;
        if bytes.len() > max_bytes {
            return Err(kuku::Error::WorkspaceUnavailable(
                "workspace file exceeds read limit".to_string(),
            ));
        }
        Ok(bytes)
    }

    fn read_skill_source(
        &self,
        selected: &kuku::event::SkillContextFact,
        max_bytes: usize,
    ) -> kuku::Result<Vec<u8>> {
        let path = selected.source.relative_path.as_ref().ok_or_else(|| {
            kuku::Error::InvalidTaskContext("selected skill path is missing".to_string())
        })?;
        self.read_file(path.as_str(), max_bytes)
    }

    fn write_file(
        &self,
        relative_path: &str,
        contents: &[u8],
        max_bytes: usize,
    ) -> kuku::Result<()> {
        self.verify_identity()?;
        if contents.len() > max_bytes {
            return Err(kuku::Error::WorkspaceUnavailable(
                "workspace write exceeds limit".to_string(),
            ));
        }
        Ok(std::fs::write(self.root.join(relative_path), contents)?)
    }

    fn list_entries(
        &self,
        relative_path: &str,
        max_entries: usize,
    ) -> kuku::Result<Vec<kuku::WorkspaceEntry>> {
        self.verify_identity()?;
        let root = if relative_path == "." {
            self.root.clone()
        } else {
            self.root.join(relative_path)
        };
        let mut pending = vec![root];
        let mut entries = Vec::new();
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(directory)? {
                let entry = entry?;
                let path = entry.path();
                let relative = path
                    .strip_prefix(&self.root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let file_type = entry.file_type()?;
                entries.push(kuku::WorkspaceEntry {
                    path: relative,
                    is_file: file_type.is_file(),
                    is_dir: file_type.is_dir(),
                });
                if file_type.is_dir() {
                    pending.push(path);
                }
                if entries.len() >= max_entries {
                    return Ok(entries);
                }
            }
        }
        Ok(entries)
    }

    fn run_command<'a>(
        &'a self,
        _request: kuku::WorkspaceCommandRequest,
        _events: Option<tokio::sync::mpsc::Sender<kuku::WorkspaceCommandEvent>>,
        _cancellation: kuku::WorkspaceCommandCancellation,
    ) -> Pin<Box<dyn Future<Output = kuku::Result<kuku::WorkspaceCommandOutput>> + Send + 'a>> {
        Box::pin(async {
            Err(kuku::Error::WorkspaceUnavailable(
                "test capability does not run commands".to_string(),
            ))
        })
    }
}

fn workspace_capability(
    root: &std::path::Path,
    valid: Arc<AtomicBool>,
) -> Arc<dyn WorkspaceQueryCapability> {
    Arc::new(TestWorkspaceCapability {
        root: root.to_path_buf(),
        valid,
    })
}

fn selected_project_skill(
    workspace: &std::path::Path,
    skill_id: &str,
    relative_path: &str,
) -> kuku::event::SkillContextFact {
    let path = workspace.join(relative_path);
    let name = skill_id.rsplit(':').next().unwrap();
    let registry = kuku::skill::registry::SkillRegistry::builder()
        .load_from_dir(
            path.parent().unwrap().parent().unwrap(),
            kuku::skill::definition::SkillSource::Project,
        )
        .unwrap()
        .build();
    kuku::event::SkillContextFact {
        skill_id: skill_id.to_string(),
        source: kuku::event::SourceFact {
            scope: kuku::event::SourceScope::Project,
            id: "source:project".to_string(),
            relative_path: Some(kuku::event::WorkspaceRelativePath::parse(relative_path).unwrap()),
        },
        origin: kuku::event::SkillLoadOrigin::You,
        content_hash: registry.get(name).unwrap().hash.clone(),
    }
}

fn selected_workspace_skill(
    workspace: &std::path::Path,
    skill_id: &str,
    relative_path: &str,
) -> kuku::event::SkillContextFact {
    let project_id = skill_id.replacen("skill:workspace:", "skill:project:", 1);
    let mut selected = selected_project_skill(workspace, &project_id, relative_path);
    selected.skill_id = skill_id.to_string();
    selected.source.scope = kuku::event::SourceScope::Workspace;
    selected.source.id = "source:workspace".to_string();
    selected
}

fn task_store(path: &std::path::Path, scope: &ExecutionScope) -> EventStore {
    let mut store = EventStore::open(path).unwrap();
    let receipt = kuku::event::CommandReceipt::new(
        "create-task",
        "digest",
        kuku::event::CommandResult::TaskCreated {
            task_id: scope.task_id.clone(),
        },
    )
    .unwrap();
    let run = kuku::event::RunFact {
        run_id: scope.run_id.clone(),
        task_id: scope.task_id.clone(),
        state: kuku::event::RunState::Queued,
        started_at: "2026-07-20T00:00:00Z".to_string(),
        finished_at: None,
        summary: None,
        warnings: Vec::new(),
        checks: None,
        metrics: None,
        workspace_changes: None,
    };
    let transaction = kuku::event::TaskTransaction::try_new(
        kuku::event::TaskRevision::try_new(0).unwrap(),
        receipt,
        vec![
            kuku::event::TaskEvent::TaskCreated {
                task_id: scope.task_id.clone(),
                workspace_id: scope.workspace_id.clone(),
                title: "Task".to_string(),
                created_at: "2026-07-20T00:00:00Z".to_string(),
            },
            kuku::event::TaskEvent::RunQueued { run },
        ],
    )
    .unwrap();
    store
        .append_synced(EventPayload::TaskLedger(
            kuku::event::TaskLedgerRecord::Control(transaction),
        ))
        .unwrap();
    store
}

#[tokio::test]
async fn task_query_uses_only_the_supplied_ledger_and_scope() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let ledger_path = home.path().join("tasks/task/events.jsonl");
    let scope = common::execution_scope();
    let store = task_store(&ledger_path, &scope);
    let root = workspace_capability(workspace.path(), Arc::new(AtomicBool::new(true)));

    let run = configured_query("hello")
        .kuku_home(home.path())
        .task_context(TaskQueryContext::new(scope.clone(), store, root))
        .start()
        .await
        .unwrap();

    assert_eq!(run.task_id(), &scope.task_id);
    assert_eq!(run.run_id(), &scope.run_id);
    let events = EventStore::replay(&ledger_path).unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::TurnStarted { execution, .. } if execution == &scope
    )));
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::MessageUser { execution, text, .. }
            if execution == &scope && text == "hello"
    )));
    assert!(!events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::SessionCreated { .. })));

    let legacy =
        kuku::session::session_events_path(home.path(), workspace.path(), run.session_id())
            .unwrap();
    assert_ne!(legacy, ledger_path);
    assert!(!legacy.exists());
}

#[tokio::test]
async fn task_query_rejects_a_workspace_capability_that_lost_identity() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let valid = Arc::new(AtomicBool::new(true));
    let root = workspace_capability(workspace.path(), Arc::clone(&valid));
    valid.store(false, Ordering::SeqCst);
    let scope = common::execution_scope();
    let context = TaskQueryContext::new(
        scope.clone(),
        task_store(&home.path().join("events.jsonl"), &scope),
        root,
    );

    let error = configured_query("hello")
        .kuku_home(home.path())
        .task_context(context)
        .start()
        .await
        .unwrap_err();

    assert_eq!(error.code(), "workspace_unavailable");
}

#[tokio::test]
async fn task_query_and_request_lifecycle_share_the_injected_store() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let ledger_path = home.path().join("tasks/task/events.jsonl");
    let scope = common::execution_scope();
    let store = task_store(&ledger_path, &scope);
    let durable_notifications = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    store.register_observer({
        let durable_notifications = Arc::clone(&durable_notifications);
        Arc::new(move |_| {
            durable_notifications.fetch_add(1, Ordering::SeqCst);
        })
    });
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200)
            .header("content-type", "text/event-stream")
            .body(concluding_anthropic_response("done"));
    });
    let context = TaskQueryContext::new(
        scope.clone(),
        store,
        workspace_capability(workspace.path(), Arc::new(AtomicBool::new(true))),
    );

    let output = kuku::query("hello")
        .provider(kuku::Provider::Anthropic)
        .model("test-model")
        .base_url(server.base_url())
        .api_key("test-key")
        .kuku_home(home.path())
        .task_context(context)
        .run()
        .await
        .unwrap();

    assert_eq!(output.text, "done");
    let events = EventStore::replay(&ledger_path).unwrap();
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ModelResponse { .. })));
    let lifecycle = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::TaskLedger(kuku::event::TaskLedgerRecord::Activity(batch)) => {
                Some(batch.events())
            }
            _ => None,
        })
        .flatten()
        .filter(|event| {
            matches!(
                event,
                kuku::event::TaskEvent::RequestStarted(_)
                    | kuku::event::TaskEvent::RequestCompleted(_)
            )
        })
        .count();
    assert_eq!(lifecycle, 2);
    assert_eq!(durable_notifications.load(Ordering::SeqCst), 2);
    assert!(!events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::SessionCreated { .. })));
}

#[tokio::test]
async fn task_query_rejects_task_workspace_or_active_run_mismatch_before_append() {
    for mismatch in ["task", "workspace", "run"] {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let ledger_path = home.path().join(format!("{mismatch}.jsonl"));
        let ledger_scope = common::execution_scope();
        let store = task_store(&ledger_path, &ledger_scope);
        let before = store.read_all().unwrap().len();
        let mut supplied = ledger_scope.clone();
        match mismatch {
            "task" => supplied.task_id = kuku::TaskId::try_new().unwrap(),
            "workspace" => supplied.workspace_id = kuku::WorkspaceId::try_new().unwrap(),
            "run" => supplied.run_id = kuku::RunId::try_new().unwrap(),
            _ => unreachable!(),
        }
        let context = TaskQueryContext::new(
            supplied,
            store.clone(),
            workspace_capability(workspace.path(), Arc::new(AtomicBool::new(true))),
        );

        let error = configured_query("hello")
            .kuku_home(home.path())
            .task_context(context)
            .start()
            .await
            .unwrap_err();

        assert_eq!(error.code(), "invalid_task_context", "{mismatch}");
        assert_eq!(store.read_all().unwrap().len(), before, "{mismatch}");
    }
}

#[tokio::test]
async fn task_query_activates_only_explicitly_selected_capability_skills() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    for name in ["focus", "ignored", "zeta"] {
        let directory = workspace.path().join(format!(".agent/skills/{name}"));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name} skill\n---\n\n{name} instructions\n"),
        )
        .unwrap();
    }
    let scope = common::execution_scope();
    let store = task_store(&home.path().join("events.jsonl"), &scope);
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .matches(|request: &HttpMockRequest| {
                let body = String::from_utf8_lossy(request.body.as_deref().unwrap_or_default());
                matches!(
                    (body.find("zeta instructions"), body.find("focus instructions")),
                    (Some(zeta), Some(focus)) if zeta < focus
                ) && !body.contains("ignored instructions")
            });
        then.status(200)
            .header("content-type", "text/event-stream")
            .body(concluding_anthropic_response("done"));
    });
    let context = TaskQueryContext::new(
        scope,
        store.clone(),
        workspace_capability(workspace.path(), Arc::new(AtomicBool::new(true))),
    )
    .with_selected_skills(vec![
        selected_project_skill(
            workspace.path(),
            "skill:project:zeta",
            ".agent/skills/zeta/SKILL.md",
        ),
        selected_project_skill(
            workspace.path(),
            "skill:project:focus",
            ".agent/skills/focus/SKILL.md",
        ),
    ]);
    let mut config: kuku::config::ConfigFile =
        toml::from_str(kuku::config::generate_default()).unwrap();
    config.discovery.as_mut().unwrap().auto_discover = false;

    configured_query("hello")
        .config(config.resolve().unwrap())
        .kuku_home(home.path())
        .base_url(server.base_url())
        .task_context(context)
        .run()
        .await
        .unwrap();

    let registry = store
        .read_all()
        .unwrap()
        .into_iter()
        .find_map(|event| match event.payload {
            EventPayload::ContextSkills { registry, .. } => Some(registry),
            _ => None,
        })
        .unwrap();
    let encoded = registry.to_string();
    assert!(encoded.contains("focus"));
    assert!(encoded.contains("zeta"));
    assert!(!encoded.contains("ignored"));
}

#[tokio::test]
async fn unavailable_selected_skill_is_rejected_before_query_facts_append() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let scope = common::execution_scope();
    let store = task_store(&home.path().join("events.jsonl"), &scope);
    let before = store.read_all().unwrap().len();
    let context = TaskQueryContext::new(
        scope,
        store.clone(),
        workspace_capability(workspace.path(), Arc::new(AtomicBool::new(true))),
    )
    .with_selected_skills(vec![kuku::event::SkillContextFact {
        skill_id: "skill:project:missing".to_string(),
        source: kuku::event::SourceFact {
            scope: kuku::event::SourceScope::Project,
            id: "source:project:missing".to_string(),
            relative_path: Some(
                kuku::event::WorkspaceRelativePath::parse(".agent/skills/missing/SKILL.md")
                    .unwrap(),
            ),
        },
        origin: kuku::event::SkillLoadOrigin::You,
        content_hash: "sha256:missing".to_string(),
    }]);

    let error = configured_query("hello")
        .kuku_home(home.path())
        .task_context(context)
        .start()
        .await
        .unwrap_err();

    assert_eq!(error.code(), "invalid_task_context");
    assert_eq!(store.read_all().unwrap().len(), before);
}

#[tokio::test]
async fn task_query_accepts_workspace_skills_sharing_a_catalog_source() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    for name in ["one", "two"] {
        let directory = workspace.path().join(format!("catalog/skills/{name}"));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name}\n---\n\n{name} instructions\n"),
        )
        .unwrap();
    }
    let scope = common::execution_scope();
    let store = task_store(&home.path().join("events.jsonl"), &scope);
    let selected = ["one", "two"]
        .into_iter()
        .map(|name| {
            selected_workspace_skill(
                workspace.path(),
                &format!("skill:workspace:{name}"),
                &format!("catalog/skills/{name}/SKILL.md"),
            )
        })
        .collect();

    let run = configured_query("hello")
        .kuku_home(home.path())
        .task_context(
            TaskQueryContext::new(
                scope,
                store.clone(),
                workspace_capability(workspace.path(), Arc::new(AtomicBool::new(true))),
            )
            .with_selected_skills(selected),
        )
        .start()
        .await
        .unwrap();
    drop(run);

    let registry = store
        .read_all()
        .unwrap()
        .into_iter()
        .find_map(|event| match event.payload {
            EventPayload::ContextSkills { registry, .. } => Some(registry.to_string()),
            _ => None,
        })
        .unwrap();
    assert!(registry.contains("one"));
    assert!(registry.contains("two"));
    assert!(registry.contains("workspace"));
}

#[tokio::test]
async fn fresh_task_turn_uses_current_skill_selection_and_can_clear_it() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    for name in ["alpha", "beta"] {
        let directory = workspace.path().join(format!(".agent/skills/{name}"));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name}\n---\n\n{name} exact instructions\n"),
        )
        .unwrap();
    }
    let scope = common::execution_scope();
    let store = task_store(&home.path().join("events.jsonl"), &scope);
    let capability = workspace_capability(workspace.path(), Arc::new(AtomicBool::new(true)));
    let alpha = selected_project_skill(
        workspace.path(),
        "skill:project:alpha",
        ".agent/skills/alpha/SKILL.md",
    );
    let beta = selected_project_skill(
        workspace.path(),
        "skill:project:beta",
        ".agent/skills/beta/SKILL.md",
    );

    let alpha_server = MockServer::start();
    alpha_server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200)
            .header("content-type", "text/event-stream")
            .body(concluding_anthropic_response("alpha done"));
    });
    configured_query("first")
        .base_url(alpha_server.base_url())
        .kuku_home(home.path())
        .task_context(
            TaskQueryContext::new(scope.clone(), store.clone(), capability.clone())
                .with_selected_skills(vec![alpha]),
        )
        .run()
        .await
        .unwrap();

    let beta_server = MockServer::start();
    beta_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .matches(|request: &HttpMockRequest| {
                let body = String::from_utf8_lossy(request.body.as_deref().unwrap_or_default());
                body.contains("beta exact instructions")
                    && !body.contains("alpha exact instructions")
            });
        then.status(200)
            .header("content-type", "text/event-stream")
            .body(concluding_anthropic_response("beta done"));
    });
    configured_query("second")
        .base_url(beta_server.base_url())
        .kuku_home(home.path())
        .task_context(
            TaskQueryContext::new(scope.clone(), store.clone(), capability.clone())
                .with_selected_skills(vec![beta]),
        )
        .run()
        .await
        .unwrap();

    let empty_server = MockServer::start();
    empty_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .matches(|request: &HttpMockRequest| {
                let body = String::from_utf8_lossy(request.body.as_deref().unwrap_or_default());
                body.contains("Loaded: none")
                    && !body.contains("alpha exact instructions")
                    && !body.contains("beta exact instructions")
            });
        then.status(200)
            .header("content-type", "text/event-stream")
            .body(concluding_anthropic_response("empty done"));
    });
    configured_query("third")
        .base_url(empty_server.base_url())
        .kuku_home(home.path())
        .task_context(TaskQueryContext::new(scope, store, capability))
        .run()
        .await
        .unwrap();
}

fn concluding_anthropic_response(text: &str) -> String {
    format!(
        "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_1\",\"model\":\"test-model\",\"content\":[],\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}}}}}}\n\n\
         event: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\n\
         event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":{}}}}}\n\n\
         event: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n\
         event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{\"output_tokens\":1}}}}\n\n\
         event: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n",
        serde_json::to_string(text).unwrap()
    )
}
