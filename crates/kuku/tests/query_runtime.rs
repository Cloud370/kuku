use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use httpmock::prelude::*;
use kuku::event::{EventPayload, EventStore};
use kuku::session::session_events_path;
use kuku::{query, Error, Provider, UiEvent};
use tempfile::TempDir;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct TestEnv {
    _guard: MutexGuard<'static, ()>,
    home: TempDir,
    workspace: TempDir,
    previous_kuku_home: Option<OsString>,
    previous_cwd: PathBuf,
}

impl TestEnv {
    fn new() -> Self {
        let guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_kuku_home = std::env::var_os("KUKU_HOME");
        let previous_cwd = std::env::current_dir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();

        std::env::set_var("KUKU_HOME", home.path());
        std::env::set_current_dir(workspace.path()).unwrap();

        Self {
            _guard: guard,
            home,
            workspace,
            previous_kuku_home,
            previous_cwd,
        }
    }

    fn events_path(&self, session_id: &str) -> PathBuf {
        let workspace = std::fs::canonicalize(self.workspace.path()).unwrap();
        session_events_path(self.home.path(), &workspace, session_id).unwrap()
    }

    fn workspace_path(&self) -> &Path {
        self.workspace.path()
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.previous_cwd).unwrap();
        match &self.previous_kuku_home {
            Some(value) => std::env::set_var("KUKU_HOME", value),
            None => std::env::remove_var("KUKU_HOME"),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn start_creates_session_events_under_kuku_home() {
    let env = TestEnv::new();

    let run = query("inspect this project").start().await.unwrap();
    let session_id = run.session_id().to_string();

    let events = EventStore::replay(env.events_path(&session_id)).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].id, 1);
    assert_eq!(events[1].id, 2);
    assert_eq!(events[2].id, 3);

    match &events[0].payload {
        EventPayload::SessionMeta {
            schema_version,
            session_id: meta_session_id,
            kuku_version,
            ts,
            created_at,
        } => {
            assert_eq!(*schema_version, 1);
            assert_eq!(meta_session_id, &session_id);
            assert_eq!(kuku_version, env!("CARGO_PKG_VERSION"));
            assert!(ts.ends_with('Z'));
            assert!(created_at.ends_with('Z'));
        }
        other => panic!("expected session.meta, got {other:?}"),
    }

    match &events[1].payload {
        EventPayload::TurnStart { turn, ts } => {
            assert_eq!(*turn, 1);
            assert!(ts.ends_with('Z'));
        }
        other => panic!("expected turn.start, got {other:?}"),
    }

    match &events[2].payload {
        EventPayload::UserInput { turn, text, ts } => {
            assert_eq!(*turn, 1);
            assert_eq!(text, "inspect this project");
            assert!(ts.ends_with('Z'));
        }
        other => panic!("expected user.input, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn run_without_provider_config_writes_error_and_closes_turn() {
    let env = TestEnv::new();

    let error = query("summarize")
        .session("s_run_fixed")
        .run()
        .await
        .unwrap_err();

    assert!(matches!(error, Error::MissingProviderConfig(_)));
    let events = EventStore::replay(env.events_path("s_run_fixed")).unwrap();
    assert_eq!(events.len(), 5);
    assert!(matches!(
        events[0].payload,
        EventPayload::SessionMeta { .. }
    ));
    assert!(matches!(
        events[1].payload,
        EventPayload::TurnStart { turn: 1, .. }
    ));
    assert!(matches!(
        events[2].payload,
        EventPayload::UserInput { turn: 1, .. }
    ));
    assert!(matches!(events[3].payload, EventPayload::ModelError { .. }));
    assert!(matches!(
        events[4].payload,
        EventPayload::TurnEnd { turn: 1, .. }
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_session_start_appends_turn_without_duplicate_meta() {
    let env = TestEnv::new();

    query("first").session("s_continue").start().await.unwrap();
    query("second").session("s_continue").start().await.unwrap();

    let events = EventStore::replay(env.events_path("s_continue")).unwrap();
    assert_eq!(events.len(), 5);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::SessionMeta { .. }))
            .count(),
        1
    );

    assert!(matches!(
        events[1].payload,
        EventPayload::TurnStart { turn: 1, .. }
    ));
    assert!(matches!(
        events[2].payload,
        EventPayload::UserInput { turn: 1, .. }
    ));
    assert!(matches!(
        events[3].payload,
        EventPayload::TurnStart { turn: 2, .. }
    ));
    match &events[4].payload {
        EventPayload::UserInput { turn, text, .. } => {
            assert_eq!(*turn, 2);
            assert_eq!(text, "second");
        }
        other => panic!("expected second user.input, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_is_not_polluted() {
    let env = TestEnv::new();

    let _ = query("no pollution").run().await.unwrap_err();

    assert_eq!(std::fs::read_dir(env.workspace_path()).unwrap().count(), 0);
    assert!(!env.workspace_path().join(".kuku").exists());
    assert!(!env.workspace_path().join(".kuku-id").exists());
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_session_ids_fail_before_creating_session_path() {
    let env = TestEnv::new();

    for session_id in [
        "../bad",
        "CON",
        "con",
        "COM1",
        "LPT9",
        "CON.txt",
        "aux.log",
        "LPT1.json",
        "name.",
        "name ",
    ] {
        let error = query("bad").session(session_id).run().await.unwrap_err();
        assert!(matches!(error, Error::InvalidSessionId(ref value) if value == session_id));
    }

    assert!(!env.home.path().join("p").exists());
}

#[tokio::test(flavor = "current_thread")]
async fn run_emits_permission_requested_for_gated_tool() {
    let env = TestEnv::new();
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200).json_body(serde_json::json!({
            "id": "msg_tool",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Need approval."},
                {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"command": "cargo test", "timeout": 60}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 5, "output_tokens": 6}
        }));
    });

    let mut run = query("run tests")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .start()
        .await
        .unwrap();

    let event = run.next().await.unwrap().expect("permission event");
    match event {
        UiEvent::PermissionRequested { request } => {
            assert_eq!(request.tool_call_id, "toolu_cmd");
            assert_eq!(request.tool, "run_command");
        }
        other => panic!("expected PermissionRequested, got {other:?}"),
    }

    let events = EventStore::replay(env.events_path(run.session_id())).unwrap();
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::PermissionRequest { .. })));
    assert!(!events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::PermissionDecision { .. })));
}

