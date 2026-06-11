mod common;

use common::mock_provider;
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
            ts: "2026-06-09T00:00:04.500Z".into(),
            turn: 1,
            request_id: "req_main".into(),
            text: "main model response".into(),
            thinking: None,
            input_tokens_total: None,
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
async fn nonexistent_session_returns_not_found() {
    let mock = mock_provider::start_mock_provider().await;
    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;

    let client = wreq::Client::new();
    let resp = client
        .get(format!("{}/sessions/nonexistent/events", server.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["code"], "session_not_found");
}

#[tokio::test]
async fn health_passes_even_with_password_set() {
    let mock = mock_provider::start_mock_provider().await;
    let config = mock_provider::make_test_config(mock.port());
    let server =
        common::TestServer::start_with_password(config, Some("testpass".to_string())).await;

    let client = wreq::Client::new();
    let resp = client
        .get(format!("{}/health", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn events_can_filter_by_conversation_and_keep_session_envelope() {
    let mock = mock_provider::start_mock_provider().await;
    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;
    let session_id = "s_events_conversation_filter";
    write_session_events(server.home.path(), server.workspace.path(), session_id);

    let client = wreq::Client::new();
    let resp = client
        .get(format!(
            "{}/sessions/{}/events?workspace={}&conversation=review&after=0",
            server.base_url,
            session_id,
            server.workspace.path().display()
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let events = body.as_array().unwrap();
    assert!(events
        .iter()
        .any(|event| event["payload"]["kind"] == "session.created"));
    assert!(events
        .iter()
        .any(|event| event["payload"]["conversation"] == "review"));
    assert!(!events
        .iter()
        .any(|event| event["payload"]["text"] == "main message"));
    assert!(!events
        .iter()
        .any(|event| event["payload"]["kind"] == "model.response"));
    assert!(!events
        .iter()
        .any(|event| event["payload"]["kind"] == "tool.call"));
    assert!(!events
        .iter()
        .any(|event| event["payload"]["kind"] == "tool.result"));
}

#[tokio::test]
async fn main_conversation_filter_keeps_unscoped_main_facts() {
    let mock = mock_provider::start_mock_provider().await;
    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;
    let session_id = "s_events_main_filter";
    write_session_events(server.home.path(), server.workspace.path(), session_id);

    let client = wreq::Client::new();
    let resp = client
        .get(format!(
            "{}/sessions/{}/events?workspace={}&conversation=main&after=0",
            server.base_url,
            session_id,
            server.workspace.path().display()
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let events = body.as_array().unwrap();
    assert!(events
        .iter()
        .any(|event| event["payload"]["text"] == "main message"));
    assert!(events
        .iter()
        .any(|event| event["payload"]["kind"] == "model.response"));
    assert!(events
        .iter()
        .any(|event| event["payload"]["kind"] == "tool.call"));
    assert!(events
        .iter()
        .any(|event| event["payload"]["kind"] == "tool.result"));
    assert!(!events
        .iter()
        .any(|event| event["payload"]["text"] == "review message"));
}

#[tokio::test]
async fn events_without_conversation_returns_full_ledger() {
    let mock = mock_provider::start_mock_provider().await;
    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;
    let session_id = "s_events_full_ledger";
    write_session_events(server.home.path(), server.workspace.path(), session_id);

    let client = wreq::Client::new();
    let resp = client
        .get(format!(
            "{}/sessions/{}/events?workspace={}",
            server.base_url,
            session_id,
            server.workspace.path().display()
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let events = body.as_array().unwrap();
    assert!(events
        .iter()
        .any(|event| event["payload"]["text"] == "main message"));
    assert!(events
        .iter()
        .any(|event| event["payload"]["text"] == "review message"));
}
