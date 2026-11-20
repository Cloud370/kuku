use std::path::Path;
use std::time::Duration;

use httpmock::prelude::{HttpMockRequest, MockServer, POST};
use httpmock::Mock;
use serde_json::{json, Value};

use kuku::config::{ApiKey, ProviderAuth, ProviderProfile};
use kuku::event::{EventPayload, EventStore, StoredEvent};
use kuku::test_support::read_active_journal;
use kuku::{AgentConfig, AgentSnapshot, AgentStatus, Input, Session, SessionOptions};

use super::common;

const MODEL: &str = "claude-sonnet-4-6";

#[allow(clippy::too_many_arguments)]
pub(super) fn session_options(
    server: &MockServer,
    workspace: &Path,
    data: &Path,
    session_id: &str,
    connection: &str,
    path: &str,
    profile: ProviderProfile,
    api_key: &str,
) -> SessionOptions {
    let mut config = common::test_config();
    let mut provider = config.providers.get("anthropic").unwrap().clone();
    provider.base_url = server.url(path);
    provider.options.auth = Some(ProviderAuth::ApiKey);
    provider.options.profile = profile;
    provider.api_key = ApiKey::Plaintext(api_key.into());
    config.providers.insert(connection.into(), provider);
    let tier = config.tiers.get_mut("balanced").unwrap();
    tier.provider = connection.into();
    tier.model = MODEL.into();
    config.handoff.enabled = false;
    config.plugin.enabled = false;
    let mut options = SessionOptions::new(workspace, data, config);
    options.session_id = Some(session_id.into());
    options
}

pub(super) fn add_connection(
    options: &mut SessionOptions,
    server: &MockServer,
    name: &str,
    path: &str,
    profile: ProviderProfile,
    api_key: &str,
) {
    let mut provider = options.config.providers.get("compatible").unwrap().clone();
    provider.base_url = server.url(path);
    provider.options.profile = profile;
    provider.api_key = ApiKey::Plaintext(api_key.into());
    options.config.providers.insert(name.into(), provider);
}

pub(super) fn agent_config(connection: &str) -> AgentConfig {
    AgentConfig {
        connection: Some(connection.into()),
        model: Some(MODEL.into()),
        ..Default::default()
    }
}

fn response_with_stop(id: &str, content: Value, stop_reason: &str) -> String {
    common::anthropic_sse_response(json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "content": content,
        "stop_reason": stop_reason
    }))
}

pub(super) async fn mock_reply<'a>(
    server: &'a MockServer,
    endpoint: &str,
    marker: &str,
    api_key: &str,
    id: &str,
    content: Value,
) -> Mock<'a> {
    mock_reply_with_stop(server, endpoint, marker, api_key, id, content, "end_turn").await
}

pub(super) async fn mock_reply_with_stop<'a>(
    server: &'a MockServer,
    endpoint: &str,
    marker: &str,
    api_key: &str,
    id: &str,
    content: Value,
    stop_reason: &str,
) -> Mock<'a> {
    let body = response_with_stop(id, content, stop_reason);
    server
        .mock_async(|when, then| {
            when.method(POST)
                .path(endpoint)
                .header("x-api-key", api_key)
                .body_contains(marker)
                .matches(capture_body);
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(body);
        })
        .await
}

fn capture_body(request: &HttpMockRequest) -> bool {
    common::capture_http_request(request)
}

pub(super) async fn send(
    session: &Session,
    command: &str,
    prompt: &str,
    expected_output: Option<&str>,
) -> AgentSnapshot {
    session.send(Input::new(command, "main", prompt)).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = session.agent("main").unwrap();
            let completed = match expected_output {
                Some(output) => snapshot.last_output.as_deref() == Some(output),
                None => snapshot
                    .last_error
                    .as_deref()
                    .is_some_and(|error| error.contains("frozen provider route or capabilities")),
            };
            if snapshot.status != AgentStatus::Running && completed {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "snapshot fixture did not settle for {command}: {:?}",
            session.agent("main")
        )
    })
}

pub(super) async fn wait_for_output(session: &Session, expected: &str) -> AgentSnapshot {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = session.agent("main").unwrap();
            if snapshot.status != AgentStatus::Running
                && snapshot.last_output.as_deref() == Some(expected)
            {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("snapshot fixture did not produce the expected output")
}

pub(super) fn facts(events: &[StoredEvent], kind: &str) -> Vec<Value> {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::Unknown(value) if value["kind"] == kind && value["agent"] == "main" => {
                Some(value.clone())
            }
            _ => None,
        })
        .collect()
}

