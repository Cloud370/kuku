use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use httpmock::MockServer;
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
    fn verify_identity(&self) -> kuku::Result<()> {
        self.valid
            .load(Ordering::SeqCst)
            .then_some(())
            .ok_or_else(|| kuku::Error::WorkspaceUnavailable("workspace identity changed".into()))
    }

    fn execution_root(&self) -> kuku::Result<std::path::PathBuf> {
        self.verify_identity()?;
        Ok(self.root.clone())
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
