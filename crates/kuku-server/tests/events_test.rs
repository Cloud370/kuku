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
            job_delivery: false,
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
            job_delivery: false,
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

    let page: serde_json::Value = server
        .session_request(id, "events", &[("reverse", "true"), ("limit", "2")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(8, page["next_before"]);
    assert_eq!(8, page["events"][0]["id"]);
    assert_eq!(9, page["events"][1]["id"]);

    let older: serde_json::Value = server
        .session_request(
            id,
            "events",
            &[("before", "8"), ("limit", "2"), ("agent", "missing")],
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(6, older["next_before"]);
    assert!(older["events"].as_array().unwrap().is_empty());
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
            job_delivery: false,
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

fn write_rewind_group_events(
    server: &TestServer,
    session_id: &str,
) -> (u64, u64, u64, u64, u64, u64, u64) {
    let mut store = EventStore::open(server.events_path(session_id)).unwrap();
    store
        .append(EventPayload::SessionCreated {
            ts: "2026-06-10T00:00:00Z".into(),
            schema_version: 1,
            session_id: session_id.into(),
            created_at: "2026-06-10T00:00:00Z".into(),
            kuku_version: "test".into(),
        })
        .unwrap();
    store
        .append(EventPayload::ConversationOpened {
            ts: "2026-06-10T00:00:01Z".into(),
            conversation: "main".into(),
        })
        .unwrap();
    store
        .append(EventPayload::ConversationOpened {
            ts: "2026-06-10T00:00:02Z".into(),
            conversation: "reviewer".into(),
        })
        .unwrap();
    let old_target = append_group_user(&mut store, "main", 101, "old prompt");
    append_group_response(&mut store, 101, "old reply");
    let first_target = append_group_user(&mut store, "main", 102, "first rewind prompt");
    append_group_response(&mut store, 102, "first rewind reply");
    let first_rewind = append_group_rewind(&mut store, "main", first_target);
    let reviewer_target = append_group_user(&mut store, "reviewer", 201, "reviewer prompt");
    append_group_response(&mut store, 201, "reviewer reply");
    append_group_user(&mut store, "main", 103, "later prompt");
    let later_rewind = append_group_rewind(&mut store, "main", old_target);
    let reviewer_rewind = append_group_rewind(&mut store, "reviewer", reviewer_target);
    (
        old_target,
        first_target,
        first_rewind,
        later_rewind,
        reviewer_target,
        reviewer_rewind,
        store.last_id(),
    )
}

fn append_group_user(store: &mut EventStore, agent: &str, turn: u64, text: &str) -> u64 {
    let id = store.last_id() + 1;
    store
        .append(EventPayload::MessageUser {
            ts: "2026-06-10T00:00:03Z".into(),
            conversation: agent.into(),
            turn,
            text: text.into(),
            from: None,
            via_tool_call_id: None,
            job_delivery: false,
        })
        .unwrap();
    id
}

fn append_group_response(store: &mut EventStore, turn: u64, text: &str) -> u64 {
    let id = store.last_id() + 1;
    store
        .append(EventPayload::ModelResponse {
            conversation: None,
            ts: "2026-06-10T00:00:04Z".into(),
            turn,
            request_id: format!("request-{turn}"),
            text: text.into(),
            thinking: None,
            input_tokens_total: None,
            output_tokens_total: None,
            stop_reason: None,
        })
        .unwrap();
    id
}

fn append_group_rewind(store: &mut EventStore, agent: &str, target_event_id: u64) -> u64 {
    let id = store.last_id() + 1;
    store
        .append(EventPayload::Unknown(serde_json::json!({
            "kind": "runtime.operation",
            "agent": agent,
            "record": {
                "operation": "message_rewind",
                "address": agent,
                "target_event_id": target_event_id,
            }
        })))
        .unwrap();
    id
}

#[tokio::test]
async fn rewind_history_pages_keep_active_history_filtered_and_groups_owned_once() {
    let mock = mock_provider::start_mock_provider().await;
    let server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let id = "rewind-groups";
    let (old_target, first_target, first_rewind, later_rewind, reviewer_target, reviewer_rewind, _) =
        write_rewind_group_events(&server, id);
    let journal_path = server.events_path(id);
    let original_head = EventStore::read_head(&journal_path).unwrap();

    let newest: serde_json::Value = server
        .session_request(id, "history", &[("agent", "main"), ("limit", "1")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(0, newest["entries"].as_array().unwrap().len());
    assert_eq!(1, newest["rewind_groups"].as_array().unwrap().len());
    assert_eq!(first_rewind, newest["rewind_groups"][0]["rewind_event_id"]);
    assert_eq!(first_target, newest["rewind_groups"][0]["target_event_id"]);
    assert_eq!(2, newest["rewind_groups"][0]["entry_count"]);

    let before = newest["next_before"].as_u64().unwrap().to_string();
    let older: serde_json::Value = server
        .session_request(
            id,
            "history",
            &[("agent", "main"), ("before", &before), ("limit", "1")],
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(0, older["entries"].as_array().unwrap().len());
    assert_eq!(1, older["rewind_groups"].as_array().unwrap().len());
    assert_eq!(later_rewind, older["rewind_groups"][0]["rewind_event_id"]);
    assert_eq!(old_target, older["rewind_groups"][0]["target_event_id"]);
    assert_eq!(3, older["rewind_groups"][0]["entry_count"]);
    let exhausted_cursor = older["next_before"].as_u64().unwrap().to_string();
    let exhausted: serde_json::Value = server
        .session_request(
            id,
            "history",
            &[
                ("agent", "main"),
                ("before", &exhausted_cursor),
                ("limit", "1"),
            ],
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(exhausted["rewind_groups"].as_array().unwrap().is_empty());
    assert!(exhausted["next_before"].is_null());

    let first_page: serde_json::Value = server
        .session_request(
            id,
            &format!("history/rewinds/{first_rewind}"),
            &[("agent", "main"), ("limit", "1")],
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!("first rewind reply", first_page["entries"][0]["text"]);
    assert_eq!("main", first_page["entries"][0]["agent"]);
    let page_before = first_page["next_before"].as_u64().unwrap().to_string();
    let second_page: serde_json::Value = server
        .session_request(
            id,
            &format!("history/rewinds/{first_rewind}"),
            &[("agent", "main"), ("before", &page_before), ("limit", "1")],
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!("first rewind prompt", second_page["entries"][0]["text"]);
    assert!(second_page["next_before"].is_null());

    let later_group: serde_json::Value = server
        .session_request(
            id,
            &format!("history/rewinds/{later_rewind}"),
            &[("agent", "main"), ("limit", "8")],
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let later_texts = later_group["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["text"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ["old prompt", "old reply", "later prompt"],
        later_texts.as_slice()
    );
    assert!(!later_texts.contains(&"first rewind prompt"));
    assert!(!later_texts.contains(&"first rewind reply"));

    let wrong_agent = server
        .session_request(
            id,
            &format!("history/rewinds/{first_rewind}"),
            &[("agent", "reviewer")],
        )
        .send()
        .await
        .unwrap();
    assert_eq!(404, wrong_agent.status());

    let reviewer_history: serde_json::Value = server
        .session_request(id, "history", &[("agent", "reviewer")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(reviewer_history["entries"].as_array().unwrap().is_empty());
    assert_eq!(
        reviewer_target,
        reviewer_history["rewind_groups"][0]["target_event_id"]
    );
    assert_eq!(2, reviewer_history["rewind_groups"][0]["entry_count"]);
    let reviewer_group: serde_json::Value = server
        .session_request(
            id,
            &format!("history/rewinds/{reviewer_rewind}"),
            &[("agent", "reviewer"), ("limit", "8")],
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let reviewer_texts = reviewer_group["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["text"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ["reviewer prompt", "reviewer reply"],
        reviewer_texts.as_slice()
    );
    assert_eq!(original_head, EventStore::read_head(&journal_path).unwrap());
}

#[tokio::test]
async fn rewind_group_history_rejects_a_single_entry_over_the_response_byte_cap() {
    let mock = mock_provider::start_mock_provider().await;
    let server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let id = "rewind-group-byte-cap";
    write_rewind_group_events(&server, id);
    let mut store = EventStore::open(server.events_path(id)).unwrap();
    let oversized_text = "x".repeat(1_048_200);
    let target = append_group_user(&mut store, "main", 301, &oversized_text);
    let rewind = append_group_rewind(&mut store, "main", target);

    let response = server
        .session_request(
            id,
            &format!("history/rewinds/{rewind}"),
            &[("agent", "main")],
        )
        .send()
        .await
        .unwrap();
    assert_eq!(413, response.status());
    assert!(response.bytes().await.unwrap().len() < 4096);
}