fn assert_interrupted_for_route_snapshot(snapshot: &AgentSnapshot) {
    assert_ne!(AgentStatus::Running, snapshot.status);
    assert!(
        snapshot.last_error.as_deref().is_some_and(|error| {
            error.contains("frozen provider route or capabilities")
                && error.contains("restore the recorded route/profile")
                && error.contains("configure_agent")
                && error.contains("start a new Session")
        }),
        "unexpected route snapshot error: {:?}",
        snapshot.last_error
    );
}

fn assert_prefix<T: std::fmt::Debug + PartialEq>(prefix: &[T], values: &[T]) {
    assert!(values.starts_with(prefix), "existing event facts changed");
}

#[tokio::test]
async fn same_route_reopen_keeps_snapshot_segment_and_admitted_prefix() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let base_path = "/same-route-snapshot";
    let endpoint = format!("{base_path}/v1/messages");
    let second = mock_reply(
        &server,
        &endpoint,
        "same route follow-up",
        "test-key",
        "same_route_second",
        json!([{"type":"text","text":"continued"}]),
    )
    .await;
    let first = mock_reply(
        &server,
        &endpoint,
        "same route seed",
        "test-key",
        "same_route_first",
        json!([{"type":"text","text":"stable historical answer"}]),
    )
    .await;
    let options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "same_route_snapshot",
        "compatible",
        base_path,
        ProviderProfile::Compatible,
        "test-key",
    );
    let session = Session::open(options.clone()).await.unwrap();
    session
        .create_agent("create", "main", agent_config("compatible"))
        .unwrap();
    let first_snapshot = send(
        &session,
        "seed",
        "same route seed",
        Some("stable historical answer"),
    )
    .await;
    assert_eq!(
        AgentStatus::Idle,
        first_snapshot.status,
        "{:?}",
        first_snapshot.last_error
    );
    first.assert_hits_async(1).await;
    let events_path = session.events_path().to_path_buf();
    let events_before = EventStore::replay(&events_path).unwrap();
    let segments_before = facts(&events_before, "runtime.context_segment");
    let admissions_before = facts(&events_before, "runtime.context_admitted");
    assert_eq!(1, segments_before.len());
    assert_eq!("compatible", segments_before[0]["capabilities"]["profile"]);
    assert_eq!(
        "compatible",
        segments_before[0]["route_selectors"]["query_connection"]
    );
    session.close().await.unwrap();
    let bytes_before = std::fs::read(&events_path).unwrap();

    let reopened = Session::open(options).await.unwrap();
    let second_snapshot = send(
        &reopened,
        "follow-up",
        "same route follow-up",
        Some("continued"),
    )
    .await;
    assert_eq!(
        AgentStatus::Idle,
        second_snapshot.status,
        "{:?}",
        second_snapshot.last_error
    );
    second.assert_hits_async(1).await;
    reopened.close().await.unwrap();

    let bytes_after = std::fs::read(&events_path).unwrap();
    assert!(bytes_after.starts_with(&bytes_before));
    let events_after = EventStore::replay(&events_path).unwrap();
    let segments_after = facts(&events_after, "runtime.context_segment");
    let admissions_after = facts(&events_after, "runtime.context_admitted");
    assert_eq!(segments_before, segments_after);
    assert_prefix(&admissions_before, &admissions_after);
    let bodies = common::take_http_requests(&endpoint);
    assert_eq!(2, bodies.len());
    assert_eq!(bodies[0]["system"], bodies[1]["system"]);
    assert_eq!(bodies[0]["tools"], bodies[1]["tools"]);
    let first_messages = bodies[0]["messages"].as_array().unwrap();
    let second_messages = bodies[1]["messages"].as_array().unwrap();
    assert_eq!(
        first_messages.as_slice(),
        &second_messages[..first_messages.len()]
    );
    let mut previous_messages = first_messages.clone();
    previous_messages.push(json!({
        "role": "assistant",
        "content": [{"type": "text", "text": "stable historical answer"}]
    }));
    assert_eq!(
        previous_messages.as_slice(),
        &second_messages[..previous_messages.len()]
    );
}

