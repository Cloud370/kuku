use std::path::{Path, PathBuf};
use std::time::Duration;

use httpmock::prelude::MockServer;
use kuku::config::ProviderProfile;
use kuku::context::ToolSchema;
use kuku::event::{EventPayload, EventStore, StoredEvent};
use kuku::{AgentSnapshot, AgentStatus, Input, ProviderSettings, Session};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::common;
use super::session_context_snapshot::{
    agent_config, facts, mock_reply, mock_reply_with_stop, send, session_options,
};

#[derive(Clone, Copy)]
pub(super) enum SnapshotFields {
    Legacy,
    MissingCapabilities,
    MissingRouteSelectors,
}

pub(super) fn context_segment(events: &[StoredEvent]) -> (&StoredEvent, &Value) {
    events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::Unknown(value) if value["kind"] == "runtime.context_segment" => {
                Some((event, value))
            }
            _ => None,
        })
        .expect("context segment fact should be durable")
}

fn legacy_signature(events: &[StoredEvent]) -> String {
    let (segment_event, segment) = context_segment(events);
    let prepared = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::Unknown(value)
                if value["kind"] == "runtime.step_prepared"
                    && value["segment"] == segment_event.id =>
            {
                Some(value)
            }
            _ => None,
        })
        .expect("prepared-step fact should be durable");
    let settings: ProviderSettings = serde_json::from_value(prepared["settings"].clone()).unwrap();
    let max_context_tokens: u32 =
        serde_json::from_value(prepared["context_window_tokens"].clone()).unwrap();
    let max_output_tokens: Option<u32> =
        serde_json::from_value(prepared["max_output_tokens"].clone()).unwrap();
    let temperature: Option<f32> = serde_json::from_value(prepared["temperature"].clone()).unwrap();
    let workspace = PathBuf::from(segment["workspace"].as_str().unwrap());
    let tools: Vec<ToolSchema> = serde_json::from_value(segment["tools"].clone()).unwrap();
    let bytes = serde_json::to_vec(&(
        segment["protocol"].as_str().unwrap(),
        segment["connection"].as_str().unwrap(),
        segment["endpoint"].as_str().unwrap(),
        segment["model"].as_str().unwrap(),
        settings,
        max_context_tokens,
        max_output_tokens,
        temperature,
        workspace,
        tools,
        segment["behavior"].as_str().unwrap(),
        segment["handoff"].as_u64().unwrap(),
    ))
    .unwrap();
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn install_legacy_fixture(
    path: &Path,
    fields: SnapshotFields,
    remove_provider_replay: bool,
) -> Vec<u8> {
    let events = EventStore::replay(path).unwrap();
    let signature = legacy_signature(&events);
    let original = std::fs::read(path).unwrap();
    let mut rewritten = Vec::with_capacity(original.len());
    let mut segments = 0;
    let mut replay_decisions = 0;

    for record in original.split_inclusive(|byte| *byte == b'\n') {
        let (line, newline) = match record.last() {
            Some(b'\n') => (&record[..record.len() - 1], true),
            _ => (record, false),
        };
        if line.is_empty() {
            if newline {
                rewritten.push(b'\n');
            }
            continue;
        }
        let mut value: Value = serde_json::from_slice(line).unwrap();
        let kind = value["kind"].as_str().unwrap_or_default();
        let changed = if kind == "runtime.context_segment" {
            segments += 1;
            value["signature"] = Value::String(signature.clone());
            let object = value.as_object_mut().unwrap();
            match fields {
                SnapshotFields::Legacy => {
                    object.remove("capabilities");
                    object.remove("route_selectors");
                }
                SnapshotFields::MissingCapabilities => {
                    object.remove("capabilities");
                }
                SnapshotFields::MissingRouteSelectors => {
                    object.remove("route_selectors");
                }
            }
            true
        } else if remove_provider_replay && kind == "runtime.provider_replay" {
            replay_decisions += 1;
            value["kind"] = Value::String("test.provider_replay_unavailable".into());
            true
        } else {
            false
        };
        if changed {
            rewritten.extend_from_slice(&serde_json::to_vec(&value).unwrap());
        } else {
            rewritten.extend_from_slice(line);
        }
        if newline {
            rewritten.push(b'\n');
        }
    }

    assert_eq!(1, segments);
    if remove_provider_replay {
        assert!(
            replay_decisions > 0,
            "fixture must remove a durable replay decision"
        );
    }
    std::fs::write(path, &rewritten).unwrap();
    let legacy_events = EventStore::replay(path).unwrap();
    let (_, legacy_segment) = context_segment(&legacy_events);
    assert_eq!(signature, legacy_segment["signature"]);
    match fields {
        SnapshotFields::Legacy => {
            assert!(legacy_segment.get("capabilities").is_none());
            assert!(legacy_segment.get("route_selectors").is_none());
        }
        SnapshotFields::MissingCapabilities => {
            assert!(legacy_segment.get("capabilities").is_none());
            assert!(legacy_segment.get("route_selectors").is_some());
        }
        SnapshotFields::MissingRouteSelectors => {
            assert!(legacy_segment.get("capabilities").is_some());
            assert!(legacy_segment.get("route_selectors").is_none());
        }
    }
    std::fs::read(path).unwrap()
}