#[tokio::test(flavor = "current_thread")]
async fn session_scope_allow_is_reused_on_later_turn_in_same_session() {
    let env = TestEnv::new();
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""messages":[{"content":[{"text":"run tests","type":"text"}],"role":"user"}]"#);
        then.status(200).json_body(serde_json::json!({
            "id": "msg_tool",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Need approval."},
                {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"command": "cargo test", "timeout": 60}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 5, "output_tokens": 6}
        }));
    });

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#);
        then.status(200).json_body(serde_json::json!({
            "id": "msg_final_1",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "First command completed."}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 8, "output_tokens": 5}
        }));
    });

    let session_id = "s_session_grant";
    let mut run = query("run tests")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .start()
        .await
        .unwrap();

    let request = match run.next().await.unwrap().unwrap() {
        UiEvent::PermissionRequested { request } => request,
        other => panic!("expected PermissionRequested, got {other:?}"),
    };
    run.decide(&request.id, kuku::query::PermissionChoice::Session)
        .await
        .unwrap();
    let first_done = run.next().await.unwrap().unwrap();
    match first_done {
        UiEvent::Done { output } => assert_eq!(output.text, "First command completed."),
        other => panic!("expected Done, got {other:?}"),
    }

    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""messages":[{"content":[{"text":"run tests","type":"text"}],"role":"user"}]"#);
        then.status(200).json_body(serde_json::json!({
            "id": "msg_tool_2",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Need approval again."},
                {"type": "tool_use", "id": "toolu_cmd_2", "name": "run_command", "input": {"command": "cargo test", "timeout": 60}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 5, "output_tokens": 6}
        }));
    });

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#);
        then.status(200).json_body(serde_json::json!({
            "id": "msg_final_2",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "Second command completed."}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 8, "output_tokens": 5}
        }));
    });

    let mut run = query("run tests")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .start()
        .await
        .unwrap();

    let done = run.next().await.unwrap().unwrap();
    match done {
        UiEvent::Done { output } => assert_eq!(output.text, "Second command completed."),
        other => panic!("expected Done, got {other:?}"),
    }

    let events = EventStore::replay(env.events_path(session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(event.payload, EventPayload::PermissionDecision { ref decision, ref scope, .. } if decision == "allow" && scope == "session")));
    assert!(events.iter().any(|event| matches!(event.payload, EventPayload::ToolResult { ref status, .. } if status == "ok")));
}

#[tokio::test(flavor = "current_thread")]
async fn run_convenience_path_auto_denies_and_continues_when_approval_is_needed() {
    let env = TestEnv::new();
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""messages":[{"content":[{"text":"run tests","type":"text"}],"role":"user"}]"#);
        then.status(200).json_body(serde_json::json!({
            "id": "msg_tool",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Need approval."},
                {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"command": "cargo test", "timeout": 60}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 5, "output_tokens": 6}
        }));
    });

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#)
            .body_contains("permission gate denied this tool call");
        then.status(200).json_body(serde_json::json!({
            "id": "msg_final",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "Command was blocked."}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 8, "output_tokens": 5}
        }));
    });

    let output = query("run tests")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .run()
        .await
        .unwrap();

    assert_eq!(output.text, "Command was blocked.");
    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(event.payload, EventPayload::PermissionDecision { ref decision, ref scope, .. } if decision == "deny" && scope == "once")));
    assert!(events.iter().any(|event| matches!(event.payload, EventPayload::ToolResult { ref status, .. } if status == "blocked")));
}