async fn assert_passive_profile_flip(
    server: &MockServer,
    workspace: &Path,
    data: &Path,
    name: &str,
    from: ProviderProfile,
    to: ProviderProfile,
) {
    let base_path = format!("/{name}");
    let endpoint = format!("{base_path}/v1/messages");
    let blocked = mock_reply(
        server,
        &endpoint,
        "passive profile follow-up",
        "test-key",
        &format!("{name}_blocked"),
        json!([{"type":"text","text":"restored"}]),
    )
    .await;
    let first = mock_reply(
        server,
        &endpoint,
        "profile seed",
        "test-key",
        &format!("{name}_seed"),
        json!([
            {"type":"thinking","thinking":"unsigned private thought"},
            {"type":"text","text":"safe prior answer"}
        ]),
    )
    .await;
    let original_options = session_options(
        server, workspace, data, name, "route", &base_path, from, "test-key",
    );
    let session = Session::open(original_options.clone()).await.unwrap();
    session
        .create_agent("create", "main", agent_config("route"))
        .unwrap();
    let first_snapshot = send(&session, "seed", "profile seed", Some("safe prior answer")).await;
    assert_eq!(
        AgentStatus::Idle,
        first_snapshot.status,
        "{:?}",
        first_snapshot.last_error
    );
    first.assert_hits_async(1).await;
    let events_path = session.events_path().to_path_buf();
    let events_before = EventStore::replay(&events_path).unwrap();
    let segments_before = facts(&events_before, "runtime.context_segment");
    let admissions_before = facts(&events_before, "runtime.context_admitted");
    session.close().await.unwrap();
    let bytes_before = std::fs::read(&events_path).unwrap();

    let changed_options = session_options(
        server, workspace, data, name, "route", &base_path, to, "test-key",
    );
    let changed = Session::open(changed_options).await.unwrap();
    let blocked_snapshot = send(
        &changed,
        "profile-change",
        "passive profile follow-up",
        None,
    )
    .await;
    assert_interrupted_for_route_snapshot(&blocked_snapshot);
    blocked.assert_hits_async(0).await;
    changed.close().await.unwrap();
    let bytes_after_block = std::fs::read(&events_path).unwrap();
    assert!(bytes_after_block.starts_with(&bytes_before));
    let events_after_block = EventStore::replay(&events_path).unwrap();
    assert_eq!(
        segments_before,
        facts(&events_after_block, "runtime.context_segment")
    );
    assert_eq!(
        admissions_before,
        facts(&events_after_block, "runtime.context_admitted")
    );
    let restored = Session::open(original_options).await.unwrap();
    restored.advance("main").unwrap();
    let restored_snapshot = wait_for_output(&restored, "restored").await;
    assert_eq!(
        AgentStatus::Idle,
        restored_snapshot.status,
        "{:?}",
        restored_snapshot.last_error
    );
    blocked.assert_hits_async(1).await;
    restored.close().await.unwrap();
    let events_after_restore = EventStore::replay(&events_path).unwrap();
    assert_eq!(
        segments_before,
        facts(&events_after_restore, "runtime.context_segment")
    );
    assert_prefix(
        &admissions_before,
        &facts(&events_after_restore, "runtime.context_admitted"),
    );
}

#[tokio::test]
async fn passive_profile_flips_in_both_directions_stop_and_restore() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    assert_passive_profile_flip(
        &server,
        workspace.path(),
        data.path(),
        "native_to_compatible_snapshot",
        ProviderProfile::Anthropic,
        ProviderProfile::Compatible,
    )
    .await;
    assert_passive_profile_flip(
        &server,
        workspace.path(),
        data.path(),
        "compatible_to_native_snapshot",
        ProviderProfile::Compatible,
        ProviderProfile::Anthropic,
    )
    .await;
}

