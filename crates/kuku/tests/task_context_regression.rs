use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use httpmock::MockServer;
use kuku::event::{EventPayload, EventStore, ExecutionScope};
use kuku::query::{TaskQueryContext, WorkspaceQueryCapability};

mod common;

#[derive(Debug)]
struct SandboxCapability {
    root: std::path::PathBuf,
}

impl WorkspaceQueryCapability for SandboxCapability {
    fn workspace_id(&self) -> &str {
        "wsp_111111111111111111111111"
    }

    fn verify_identity(&self) -> kuku::Result<()> {
        Ok(())
    }

    fn file_exists(&self, relative_path: &str) -> kuku::Result<bool> {
        Ok(self.root.join(relative_path).is_file())
    }

    fn read_file(&self, relative_path: &str, max_bytes: usize) -> kuku::Result<Vec<u8>> {
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
        _max_entries: usize,
    ) -> kuku::Result<Vec<kuku::WorkspaceEntry>> {
        let root = if relative_path == "." {
            self.root.clone()
        } else {
            self.root.join(relative_path)
        };
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(root)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            entries.push(kuku::WorkspaceEntry {
                path: entry
                    .path()
                    .strip_prefix(&self.root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
                is_file: file_type.is_file(),
                is_dir: file_type.is_dir(),
            });
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

fn query_task(
    root: &std::path::Path,
    home: &std::path::Path,
    scope: &ExecutionScope,
) -> kuku::Query {
    let context = TaskQueryContext::new(
        scope.clone(),
        task_store(&home.join("events.jsonl"), scope),
        Arc::new(SandboxCapability {
            root: root.to_path_buf(),
        }),
    );
    kuku::query("task probe")
        .provider(kuku::Provider::OpenAiCompatible)
        .model("test-model")
        .base_url("http://127.0.0.1:1")
        .api_key("test-key")
        .kuku_home(home)
        .task_context(context)
}

fn task_facts(events: &[kuku::event::StoredEvent]) -> Vec<kuku::event::TaskEvent> {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::TaskLedger(kuku::event::TaskLedgerRecord::Activity(batch)) => {
                Some(batch.events().to_vec())
            }
            _ => None,
        })
        .flatten()
        .collect()
}

#[tokio::test]
async fn task_success_records_one_completed_request_fact() {
    let home = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    let scope = common::execution_scope();
    let events_path = home.path().join("events.jsonl");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/chat/completions");
        then.status(200)
            .body(common::openai_sse_response(serde_json::json!({
                "choices": [{"message": {"content": "done"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 3, "completion_tokens": 1}
            })));
    });

    query_task(&root, home.path(), &scope)
        .base_url(server.base_url())
        .run()
        .await
        .unwrap();

    let events = EventStore::replay(&events_path).unwrap();
    let facts = task_facts(&events);
    let started = facts
        .iter()
        .filter(|fact| matches!(fact, kuku::event::TaskEvent::RequestStarted(_)))
        .count();
    let completed = facts
        .iter()
        .filter(|fact| matches!(fact, kuku::event::TaskEvent::RequestCompleted(_)))
        .count();
    assert_eq!(1, started);
    assert_eq!(1, completed);
    if let Some(kuku::event::TaskEvent::RequestCompleted(completed)) = facts
        .iter()
        .find(|fact| matches!(fact, kuku::event::TaskEvent::RequestCompleted(_)))
    {
        assert_eq!(Some(3), completed.usage.input_tokens);
        assert_eq!(Some(1), completed.usage.output_tokens);
    } else {
        panic!("completed request fact missing");
    }
}

#[tokio::test]
async fn task_provider_failure_records_one_failed_request_fact() {
    let home = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    let scope = common::execution_scope();
    let events_path = home.path().join("events.jsonl");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/chat/completions");
        then.status(401)
            .header("content-type", "application/json")
            .body(r#"{"error":{"message":"denied"}}"#);
    });

    let error = query_task(&root, home.path(), &scope)
        .base_url(server.base_url())
        .run()
        .await
        .unwrap_err();
    assert_eq!("provider_auth", error.code());

    let events = EventStore::replay(&events_path).unwrap();
    let facts = task_facts(&events);
    let started = facts
        .iter()
        .filter(|fact| matches!(fact, kuku::event::TaskEvent::RequestStarted(_)))
        .count();
    let failed = facts
        .iter()
        .filter(|fact| matches!(fact, kuku::event::TaskEvent::RequestFailed(_)))
        .count();
    assert_eq!(1, started);
    assert_eq!(1, failed);
    assert!(!facts
        .iter()
        .any(|fact| { matches!(fact, kuku::event::TaskEvent::RequestCompleted(_)) }));
}

#[tokio::test]
async fn task_find_files_uses_the_workspace_capability() {
    let home = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("needle.txt"), "fixture").unwrap();
    let scope = common::execution_scope();

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/chat/completions");
        then.status(200)
            .body(common::openai_sse_response(serde_json::json!({
                "choices": [{
                    "message": {
                        "tool_calls": [{
                            "id": "tc_find",
                            "type": "function",
                            "function": {"name": "find_files", "arguments": "{\"path\":\".\"}"}
                        }]
                    },
                    "finish_reason": "tool_calls"
                }],
                "usage": {"prompt_tokens": 3, "completion_tokens": 1}
            })));
    });

    let mut run = query_task(&root, home.path(), &scope)
        .base_url(server.base_url())
        .start()
        .await
        .unwrap();
    let model_content = loop {
        match run.next().await.unwrap().unwrap() {
            kuku::UiEvent::ToolEnd { model_content, .. } => break model_content,
            _ => {}
        }
    };
    assert!(model_content
        .as_deref()
        .unwrap_or_default()
        .contains("needle.txt"));
}

#[tokio::test]
async fn task_context_loads_instructions_through_the_capability() {
    let home = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("AGENTS.md"), "capability instruction").unwrap();
    let scope = common::execution_scope();
    let events_path = home.path().join("events.jsonl");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/chat/completions");
        then.status(200)
            .body(common::openai_sse_response(serde_json::json!({
                "choices": [{"message": {"content": "done"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 3, "completion_tokens": 1}
            })));
    });

    query_task(&root, home.path(), &scope)
        .base_url(server.base_url())
        .run()
        .await
        .unwrap();

    let events = EventStore::replay(&events_path).unwrap();
    assert!(events.iter().any(|event| {
        matches!(
            &event.payload,
            EventPayload::ContextSources { project_instruction_sources, .. }
                if project_instruction_sources.iter().any(|source| source.path.contains("AGENTS.md"))
        )
    }));
}
