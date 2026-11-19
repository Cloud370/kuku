use httpmock::prelude::MockServer;
use kuku::config::ProviderProfile;
use kuku::event::EventStore;
use kuku::{AgentStatus, Session};
use serde_json::json;

use super::super::common;
use super::super::session_context_snapshot::{
    agent_config, facts, mock_reply, send, session_options,
};
use super::{context_segment, install_legacy_fixture, send_and_settle, SnapshotFields};

#[tokio::test]
async fn partial_legacy_snapshot_metadata_fails_closed() {
    for (session_id, path, fields) in [
        (
            "legacy_partial_capabilities",
            "/legacy-partial-capabilities",
            SnapshotFields::MissingCapabilities,
        ),
        (
            "legacy_partial_selectors",
            "/legacy-partial-selectors",
            SnapshotFields::MissingRouteSelectors,
        ),
    ] {
        let server = MockServer::start_async().await;
        let workspace = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let endpoint = format!("{path}/v1/messages");
        let blocked = mock_reply(
            &server,
            &endpoint,
            "partial metadata follow-up",
            "legacy-key",
            "partial_metadata_followup",
            json!([{"type":"text","text":"must not dispatch"}]),
        )
        .await;
        let seed = mock_reply(
            &server,
            &endpoint,
            "partial metadata seed",
            "legacy-key",
            "partial_metadata_seed",
            json!([{"type":"text","text":"partial metadata answer"}]),
        )
        .await;
        let options = session_options(
            &server,
            workspace.path(),
            data.path(),
            session_id,
            "legacy-route",
            path,
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
            "partial metadata seed",
            Some("partial metadata answer"),
        )
        .await;
        assert_eq!(AgentStatus::Idle, first.status, "{:?}", first.last_error);
        seed.assert_hits_async(1).await;
        let events_path = session.events_path().to_path_buf();
        session.close().await.unwrap();
        drop(session);

        let legacy_bytes = install_legacy_fixture(&events_path, fields, false);
        let before = EventStore::replay(&events_path).unwrap();
        let segments_before = facts(&before, "runtime.context_segment");
        let admissions_before = facts(&before, "runtime.context_admitted");
        let reopened_options = session_options(
            &server,
            workspace.path(),
            data.path(),
            session_id,
            "legacy-route",
            path,
            ProviderProfile::Compatible,
            "legacy-key",
        );
        let reopened = Session::open(reopened_options).await.unwrap();
        let rejected = send_and_settle(&reopened, "followup", "partial metadata follow-up").await;
        assert_eq!(AgentStatus::Interrupted, rejected.status);
        assert!(rejected.last_error.as_deref().is_some_and(|error| {
            error.contains("inconsistent partial provider route snapshot")
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
        assert_eq!(1, common::take_http_requests(&endpoint).len());
    }
}

#[tokio::test]
async fn legitimate_context_change_creates_a_snapshotted_legacy_successor() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let path = "/legacy-context-successor";
    let endpoint = format!("{path}/v1/messages");
    let followup = mock_reply(
        &server,
        &endpoint,
        "legacy successor follow-up",
        "legacy-key",
        "legacy_successor_followup",
        json!([{"type":"text","text":"snapshotted successor answer"}]),
    )
    .await;
    let third_turn = mock_reply(
        &server,
        &endpoint,
        "legacy successor third turn",
        "legacy-key",
        "legacy_successor_third_turn",
        json!([{"type":"text","text":"third successor answer"}]),
    )
    .await;
    let seed = mock_reply(
        &server,
        &endpoint,
        "legacy successor seed",
        "legacy-key",
        "legacy_successor_seed",
        json!([{"type":"text","text":"legacy successor seed answer"}]),
    )
    .await;
    let options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "legacy_context_successor",
        "legacy-route",
        path,
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
        "legacy successor seed",
        Some("legacy successor seed answer"),
    )
    .await;
    assert_eq!(AgentStatus::Idle, first.status, "{:?}", first.last_error);
    seed.assert_hits_async(1).await;
    seed.delete_async().await;
    let events_path = session.events_path().to_path_buf();
    session.close().await.unwrap();
    drop(session);

    let legacy_bytes = install_legacy_fixture(&events_path, SnapshotFields::Legacy, false);
    let before = EventStore::replay(&events_path).unwrap();
    let segments_before = facts(&before, "runtime.context_segment");
    let admissions_before = facts(&before, "runtime.context_admitted");
    let predecessor_id = context_segment(&before).0.id;
    let reopened_options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "legacy_context_successor",
        "legacy-route",
        path,
        ProviderProfile::Compatible,
        "legacy-key",
    );
    let reopened = Session::open(reopened_options).await.unwrap();
    let mut config = agent_config("legacy-route");
    config.instructions = Some("new instructions create a deliberate context boundary".into());
    reopened
        .configure_agent("configure", "main", config)
        .unwrap();
    let second = send(
        &reopened,
        "followup",
        "legacy successor follow-up",
        Some("snapshotted successor answer"),
    )
    .await;
    assert_eq!(AgentStatus::Idle, second.status, "{:?}", second.last_error);
    followup.assert_hits_async(1).await;
    followup.delete_async().await;
    reopened.close().await.unwrap();
    drop(reopened);

    let after_successor = EventStore::replay(&events_path).unwrap();
    let segments_after_successor = facts(&after_successor, "runtime.context_segment");
    let admissions_after_successor = facts(&after_successor, "runtime.context_admitted");
    assert_eq!(2, segments_after_successor.len());
    let successor = segments_after_successor[1].clone();

    let third_options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "legacy_context_successor",
        "legacy-route",
        path,
        ProviderProfile::Compatible,
        "legacy-key",
    );
    let third_reopened = Session::open(third_options).await.unwrap();
    let third = send(
        &third_reopened,
        "third",
        "legacy successor third turn",
        Some("third successor answer"),
    )
    .await;
    assert_eq!(AgentStatus::Idle, third.status, "{:?}", third.last_error);
    third_turn.assert_hits_async(1).await;
    third_reopened.close().await.unwrap();
    drop(third_reopened);

    let bytes_after = std::fs::read(&events_path).unwrap();
    assert!(bytes_after.starts_with(&legacy_bytes));
    let after = EventStore::replay(&events_path).unwrap();
    let segments_after = facts(&after, "runtime.context_segment");
    let admissions_after = facts(&after, "runtime.context_admitted");
    assert_eq!(segments_before[0], segments_after[0]);
    assert_eq!(2, segments_after.len());
    assert_ne!(
        segments_after[0]["signature"],
        segments_after[1]["signature"]
    );
    assert_eq!(successor["signature"], segments_after[1]["signature"]);
    assert_eq!(
        successor["system_prompt"],
        segments_after[1]["system_prompt"]
    );
    assert_eq!(successor["tools"], segments_after[1]["tools"]);
    assert_eq!(
        Some(predecessor_id),
        segments_after[1]["predecessor"].as_u64()
    );
    assert_eq!("configuration", segments_after[1]["reason"]);
    assert!(segments_after[0].get("capabilities").is_none());
    assert!(segments_after[0].get("route_selectors").is_none());
    assert!(segments_after[1].get("capabilities").is_some());
    assert!(segments_after[1].get("route_selectors").is_some());
    assert!(admissions_after.starts_with(&admissions_before));
    assert!(admissions_after.starts_with(&admissions_after_successor));

    let bodies = common::take_http_requests(&endpoint);
    assert_eq!(3, bodies.len());
    assert_eq!(bodies[1]["system"], bodies[2]["system"]);
    assert_eq!(bodies[1]["tools"], bodies[2]["tools"]);
    let successor_messages = bodies[1]["messages"].as_array().unwrap();
    let third_messages = bodies[2]["messages"].as_array().unwrap();
    let mut expected_prefix = successor_messages.clone();
    expected_prefix.push(json!({
        "role": "assistant",
        "content": [{"type": "text", "text": "snapshotted successor answer"}]
    }));
    assert_eq!(
        expected_prefix.as_slice(),
        &third_messages[..expected_prefix.len()]
    );
}