#[tokio::test]
async fn passive_endpoint_change_is_rejected_before_dispatch() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let old_path = "/endpoint-snapshot-old";
    let new_path = "/endpoint-snapshot-new";
    let old_endpoint = format!("{old_path}/v1/messages");
    let new_endpoint = format!("{new_path}/v1/messages");
    let first = mock_reply(
        &server,
        &old_endpoint,
        "endpoint seed",
        "test-key",
        "endpoint_seed",
        json!([{"type":"text","text":"endpoint answer"}]),
    )
    .await;
    let blocked = mock_reply(
        &server,
        &new_endpoint,
        "endpoint follow-up",
        "test-key",
        "endpoint_blocked",
        json!([{"type":"text","text":"must not dispatch"}]),
    )
    .await;
    let original_options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "endpoint_snapshot",
        "route",
        old_path,
        ProviderProfile::Compatible,
        "test-key",
    );
    let session = Session::open(original_options).await.unwrap();
    session
        .create_agent("create", "main", agent_config("route"))
        .unwrap();
    let first_snapshot = send(&session, "seed", "endpoint seed", Some("endpoint answer")).await;
    assert_eq!(
        AgentStatus::Idle,
        first_snapshot.status,
        "{:?}",
        first_snapshot.last_error
    );
    first.assert_hits_async(1).await;
    let events_path = session.events_path().to_path_buf();
    let events_before = EventStore::replay(&events_path).unwrap();
    let segments_before = facts(&events_before, "runtime.context_segment");
    let admissions_before = facts(&events_before, "runtime.context_admitted");
    session.close().await.unwrap();
    let bytes_before = std::fs::read(&events_path).unwrap();

    let changed_options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "endpoint_snapshot",
        "route",
        new_path,
        ProviderProfile::Compatible,
        "test-key",
    );
    let changed = Session::open(changed_options).await.unwrap();
    let snapshot = send(&changed, "endpoint-change", "endpoint follow-up", None).await;
    assert_interrupted_for_route_snapshot(&snapshot);
    blocked.assert_hits_async(0).await;
    changed.close().await.unwrap();
    let bytes_after = std::fs::read(&events_path).unwrap();
    assert!(bytes_after.starts_with(&bytes_before));
    let events_after = EventStore::replay(&events_path).unwrap();
    assert_eq!(
        segments_before,
        facts(&events_after, "runtime.context_segment")
    );
    assert_eq!(
        admissions_before,
        facts(&events_after, "runtime.context_admitted")
    );
}

#[tokio::test]
async fn api_key_rotation_keeps_segment_and_never_persists_credentials() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let base_path = "/key-rotation-snapshot";
    let endpoint = format!("{base_path}/v1/messages");
    let first = mock_reply(
        &server,
        &endpoint,
        "key seed",
        "old-session-secret",
        "key_seed",
        json!([{"type":"text","text":"key answer"}]),
    )
    .await;
    let second = mock_reply(
        &server,
        &endpoint,
        "key follow-up",
        "rotated-session-secret",
        "key_followup",
        json!([{"type":"text","text":"key rotated"}]),
    )
    .await;
    let original_options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "key_rotation_snapshot",
        "route",
        base_path,
        ProviderProfile::Compatible,
        "old-session-secret",
    );
    let session = Session::open(original_options).await.unwrap();
    session
        .create_agent("create", "main", agent_config("route"))
        .unwrap();
    let first_snapshot = send(&session, "seed", "key seed", Some("key answer")).await;
    assert_eq!(
        AgentStatus::Idle,
        first_snapshot.status,
        "{:?}",
        first_snapshot.last_error
    );
    first.assert_hits_async(1).await;
    let events_path = session.events_path().to_path_buf();
    let before = EventStore::replay(&events_path).unwrap();
    let segments_before = facts(&before, "runtime.context_segment");
    session.close().await.unwrap();

    let rotated_options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "key_rotation_snapshot",
        "route",
        base_path,
        ProviderProfile::Compatible,
        "rotated-session-secret",
    );
    let reopened = Session::open(rotated_options).await.unwrap();
    let rotated_snapshot = send(&reopened, "follow-up", "key follow-up", Some("key rotated")).await;
    assert_eq!(
        AgentStatus::Idle,
        rotated_snapshot.status,
        "{:?}",
        rotated_snapshot.last_error
    );
    second.assert_hits_async(1).await;
    reopened.close().await.unwrap();

    let events_after = EventStore::replay(&events_path).unwrap();
    assert_eq!(
        segments_before,
        facts(&events_after, "runtime.context_segment")
    );
    let bytes = std::fs::read(&events_path).unwrap();
    let journal = String::from_utf8(bytes).unwrap();
    assert!(!journal.contains("old-session-secret"));
    assert!(!journal.contains("rotated-session-secret"));
}