pub(super) async fn send_and_settle(
    session: &Session,
    command: &str,
    prompt: &str,
) -> AgentSnapshot {
    session.send(Input::new(command, "main", prompt)).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = session.agent("main").unwrap();
            if snapshot.status != AgentStatus::Running {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("legacy fixture did not settle")
}

fn assert_context_facts_unchanged(before: &[StoredEvent], after: &[StoredEvent]) {
    assert_eq!(
        facts(before, "runtime.context_segment"),
        facts(after, "runtime.context_segment")
    );
    assert_eq!(
        facts(before, "runtime.context_admitted"),
        facts(after, "runtime.context_admitted")
    );
}

#[tokio::test]
async fn legacy_text_and_tool_history_reopens_with_original_prefix_and_live_key_rotation() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let contents = "legacy durable tool result";
    std::fs::write(workspace.path().join("legacy.txt"), contents).unwrap();
    let path = "/legacy-text-and-tools";
    let endpoint = format!("{path}/v1/messages");

    let followup = mock_reply(
        &server,
        &endpoint,
        "legacy follow-up",
        "legacy-rotated-key",
        "legacy_followup",
        json!([{"type":"text","text":"legacy follow-up answer"}]),
    )
    .await;
    let final_answer = mock_reply(
        &server,
        &endpoint,
        contents,
        "legacy-original-key",
        "legacy_tool_final",
        json!([{"type":"text","text":"legacy safe answer"}]),
    )
    .await;
    let tool_call = mock_reply_with_stop(
        &server,
        &endpoint,
        "legacy tool seed",
        "legacy-original-key",
        "legacy_tool_seed",
        json!([
            {"type":"text","text":"I will read the file."},
            {"type":"tool_use","id":"legacy-read","name":"read_file","input":{"path":"legacy.txt"}}
        ]),
        "tool_use",
    )
    .await;
    let options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "legacy_text_tools",
        "legacy",
        path,
        ProviderProfile::Compatible,
        "legacy-original-key",
    );
    let mut config = agent_config("legacy");
    config.tools = Some(vec!["read_file".into()]);
    let session = Session::open(options).await.unwrap();
    session.create_agent("create", "main", config).unwrap();
    let first = send(
        &session,
        "seed",
        "legacy tool seed",
        Some("legacy safe answer"),
    )
    .await;
    assert_eq!(AgentStatus::Idle, first.status, "{:?}", first.last_error);
    tool_call.assert_hits_async(1).await;
    final_answer.assert_hits_async(1).await;
    let events_path = session.events_path().to_path_buf();
    session.close().await.unwrap();
    drop(session);

    let legacy_bytes = install_legacy_fixture(&events_path, SnapshotFields::Legacy, false);
    let before = EventStore::replay(&events_path).unwrap();
    let segments_before = facts(&before, "runtime.context_segment");
    let admissions_before = facts(&before, "runtime.context_admitted");
    assert_eq!(1, segments_before.len());
    let original_signature = segments_before[0]["signature"].clone();

    let reopened_options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "legacy_text_tools",
        "legacy",
        path,
        ProviderProfile::Compatible,
        "legacy-rotated-key",
    );
    let reopened = Session::open(reopened_options).await.unwrap();
    let second = send(
        &reopened,
        "followup",
        "legacy follow-up",
        Some("legacy follow-up answer"),
    )
    .await;
    assert_eq!(AgentStatus::Idle, second.status, "{:?}", second.last_error);
    followup.assert_hits_async(1).await;
    reopened.close().await.unwrap();
    drop(reopened);

    let bytes_after = std::fs::read(&events_path).unwrap();
    assert!(bytes_after.starts_with(&legacy_bytes));
    let after = EventStore::replay(&events_path).unwrap();
    let segments_after = facts(&after, "runtime.context_segment");
    let admissions_after = facts(&after, "runtime.context_admitted");
    assert_eq!(segments_before, segments_after);
    assert_eq!(original_signature, segments_after[0]["signature"]);
    assert!(admissions_after.starts_with(&admissions_before));
    assert!(segments_after[0].get("capabilities").is_none());
    assert!(segments_after[0].get("route_selectors").is_none());
    let persisted = String::from_utf8_lossy(&bytes_after);
    assert!(!persisted.contains("legacy-original-key"));
    assert!(!persisted.contains("legacy-rotated-key"));

    let bodies = common::take_http_requests(&endpoint);
    assert_eq!(3, bodies.len());
    assert_eq!(bodies[1]["system"], bodies[2]["system"]);
    assert_eq!(bodies[1]["tools"], bodies[2]["tools"]);
    let previous = bodies[1]["messages"].as_array().unwrap();
    let reopened_messages = bodies[2]["messages"].as_array().unwrap();
    assert_eq!(previous.as_slice(), &reopened_messages[..previous.len()]);
    let tool_use = previous
        .iter()
        .flat_map(|message| message["content"].as_array().into_iter().flatten())
        .find(|block| block["type"] == "tool_use")
        .expect("legacy tool call should remain in the reopened prefix");
    let tool_use_id = tool_use["id"].as_str().unwrap();
    assert!(previous.iter().any(|message| {
        message["role"] == "user"
            && message["content"].as_array().is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    block["type"] == "tool_result"
                        && block["tool_use_id"] == tool_use_id
                        && block["content"]
                            .as_str()
                            .is_some_and(|text| text.contains(contents))
                })
            })
    }));
}

