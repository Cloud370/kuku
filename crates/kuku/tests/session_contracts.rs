mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use httpmock::prelude::{HttpMockRequest, MockServer, POST};
use httpmock::Mock;
use serde_json::{json, Value};

use kuku::event::{EventPayload, EventStore, StoredEvent};
use kuku::permission::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer};
use kuku::{
    AgentConfig, AgentSnapshot, AgentStatus, Input, InputSource, Observation, Session,
    SessionObserver, SessionOptions,
};

const OBSERVATION_CAPACITY: usize = 2;
const STREAM_CHUNKS: usize = 128;

fn options(server: &MockServer, workspace: &Path, data: &Path) -> SessionOptions {
    let mut config = common::test_config();
    config.providers.get_mut("openai").unwrap().base_url = server.url("");
    let tier = config.tiers.get_mut("balanced").unwrap();
    tier.provider = "openai".into();
    tier.model = "session-contract".into();
    tier.think = kuku::config::ThinkLevel::Off;
    config.handoff.enabled = false;
    config.plugin.enabled = false;
    let mut options = SessionOptions::new(workspace, data, config);
    options.limits.max_steps_per_turn = 4;
    options
}

fn request_body(request: &HttpMockRequest) -> Value {
    serde_json::from_slice(request.body.as_deref().unwrap_or_default()).unwrap()
}

fn tool_count(request: &HttpMockRequest) -> usize {
    request_body(request)["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .count()
}

fn second_round(request: &HttpMockRequest) -> bool {
    request_body(request)["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .is_some_and(|message| {
            message["content"]
                .to_string()
                .contains("observer-round-two")
        })
}

fn tool(id: &str, name: &str, args: Value) -> Value {
    json!({"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}})
}

fn response(message: Value, reason: &str) -> String {
    common::openai_sse_response(json!({
        "id":"session-contract","model":"session-contract",
        "choices":[{"message":message,"finish_reason":reason}],
        "usage":{"prompt_tokens":100,"completion_tokens":20}
    }))
}

async fn fixture<'a>(
    server: &'a MockServer,
    matcher: fn(&HttpMockRequest) -> bool,
    body: String,
) -> Mock<'a> {
    server
        .mock_async(move |when, then| {
            when.method(POST).path("/chat/completions").matches(matcher);
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(body);
        })
        .await
}