#[tokio::test]
async fn unrelated_agent_configuration_does_not_authorize_profile_drift() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let base_path = "/unrelated-config-snapshot";
    let endpoint = format!("{base_path}/v1/messages");
    let blocked = mock_reply(
        &server,
        &endpoint,
        "unrelated configure follow-up",
        "test-key",
        "unrelated_blocked",
        json!([{"type":"text","text":"must not dispatch"}]),
    )
    .await;
    let first = mock_reply(
        &server,
        &endpoint,
        "unrelated configure seed",
        "test-key",
        "unrelated_seed",
        json!([
            {"type":"thinking","thinking":"unsigned private thought"},
            {"type":"text","text":"safe answer"}
        ]),
    )
    .await;
    let compatible_options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "unrelated_config_snapshot",
        "route",
        base_path,
        ProviderProfile::Compatible,
        "test-key",
    );
    let session = Session::open(compatible_options).await.unwrap();
    session
        .create_agent("create", "main", agent_config("route"))
        .unwrap();
    let first_snapshot = send(
        &session,
        "seed",
        "unrelated configure seed",
        Some("safe answer"),
    )
    .await;
    assert_eq!(
        AgentStatus::Idle,
        first_snapshot.status,
        "{:?}",
        first_snapshot.last_error
    );
    first.assert_hits_async(1).await;
    let events_path = session.events_path().to_path_buf();
    let events_before = EventStore::replay(&events_path).unwrap();
    let segments_before = facts(&events_before, "runtime.context_segment");
    let admissions_before = facts(&events_before, "runtime.context_admitted");
    session.close().await.unwrap();
    let bytes_before = std::fs::read(&events_path).unwrap();

    let changed_options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "unrelated_config_snapshot",
        "route",
        base_path,
        ProviderProfile::Anthropic,
        "test-key",
    );
    let changed = Session::open(changed_options).await.unwrap();
    changed
        .configure_agent(
            "change-instructions",
            "main",
            AgentConfig {
                instructions: Some("updated instructions only".into()),
                ..agent_config("route")
            },
        )
        .unwrap();
    let snapshot = send(&changed, "follow-up", "unrelated configure follow-up", None).await;
    assert_interrupted_for_route_snapshot(&snapshot);
    blocked.assert_hits_async(0).await;
    changed.close().await.unwrap();
    let bytes_after = std::fs::read(&events_path).unwrap();
    assert!(bytes_after.starts_with(&bytes_before));
    let events_after = EventStore::replay(&events_path).unwrap();
    assert_eq!(
        segments_before,
        facts(&events_after, "runtime.context_segment")
    );
    assert_eq!(
        admissions_before,
        facts(&events_after, "runtime.context_admitted")
    );
}