#[tokio::test]
async fn legacy_signed_native_thinking_continues_under_strict_validation() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let path = "/legacy-signed-native";
    let endpoint = format!("{path}/v1/messages");
    let followup = mock_reply(
        &server,
        &endpoint,
        "signed legacy follow-up",
        "native-key",
        "signed_legacy_followup",
        json!([{"type":"text","text":"signed legacy continuation"}]),
    )
    .await;
    let seed = mock_reply(
        &server,
        &endpoint,
        "signed legacy seed",
        "native-key",
        "signed_legacy_seed",
        json!([
            {"type":"thinking","thinking":"signed durable thought","signature":"legacy-signed-signature"},
            {"type":"text","text":"signed native answer"}
        ]),
    )
    .await;
    let options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "legacy_signed_native",
        "legacy-native",
        path,
        ProviderProfile::Anthropic,
        "native-key",
    );
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", agent_config("legacy-native"))
        .unwrap();
    let first = send(
        &session,
        "seed",
        "signed legacy seed",
        Some("signed native answer"),
    )
    .await;
    assert_eq!(AgentStatus::Idle, first.status, "{:?}", first.last_error);
    seed.assert_hits_async(1).await;
    let events_path = session.events_path().to_path_buf();
    session.close().await.unwrap();
    drop(session);

    let legacy_bytes = install_legacy_fixture(&events_path, SnapshotFields::Legacy, false);
    let before = EventStore::replay(&events_path).unwrap();
    let segments_before = facts(&before, "runtime.context_segment");
    assert_eq!(1, segments_before.len());
    let signature_before = segments_before[0]["signature"].clone();
    let reopened_options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "legacy_signed_native",
        "legacy-native",
        path,
        ProviderProfile::Anthropic,
        "native-key",
    );
    let reopened = Session::open(reopened_options).await.unwrap();
    let second = send(
        &reopened,
        "followup",
        "signed legacy follow-up",
        Some("signed legacy continuation"),
    )
    .await;
    assert_eq!(AgentStatus::Idle, second.status, "{:?}", second.last_error);
    followup.assert_hits_async(1).await;
    reopened.close().await.unwrap();
    drop(reopened);

    let after = EventStore::replay(&events_path).unwrap();
    let segments_after = facts(&after, "runtime.context_segment");
    assert_eq!(segments_before, segments_after);
    assert_eq!(signature_before, segments_after[0]["signature"]);
    assert!(segments_after[0].get("capabilities").is_none());
    assert!(segments_after[0].get("route_selectors").is_none());
    assert!(std::fs::read(&events_path)
        .unwrap()
        .starts_with(&legacy_bytes));
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
    assert!(second_messages.iter().any(|message| {
        message["role"] == "assistant"
            && message["content"].as_array().is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    block["type"] == "thinking" && block["signature"] == "legacy-signed-signature"
                })
            })
    }));
}

