use kuku::config::ProviderProfile;
use kuku::event::{EventPayload, EventStore};
use kuku::test_support::RuntimeBoundaryPause;
use kuku::{AgentStatus, Input, Session};

use super::session_context_snapshot::{
    add_connection, agent_config, facts, mock_reply, session_options, wait_for_output,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_configuration_between_boundary_and_snapshot_reopens_on_new_route() {
    let server = httpmock::MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let first_path = "/concurrent-route-a";
    let second_path = "/concurrent-route-b";
    let first_endpoint = format!("{first_path}/v1/messages");
    let second_endpoint = format!("{second_path}/v1/messages");
    let first = mock_reply(
        &server,
        &first_endpoint,
        "concurrent boundary first",
        "route-a-key",
        "concurrent_first",
        serde_json::json!([{"type":"text","text":"route A answer"}]),
    )
    .await;
    let second = mock_reply(
        &server,
        &second_endpoint,
        "concurrent route B follow-up",
        "route-b-key",
        "concurrent_second",
        serde_json::json!([{"type":"text","text":"route B answer"}]),
    )
    .await;
    let mut options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "concurrent_context_snapshot",
        "compatible",
        first_path,
        ProviderProfile::Compatible,
        "route-a-key",
    );
    add_connection(
        &mut options,
        &server,
        "route-b",
        second_path,
        ProviderProfile::Compatible,
        "route-b-key",
    );
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", agent_config("compatible"))
        .unwrap();

    let pause = RuntimeBoundaryPause::install(session.events_path());
    session
        .send(Input::new("first", "main", "concurrent boundary first"))
        .unwrap();
    pause.wait_until_reached();
    session
        .configure_agent("configure-b", "main", agent_config("route-b"))
        .unwrap();
    pause.release();

    let first_snapshot = wait_for_output(&session, "route A answer").await;
    assert_eq!(AgentStatus::Idle, first_snapshot.status);
    first.assert_hits_async(1).await;
    drop(pause);

    let events_path = session.events_path().to_path_buf();
    let first_events = EventStore::replay(&events_path).unwrap();
    let segments = facts(&first_events, "runtime.context_segment");
    assert_eq!(1, segments.len());
    let created_id = first_events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::Unknown(value)
                if value["kind"] == "runtime.operation"
                    && value["record"]["operation"] == "agent_created" =>
            {
                Some(event.id)
            }
            _ => None,
        })
        .unwrap();
    let configured_id = first_events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::Unknown(value)
                if value["kind"] == "runtime.operation"
                    && value["record"]["operation"] == "agent_configured" =>
            {
                Some(event.id)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(created_id, segments[0]["configuration"].as_u64().unwrap());
    assert!(configured_id > segments[0]["configuration"].as_u64().unwrap());
    session.close().await.unwrap();
    let bytes_before = std::fs::read(&events_path).unwrap();
    drop(session);

    let mut reopened_options = session_options(
        &server,
        workspace.path(),
        data.path(),
        "concurrent_context_snapshot",
        "compatible",
        first_path,
        ProviderProfile::Compatible,
        "route-a-key",
    );
    add_connection(
        &mut reopened_options,
        &server,
        "route-b",
        second_path,
        ProviderProfile::Compatible,
        "route-b-key",
    );
    let reopened = Session::open(reopened_options).await.unwrap();
    reopened
        .send(Input::new("second", "main", "concurrent route B follow-up"))
        .unwrap();
    let second_snapshot = wait_for_output(&reopened, "route B answer").await;
    assert_eq!(AgentStatus::Idle, second_snapshot.status);
    second.assert_hits_async(1).await;
    reopened.close().await.unwrap();

    let events_after = EventStore::replay(&events_path).unwrap();
    let segments_after = facts(&events_after, "runtime.context_segment");
    assert_eq!(2, segments_after.len());
    assert_eq!(
        "route-b",
        segments_after[1]["route_selectors"]["query_connection"]
    );
    assert_eq!("configuration", segments_after[1]["reason"]);
    assert!(
        std::fs::read(&events_path)
            .unwrap()
            .starts_with(&bytes_before)
    );
}