#[tokio::test]
async fn explicit_distinct_native_connection_creates_successor_without_unsigned_thinking() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let tool_file_contents = "explicit route tool history content";
    std::fs::write(workspace.path().join("history.txt"), tool_file_contents).unwrap();
    let compatible_path = "/explicit-route-compatible";
    let native_path = "/explicit-route-native";
    let compatible_endpoint = format!("{compatible_path}/v1/messages");
    let native_endpoint = format!("{native_path}/v1/messages");
    let compatible_final = mock_reply(
        &server,
        &compatible_endpoint,
        tool_file_contents,
        "compatible-key",
        "explicit_route_compatible_final",
        json!([{"type":"text","text":"safe compatible answer"}]),
    )
    .await;
    let first = mock_reply_with_stop(
        &server,
        &compatible_endpoint,
        "explicit route seed",
        "compatible-key",
        "explicit_route_seed",
        json!([
            {"type":"thinking","thinking":"unsigned private thought"},
            {"type":"text","text":"I will read the history file."},
            {"type":"tool_use","id":"history-read","name":"read_file","input":{"path":"history.txt"}}
        ]),
        "tool_use",
    )
    .await;
    let second = mock_reply(
        &server,
        &native_endpoint,
        "explicit route follow-up",
        "native-key",
        "explicit_route_native",
        json!([{"type":"text","text":"native continuation"}]),
    )
    .await;
    let mut options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "explicit_route_snapshot",
        "compatible",
        compatible_path,
        ProviderProfile::Compatible,
        "compatible-key",
    );
    add_connection(
        &mut options,
        &server,
        "native",
        native_path,
        ProviderProfile::Anthropic,
        "native-key",
    );
    let session = Session::open(options).await.unwrap();
    let mut compatible_agent = agent_config("compatible");
    compatible_agent.tools = Some(vec!["read_file".into()]);
    session
        .create_agent("create", "main", compatible_agent)
        .unwrap();
    let first_snapshot = send(
        &session,
        "seed",
        "explicit route seed",
        Some("safe compatible answer"),
    )
    .await;
    assert_eq!(
        AgentStatus::Idle,
        first_snapshot.status,
        "{:?}",
        first_snapshot.last_error
    );
    first.assert_hits_async(1).await;
    compatible_final.assert_hits_async(1).await;
    let events_path = session.events_path().to_path_buf();
    let bytes_before = read_active_journal(&events_path).unwrap();
    let events_before = EventStore::replay(&events_path).unwrap();
    let segments_before = facts(&events_before, "runtime.context_segment");
    let admissions_before = facts(&events_before, "runtime.context_admitted");
    assert_eq!(1, segments_before.len());
    let tool_call_id = events_before
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::ToolCall {
                tool_call_id, tool, ..
            } if tool == "read_file" => Some(tool_call_id.clone()),
            _ => None,
        })
        .expect("read_file call should be durable before the route transition");
    assert!(events_before.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolResult {
            tool_call_id: result_id,
            status,
            model_content,
            ..
        } if result_id == &tool_call_id && status == "ok" && model_content.contains(tool_file_contents)
    )));
    let predecessor_id = events_before
        .iter()
        .find(|event| matches!(&event.payload, EventPayload::Unknown(value) if value["kind"] == "runtime.context_segment"))
        .expect("initial context segment should be durable")
        .id;
    session
        .configure_agent("select-native", "main", agent_config("native"))
        .unwrap();
    let second_snapshot = send(
        &session,
        "follow-up",
        "explicit route follow-up",
        Some("native continuation"),
    )
    .await;
    assert_eq!(
        AgentStatus::Idle,
        second_snapshot.status,
        "{:?}",
        second_snapshot.last_error
    );
    second.assert_hits_async(1).await;
    session.close().await.unwrap();

    let bytes_after = std::fs::read(&events_path).unwrap();
    assert!(bytes_after.starts_with(&bytes_before));
    let events_after = EventStore::replay(&events_path).unwrap();
    let segments_after = facts(&events_after, "runtime.context_segment");
    let admissions_after = facts(&events_after, "runtime.context_admitted");
    assert_eq!(2, segments_after.len());
    assert_eq!(segments_before[0], segments_after[0]);
    assert_eq!(
        Some(predecessor_id),
        segments_after[1]["predecessor"].as_u64()
    );
    assert_ne!(
        segments_before[0]["signature"],
        segments_after[1]["signature"]
    );
    assert_eq!("configuration", segments_after[1]["reason"]);
    let configured_id = events_after
        .iter()
        .find(|event| {
            matches!(&event.payload, EventPayload::Unknown(value)
            if value["kind"] == "runtime.operation"
                && value["record"]["operation"] == "agent_configured"
                && value["record"]["address"] == "main")
        })
        .expect("explicit route configuration should be durable")
        .id;
    assert_eq!(
        Some(configured_id),
        segments_after[1]["configuration"].as_u64()
    );
    assert_eq!("anthropic", segments_after[1]["capabilities"]["profile"]);
    assert_eq!(
        "native",
        segments_after[1]["route_selectors"]["query_connection"]
    );
    assert_prefix(&admissions_before, &admissions_after);

    let bodies = common::take_http_requests(&native_endpoint);
    assert_eq!(1, bodies.len());
    let messages = bodies[0]["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["role"] == "assistant"
            && message["content"].as_array().is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    block["type"] == "tool_use"
                        && block["id"] == tool_call_id
                        && block["name"] == "read_file"
                        && block["input"]["path"] == "history.txt"
                })
            })
    }));
    assert!(messages.iter().any(|message| {
        message["role"] == "user"
            && message["content"].as_array().is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    block["type"] == "tool_result"
                        && block["tool_use_id"] == tool_call_id
                        && block["content"]
                            .as_str()
                            .is_some_and(|content| content.contains(tool_file_contents))
                })
            })
    }));
    let serialized = serde_json::to_string(&bodies[0]).unwrap();
    assert!(serialized.contains("safe compatible answer"));
    assert!(serialized.contains("I will read the history file."));
    assert!(!serialized.contains("unsigned private thought"));
    assert!(!serialized.contains("signature"));
}