#[tokio::test]
async fn legacy_unsigned_native_replay_is_rejected_without_context_mutation() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let path = "/legacy-unsigned-native";
    let endpoint = format!("{path}/v1/messages");
    let blocked = mock_reply(
        &server,
        &endpoint,
        "unsigned legacy follow-up",
        "native-key",
        "unsigned_legacy_followup",
        json!([{"type":"text","text":"must not dispatch"}]),
    )
    .await;
    let seed = mock_reply(
        &server,
        &endpoint,
        "unsigned legacy seed",
        "compatible-key",
        "unsigned_legacy_seed",
        json!([
            {"type":"thinking","thinking":"unsigned durable thought"},
            {"type":"text","text":"unsigned compatible answer"}
        ]),
    )
    .await;
    let options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "legacy_unsigned_native",
        "legacy-route",
        path,
        ProviderProfile::Compatible,
        "compatible-key",
    );
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", agent_config("legacy-route"))
        .unwrap();
    let first = send(
        &session,
        "seed",
        "unsigned legacy seed",
        Some("unsigned compatible answer"),
    )
    .await;
    assert_eq!(AgentStatus::Idle, first.status, "{:?}", first.last_error);
    seed.assert_hits_async(1).await;
    let events_path = session.events_path().to_path_buf();
    session.close().await.unwrap();
    drop(session);

    let legacy_bytes = install_legacy_fixture(&events_path, SnapshotFields::Legacy, false);
    let before = EventStore::replay(&events_path).unwrap();
    let segments_before = facts(&before, "runtime.context_segment");
    let signature_before = segments_before[0]["signature"].clone();
    let native_options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "legacy_unsigned_native",
        "legacy-route",
        path,
        ProviderProfile::Anthropic,
        "native-key",
    );
    let reopened = Session::open(native_options).await.unwrap();
    let rejected = send_and_settle(&reopened, "followup", "unsigned legacy follow-up").await;
    assert_eq!(AgentStatus::Interrupted, rejected.status);
    assert!(rejected.last_error.as_deref().is_some_and(|error| {
        error.contains("native Messages thinking replay requires its original signed block")
    }));
    blocked.assert_hits_async(0).await;
    reopened.close().await.unwrap();
    drop(reopened);

    let bytes_after = std::fs::read(&events_path).unwrap();
    assert!(bytes_after.starts_with(&legacy_bytes));
    let after = EventStore::replay(&events_path).unwrap();
    assert_context_facts_unchanged(&before, &after);
    let segments_after = facts(&after, "runtime.context_segment");
    assert_eq!(signature_before, segments_after[0]["signature"]);
    let bodies = common::take_http_requests(&endpoint);
    assert_eq!(1, bodies.len());
}