async fn idle(session: &Session) -> AgentSnapshot {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = session.agent("main").unwrap();
            if snapshot.status != AgentStatus::Running {
                assert_eq!(AgentStatus::Idle, snapshot.status, "{snapshot:?}");
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("public Session execution did not settle without observer advancement")
}

#[derive(Debug)]
struct BatchPolicy;

impl ToolAuthorizer for BatchPolicy {
    fn authorize(&self, action: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        assert_eq!("main", action.agent);
        assert_eq!(InputSource::Host("local".into()), action.source);
        Ok(if action.tool_call_id == "denied" {
            AuthorizationDecision::Deny
        } else {
            AuthorizationDecision::Allow
        })
    }
}

fn complete_batch_results(request: &HttpMockRequest) -> bool {
    let body = request_body(request);
    let results = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .collect::<Vec<_>>();
    let ids = results
        .iter()
        .filter_map(|message| message["tool_call_id"].as_str())
        .collect::<BTreeSet<_>>();
    let content = |id: &str| {
        results
            .iter()
            .find(|message| message["tool_call_id"] == id)
            .map(|message| message["content"].to_string())
            .unwrap_or_default()
    };
    results.len() == 4
        && ids == BTreeSet::from(["denied", "find", "missing", "write"])
        && content("missing").contains("missing.txt")
        && content("denied").contains("not dispatched")
        && content("find").contains("seed.txt")
}

fn assert_batch_facts(events: &[StoredEvent]) {
    let mut calls = BTreeMap::new();
    let mut results = BTreeMap::new();
    let mut requests = BTreeSet::new();
    for event in events {
        if let EventPayload::ToolCall {
            tool_call_id,
            tool,
            request_id,
            turn,
            ..
        } = &event.payload
        {
            assert!(calls
                .insert(tool_call_id.as_str(), (tool.as_str(), event.id, turn))
                .is_none());
            requests.insert(request_id);
        }
        if let EventPayload::ToolResult {
            tool_call_id,
            status,
            turn,
            ..
        } = &event.payload
        {
            assert!(results
                .insert(tool_call_id.as_str(), (status.as_str(), event.id, turn))
                .is_none());
        }
    }
    let expected = BTreeMap::from([
        ("missing", ("read_file", "error")),
        ("denied", ("run_command", "blocked")),
        ("find", ("find_files", "ok")),
        ("write", ("run_command", "ok")),
    ]);
    assert_eq!(
        1,
        requests.len(),
        "tools did not originate in one model response"
    );
    assert_eq!(
        expected.keys().collect::<Vec<_>>(),
        calls.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        expected.keys().collect::<Vec<_>>(),
        results.keys().collect::<Vec<_>>()
    );
    for (id, (tool, status)) in expected {
        assert_eq!(tool, calls[id].0, "wrong tool recorded for {id}");
        assert_eq!(status, results[id].0, "wrong settlement recorded for {id}");
        assert_eq!(
            calls[id].2, results[id].2,
            "result changed the turn for {id}"
        );
        assert!(
            calls[id].1 < results[id].1,
            "result preceded request for {id}"
        );
    }
    assert!(events.iter().any(|event| matches!(&event.payload, EventPayload::PermissionDeny { tool_call_id, source, .. } if tool_call_id == "denied" && source == "host")));
    assert!(!events.iter().any(|event| matches!(&event.payload, EventPayload::Unknown(value) if value["kind"] == "runtime.tool_dispatched" && value["tool_call_id"] == "denied")));
}

#[tokio::test]
async fn one_tool_batch_retains_success_denial_and_real_failure() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("seed.txt"), "independent input").unwrap();
    let first = fixture(
        &server,
        |request| tool_count(request) == 0,
        response(json!({"tool_calls":[
            tool("missing", "read_file", json!({"path":"missing.txt"})),
            tool("denied", "run_command", json!({"command":"echo denied > denied.txt","timeout":5,"brief":"must not dispatch"})),
            tool("find", "find_files", json!({"path":".","pattern":"seed.txt"})),
            tool("write", "run_command", json!({"command":"echo allowed > allowed.txt","timeout":5,"brief":"independent allowed work"}))
        ]}), "tool_calls"),
    ).await;
    let continuation = fixture(
        &server,
        complete_batch_results,
        response(json!({"content":"batch settled"}), "stop"),
    )
    .await;
    let mut options = options(&server, workspace.path(), data.path());
    options.authorizer = Arc::new(BatchPolicy);
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("input", "main", "execute the mixed tool batch"))
        .unwrap();
    assert_eq!(
        Some("batch settled"),
        idle(&session).await.last_output.as_deref()
    );
    assert_eq!(
        "allowed",
        std::fs::read_to_string(workspace.path().join("allowed.txt"))
            .unwrap()
            .trim()
    );
    assert!(!workspace.path().join("denied.txt").exists());
    assert!(!workspace.path().join("missing.txt").exists());
    session.close().await.unwrap();
    assert_batch_facts(&EventStore::replay(session.events_path()).unwrap());
    first.assert_hits_async(1).await;
    continuation.assert_hits_async(1).await;
}

fn streaming_response() -> (String, String) {
    let mut stream = String::new();
    let mut text = String::new();
    for index in 0..STREAM_CHUNKS {
        let chunk = format!("piece-{index:03}|");
        text.push_str(&chunk);
        stream.push_str(&format!(
            "data: {}\n\n",
            json!({
                "id":"session-contract","object":"chat.completion.chunk","model":"session-contract",
                "choices":[{"index":0,"delta":{"content":chunk},"finish_reason":null}]
            })
        ));
    }
    stream.push_str(&response(json!({"content":""}), "stop"));
    (stream, text)
}

fn take_committed(observer: &mut SessionObserver, count: usize) -> Vec<u64> {
    (0..count)
        .map(|_| {
            let observation = observer
                .try_next()
                .unwrap()
                .expect("committed event was not available");
            let Observation::Committed(event) = observation else {
                panic!("durable replay yielded transient observation: {observation:?}");
            };
            assert_eq!(event.id, observer.cursor());
            event.id
        })
        .collect()
}

