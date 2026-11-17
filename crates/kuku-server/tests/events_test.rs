mod common;

use common::{mock_provider, TestServer};
use kuku::event::{EventPayload, EventStore};

fn write_session_events(home: &std::path::Path, workspace: &std::path::Path, session_id: &str) {
    let events_path = kuku::session::session_events_path(home, workspace, session_id).unwrap();
    let mut store = EventStore::open(&events_path).unwrap();
    store
        .append(EventPayload::SessionCreated {
            ts: "2026-06-09T00:00:00Z".into(),
            schema_version: 1,
            session_id: session_id.into(),
            created_at: "2026-06-09T00:00:00Z".into(),
            kuku_version: "test".into(),
        })
        .unwrap();
    store
        .append(EventPayload::ConversationOpened {
            ts: "2026-06-09T00:00:01Z".into(),
            conversation: "main".into(),
        })
        .unwrap();
    store
        .append(EventPayload::ConversationOpened {
            ts: "2026-06-09T00:00:02Z".into(),
            conversation: "review".into(),
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            ts: "2026-06-09T00:00:03Z".into(),
            conversation: "main".into(),
            turn: 1,
            text: "main message".into(),
            from: None,
            via_tool_call_id: None,
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            ts: "2026-06-09T00:00:04Z".into(),
            conversation: "review".into(),
            turn: 1,
            text: "review message".into(),
            from: None,
            via_tool_call_id: None,
        })
        .unwrap();
    store
        .append(EventPayload::ModelResponse {
            conversation: None,
            ts: "2026-06-09T00:00:04.500Z".into(),
            turn: 1,
            request_id: "req_main".into(),
            text: "main model response".into(),
            thinking: None,
            input_tokens_total: None,
            output_tokens_total: None,
            stop_reason: None,
        })
        .unwrap();
    store
        .append(EventPayload::ToolCall {
            ts: "2026-06-09T00:00:04.600Z".into(),
            turn: 1,
            conversation: None,
            tool_call_id: "tool_main".into(),
            request_id: "req_main".into(),
            index: 0,
            tool: "read_file".into(),
            args: serde_json::json!({"path": "README.md"}),
        })
        .unwrap();
    store
        .append(EventPayload::ToolResult {
            ts: "2026-06-09T00:00:04.700Z".into(),
            turn: 1,
            conversation: None,
            tool_call_id: "tool_main".into(),
            status: "ok".into(),
            summary: "read README.md".into(),
            model_content: "contents".into(),
            truncated: false,
            files_read: vec!["README.md".into()],
            files_changed: Vec::new(),
            commands_run: Vec::new(),
            memory_changed: None,
            structured: None,
        })
        .unwrap();
    store
        .append(EventPayload::TurnCompleted {
            ts: "2026-06-09T00:00:05Z".into(),
            conversation: "review".into(),
            turn: 1,
        })
        .unwrap();
}

#[tokio::test]
async fn nonexistent_session_returns_structured_not_found() {
    let mock = mock_provider::start_mock_provider().await;
    let server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let response = server
        .session_request("nonexistent", "events", &[])
        .send()
        .await
        .unwrap();
    assert_eq!(404, response.status());
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(false, body["ok"]);
    assert_eq!("session_not_found", body["code"]);
}

#[tokio::test]
async fn raw_event_pages_filter_agent_but_advance_the_full_ledger_cursor() {
    let mock = mock_provider::start_mock_provider().await;
    let server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let id = "raw-event-pages";
    write_session_events(server.home.path(), server.workspace.path(), id);

    let review: serde_json::Value = server
        .session_request(id, "events", &[("agent", "review")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let review_entries = review["events"].as_array().unwrap();
    assert_eq!(4, review_entries.len());
    assert_eq!("session.created", review_entries[0]["payload"]["kind"]);
    assert_eq!("review message", review_entries[2]["payload"]["text"]);

    let main: serde_json::Value = server
        .session_request(id, "events", &[("agent", "main")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let main_entries = main["events"].as_array().unwrap();
    assert_eq!(6, main_entries.len());
    assert!(main_entries
        .iter()
        .any(|entry| entry["payload"]["kind"] == "tool.result"));
    assert!(main_entries
        .iter()
        .all(|entry| entry["payload"]["conversation"] != "review"));

    let page: serde_json::Value = server
        .session_request(
            id,
            "events",
            &[("agent", "missing"), ("after", "1"), ("limit", "2")],
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(page["events"].as_array().unwrap().is_empty());
    assert_eq!(3, page["next_after"]);

    let full: serde_json::Value = server
        .session_request(id, "events", &[])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(9, full["events"].as_array().unwrap().len());
    assert_eq!(9, full["next_after"]);
}

#[tokio::test]
async fn history_pages_are_bounded_by_the_requested_snapshot_cursor() {
    let mock = mock_provider::start_mock_provider().await;
    let server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let id = "history-boundary";
    write_session_events(server.home.path(), server.workspace.path(), id);
    let mut store = EventStore::open(server.events_path(id)).unwrap();
    let cursor = store.last_id();
    store
        .append(EventPayload::MessageUser {
            ts: "2026-06-09T00:00:06Z".into(),
            conversation: "main".into(),
            turn: 2,
            text: "message after the saved cursor".into(),
            from: None,
            via_tool_call_id: None,
        })
        .unwrap();

    let before = (cursor + 1).to_string();
    let page: serde_json::Value = server
        .session_request(
            id,
            "history",
            &[("agent", "main"), ("before", &before), ("limit", "2")],
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!("tool_call", page["entries"][0]["type"]);
    assert_eq!("tool_result", page["entries"][1]["type"]);
    assert_eq!(7, page["next_before"]);
    let older: serde_json::Value = server
        .session_request(id, "history", &[("before", "7"), ("limit", "3")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!("main message", older["entries"][0]["text"]);
    assert_eq!("main model response", older["entries"][1]["text"]);
    assert!(older["next_before"].is_null());

    let observer = server
        .session_request(
            id,
            "observe",
            &[("agent", "main"), ("after", &cursor.to_string())],
        )
        .send()
        .await
        .unwrap();
    let mut observer = observer.bytes_stream();
    let frame = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        common::stream::next_json_line(
            &mut observer,
            &mut Vec::new(),
            "first post-snapshot commit",
        ),
    )
    .await
    .unwrap();
    assert_eq!(cursor + 1, frame["id"].as_u64().unwrap());
    assert_eq!("message after the saved cursor", frame["entry"]["text"]);
}

#[tokio::test]
async fn file_completion_exposes_only_visible_workspace_relative_paths() {
    let mock = mock_provider::start_mock_provider().await;
    let server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let workspace = server.workspace.path();
    std::fs::create_dir(workspace.join("src")).unwrap();
    std::fs::create_dir(workspace.join(".ssh")).unwrap();
    std::fs::write(workspace.join("src/visible.rs"), b"").unwrap();
    std::fs::write(workspace.join(".env"), b"test secret").unwrap();
    std::fs::write(workspace.join(".ssh/id_rsa"), b"test key").unwrap();
    let response: serde_json::Value = server
        .request(wreq::Method::GET, "/workspaces/files")
        .query(&[("workspace", workspace.to_str().unwrap()), ("query", "")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        serde_json::json!(["src/", "src/visible.rs"]),
        response["paths"]
    );
    let response = server
        .request(wreq::Method::GET, "/workspaces/files")
        .query(&[("workspace", workspace.to_str().unwrap()), ("query", "../")])
        .send()
        .await
        .unwrap();
    assert_eq!(400, response.status());
}