#[tokio::test]
async fn legacy_thinking_without_a_durable_replay_decision_is_ambiguous() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let path = "/legacy-ambiguous-thinking";
    let endpoint = format!("{path}/v1/messages");
    let blocked = mock_reply(
        &server,
        &endpoint,
        "ambiguous legacy follow-up",
        "compatible-key",
        "ambiguous_legacy_followup",
        json!([{"type":"text","text":"must not dispatch"}]),
    )
    .await;
    let seed = mock_reply(
        &server,
        &endpoint,
        "ambiguous legacy seed",
        "native-key",
        "ambiguous_legacy_seed",
        json!([
            {"type":"thinking","thinking":"thinking without durable replay","signature":"ambiguous-native-signature"},
            {"type":"text","text":"ambiguous compatible answer"}
        ]),
    )
    .await;
    let options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "legacy_ambiguous_thinking",
        "legacy-route",
        path,
        ProviderProfile::Anthropic,
        "native-key",
    );
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", agent_config("legacy-route"))
        .unwrap();
    let first = send(
        &session,
        "seed",
        "ambiguous legacy seed",
        Some("ambiguous compatible answer"),
    )
    .await;
    assert_eq!(AgentStatus::Idle, first.status, "{:?}", first.last_error);
    seed.assert_hits_async(1).await;
    let events_path = session.events_path().to_path_buf();
    session.close().await.unwrap();
    drop(session);

    let legacy_bytes = install_legacy_fixture(&events_path, SnapshotFields::Legacy, true);
    let before = EventStore::replay(&events_path).unwrap();
    let segments_before = facts(&before, "runtime.context_segment");
    let admissions_before = facts(&before, "runtime.context_admitted");
    let reopened_options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "legacy_ambiguous_thinking",
        "legacy-route",
        path,
        ProviderProfile::Compatible,
        "compatible-key",
    );
    let reopened = Session::open(reopened_options).await.unwrap();
    let rejected = send_and_settle(&reopened, "followup", "ambiguous legacy follow-up").await;
    assert_eq!(AgentStatus::Interrupted, rejected.status);
    assert!(rejected.last_error.as_deref().is_some_and(|error| {
        error.contains("legacy assistant thinking without a durable provider replay decision")
    }));
    blocked.assert_hits_async(0).await;
    reopened.close().await.unwrap();
    drop(reopened);

    let bytes_after = std::fs::read(&events_path).unwrap();
    assert!(bytes_after.starts_with(&legacy_bytes));
    let after = EventStore::replay(&events_path).unwrap();
    assert_eq!(segments_before, facts(&after, "runtime.context_segment"));
    assert_eq!(admissions_before, facts(&after, "runtime.context_admitted"));
    assert_eq!(1, common::take_http_requests(&endpoint).len());
}

#[tokio::test]
async fn legacy_effective_endpoint_change_fails_before_dispatch() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let original_path = "/legacy-endpoint-original";
    let changed_path = "/legacy-endpoint-changed";
    let original_endpoint = format!("{original_path}/v1/messages");
    let changed_endpoint = format!("{changed_path}/v1/messages");
    let blocked = mock_reply(
        &server,
        &changed_endpoint,
        "legacy endpoint follow-up",
        "legacy-key",
        "legacy_endpoint_followup",
        json!([{"type":"text","text":"must not dispatch"}]),
    )
    .await;
    let seed = mock_reply(
        &server,
        &original_endpoint,
        "legacy endpoint seed",
        "legacy-key",
        "legacy_endpoint_seed",
        json!([{"type":"text","text":"legacy endpoint answer"}]),
    )
    .await;
    let options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "legacy_endpoint_change",
        "legacy-route",
        original_path,
        ProviderProfile::Compatible,
        "legacy-key",
    );
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", agent_config("legacy-route"))
        .unwrap();
    let first = send(
        &session,
        "seed",
        "legacy endpoint seed",
        Some("legacy endpoint answer"),
    )
    .await;
    assert_eq!(AgentStatus::Idle, first.status, "{:?}", first.last_error);
    seed.assert_hits_async(1).await;
    let events_path = session.events_path().to_path_buf();
    session.close().await.unwrap();
    drop(session);

    let legacy_bytes = install_legacy_fixture(&events_path, SnapshotFields::Legacy, false);
    let before = EventStore::replay(&events_path).unwrap();
    let segments_before = facts(&before, "runtime.context_segment");
    let admissions_before = facts(&before, "runtime.context_admitted");
    let changed_options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "legacy_endpoint_change",
        "legacy-route",
        changed_path,
        ProviderProfile::Compatible,
        "legacy-key",
    );
    let reopened = Session::open(changed_options).await.unwrap();
    let rejected = send_and_settle(&reopened, "followup", "legacy endpoint follow-up").await;
    assert_eq!(AgentStatus::Interrupted, rejected.status);
    assert!(rejected.last_error.as_deref().is_some_and(|error| {
        error.contains("legacy context segment")
            && error.contains("restore the recorded route")
            && error.contains("start a new Session")
    }));
    blocked.assert_hits_async(0).await;
    reopened.close().await.unwrap();
    drop(reopened);

    assert!(std::fs::read(&events_path)
        .unwrap()
        .starts_with(&legacy_bytes));
    let after = EventStore::replay(&events_path).unwrap();
    assert_eq!(segments_before, facts(&after, "runtime.context_segment"));
    assert_eq!(admissions_before, facts(&after, "runtime.context_admitted"));
    assert_eq!(1, common::take_http_requests(&original_endpoint).len());
    assert!(common::take_http_requests(&changed_endpoint).is_empty());
}

#[path = "legacy_context_snapshot_boundaries.rs"]
mod boundaries;