fn assert_slow_observer_catches_up(observer: &mut SessionObserver, journal: &[StoredEvent]) {
    let after = observer.cursor();
    let mut committed = Vec::new();
    let mut skipped = 0;
    let mut retained = 0;
    while let Some(observation) = observer.try_next().unwrap() {
        match observation {
            Observation::Committed(event) => committed.push(event.id),
            Observation::Live { .. } => retained += 1,
            Observation::Lagged { skipped: count } => skipped += count,
        }
    }
    let expected = journal
        .iter()
        .filter(|event| event.id > after)
        .map(|event| event.id)
        .collect::<Vec<_>>();
    assert_eq!(
        expected, committed,
        "lagging lost committed execution facts"
    );
    assert!(skipped > 0, "streaming burst did not exercise observer lag");
    assert_eq!(
        OBSERVATION_CAPACITY, retained,
        "transient retention exceeded the configured queue"
    );
    assert!(
        skipped + retained as u64 >= STREAM_CHUNKS as u64,
        "model stream was not fully advanced without the observer"
    );
    assert_eq!(journal.last().unwrap().id, observer.cursor());
}

async fn drain_closed(observer: &mut SessionObserver) -> Vec<u64> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut ids = Vec::new();
        while let Some(observation) = observer.next().await.unwrap() {
            let Observation::Committed(event) = observation else {
                panic!("new closed-session observer had transient history: {observation:?}");
            };
            ids.push(event.id);
        }
        ids
    })
    .await
    .expect("closed observer did not finish durable replay")
}

#[tokio::test]
async fn lagged_observer_is_bounded_and_reconnects_after_unobserved_execution() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let first = fixture(&server, |request| tool_count(request) == 0, response(json!({"tool_calls":[tool("first", "run_command", json!({"command":"echo first > first.txt","timeout":5,"brief":"first unobserved write"}))]}), "tool_calls")).await;
    let (stream, text) = streaming_response();
    let burst = fixture(
        &server,
        |request| tool_count(request) == 1 && !second_round(request),
        stream,
    )
    .await;
    let second = fixture(&server, |request| tool_count(request) == 1 && second_round(request), response(json!({"tool_calls":[tool("second", "run_command", json!({"command":"echo second > second.txt","timeout":5,"brief":"write with all observers dropped"}))]}), "tool_calls")).await;
    let last = fixture(
        &server,
        |request| tool_count(request) == 2,
        response(json!({"content":"second settled"}), "stop"),
    )
    .await;
    let mut options = options(&server, workspace.path(), data.path());
    options.limits.observation_capacity = OBSERVATION_CAPACITY;
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    let mut dropped = session.observe(0);
    let checkpoint = take_committed(&mut dropped, 1)[0];
    drop(dropped);
    let mut slow = session.observe(checkpoint);
    session
        .send(Input::new("first", "main", "observer-round-one"))
        .unwrap();
    assert_eq!(
        Some(text.as_str()),
        idle(&session).await.last_output.as_deref()
    );
    assert_eq!(
        "first",
        std::fs::read_to_string(workspace.path().join("first.txt"))
            .unwrap()
            .trim()
    );
    let first_journal = EventStore::replay(session.events_path()).unwrap();
    assert_slow_observer_catches_up(&mut slow, &first_journal);
    drop(slow);
    let mut partial = session.observe(checkpoint);
    let mut resumed_ids = take_committed(&mut partial, 3);
    let partial_cursor = partial.cursor();
    assert!(partial_cursor < first_journal.last().unwrap().id);
    drop(partial);
    session
        .send(Input::new("second", "main", "observer-round-two"))
        .unwrap();
    assert_eq!(
        Some("second settled"),
        idle(&session).await.last_output.as_deref()
    );
    assert_eq!(
        "second",
        std::fs::read_to_string(workspace.path().join("second.txt"))
            .unwrap()
            .trim()
    );
    session.close().await.unwrap();
    let final_journal = EventStore::replay(session.events_path()).unwrap();
    let mut resumed = session.observe(partial_cursor);
    resumed_ids.extend(drain_closed(&mut resumed).await);
    let expected = final_journal
        .iter()
        .filter(|event| event.id > checkpoint)
        .map(|event| event.id)
        .collect::<Vec<_>>();
    assert_eq!(
        expected, resumed_ids,
        "cursor reconnect skipped or duplicated durable facts"
    );
    assert_eq!(final_journal.last().unwrap().id, resumed.cursor());
    first.assert_hits_async(1).await;
    burst.assert_hits_async(1).await;
    second.assert_hits_async(1).await;
    last.assert_hits_async(1).await;
}
