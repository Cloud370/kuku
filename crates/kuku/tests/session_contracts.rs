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
    SessionOptions::new(workspace, data, config)
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
        response(
            json!({"tool_calls":[
                tool("missing", "read_file", json!({"path":"missing.txt"})),
                tool("denied", "run_command", common::shell_args("echo denied > denied.txt")),
                tool("find", "find_files", json!({"path":".","pattern":"seed.txt"})),
                tool("write", "run_command", common::shell_args("echo allowed > allowed.txt"))
            ]}),
            "tool_calls",
        ),
    )
    .await;
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

#[tokio::test]
async fn command_guard_reason_reaches_direct_calls_and_scripts_without_raw_arguments() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let marker = workspace.path().join("authorization-marker.txt");
    std::fs::write(&marker, "keep").unwrap();
    let args = json!({"program":"rm", "args":["-rf","authorization-marker.txt"]});
    let first = fixture(
        &server,
        |request| tool_count(request) == 0,
        response(
            json!({"tool_calls":[
                tool("direct", "run_command", args.clone()),
                tool("script", "run_js", json!({"code":format!("return await tools.run_command({args});")}))
            ]}),
            "tool_calls",
        ),
    )
    .await;
    let continuation = fixture(
        &server,
        |request| {
            let body = request_body(request);
            let results: Vec<_> = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|message| message["role"] == "tool")
                .collect();
            results.len() == 2
                && results.iter().all(|message| {
                    message["content"].as_str().is_some_and(|content| {
                        content.contains("recursive force delete is destructive")
                            && !content.contains("authorization-marker.txt")
                    })
                })
        },
        response(json!({"content":"guard explained"}), "stop"),
    )
    .await;
    let session = Session::open(options(&server, workspace.path(), data.path()))
        .await
        .unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("input", "main", "check the guard response"))
        .unwrap();
    assert_eq!(
        Some("guard explained"),
        idle(&session).await.last_output.as_deref()
    );
    assert_eq!("keep", std::fs::read_to_string(marker).unwrap());
    session.close().await.unwrap();
    let events = EventStore::replay(session.events_path()).unwrap();
    assert!(events.iter().any(|event| matches!(&event.payload, EventPayload::PermissionDeny { tool_call_id, source, .. } if tool_call_id == "direct" && source == "hard_guard")));
    assert!(events.iter().any(|event| matches!(&event.payload, EventPayload::Unknown(value) if value["kind"] == "workspace.action" && value["status"] == "blocked" && value["facts"]["source"] == "hard_guard")));
    first.assert_hits_async(1).await;
    continuation.assert_hits_async(1).await;
}

#[tokio::test]
async fn run_js_reports_the_real_native_program_when_launch_fails() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    const PROGRAM: &str = "kuku-nonexistent-launch-regression-fixture";
    let first = fixture(
        &server,
        |request| tool_count(request) == 0,
        response(json!({"tool_calls":[
            tool("caught", "run_js", json!({"code":format!(
                "try {{ await tools.run_command({{program:'{PROGRAM}'}}); }} catch (error) {{ return {{program:error.program, termination:error.termination}}; }}"
            )})),
            tool("uncaught", "run_js", json!({"code":format!("await tools.run_command({{program:'{PROGRAM}'}});")})),
        ]}), "tool_calls"),
    ).await;
    let continuation = fixture(
        &server,
        |request| {
            let body = request_body(request);
            let result = |id| {
                body["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|message| message["tool_call_id"] == id)
                    .and_then(|message| message["content"].as_str())
            };
            let expected = json!({"program":PROGRAM,"termination":"program_missing"});
            result("caught").and_then(|text| serde_json::from_str::<Value>(text).ok())
                == Some(expected)
                && result("uncaught").is_some_and(|text| {
                    text.lines()
                        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                        .any(|row| {
                            row["program"] == PROGRAM
                                && row["termination"] == "program_missing"
                                && row["started"] == false
                        })
                })
        },
        response(json!({"content":"launch evidence received"}), "stop"),
    )
    .await;
    let session = Session::open(options(&server, workspace.path(), data.path()))
        .await
        .unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("input", "main", "check launch recovery"))
        .unwrap();
    assert_eq!(
        Some("launch evidence received"),
        idle(&session).await.last_output.as_deref()
    );
    session.close().await.unwrap();
    first.assert_hits_async(1).await;
    continuation.assert_hits_async(1).await;
}

#[tokio::test]
async fn create_agent_uses_default_routing_and_rejects_invalid_tool_configuration() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let first = fixture(
        &server,
        |request| {
            let body = request_body(request);
            let create = body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["function"]["name"] == "create_agent")
                .expect("create_agent tool definition");
            let schema = &create["function"]["parameters"];
            let properties = &schema["properties"];
            let context = body["messages"].to_string();
            tool_count(request) == 0
                && schema["required"] == json!(["address"])
                && properties["tier"]["description"]
                    .as_str()
                    .is_some_and(|text| text.contains("<kuku_models>"))
                && properties.as_object().unwrap().len() == 8
                && properties["reasoning_effort"]["type"] == "string"
                && properties["service_tier"]["type"] == "string"
                && properties["tools"]["description"].as_str().is_some_and(|text| text.contains("empty array means no tools"))
                && context.contains("<kuku_models>")
                && context.contains("balanced")
                && context.contains("Omit tier to use the default model")
                && context.contains("service provider default, service options flex, priority")
                && context.contains("only when the user explicitly asks for that tier")
                && context.contains("collect its reply with list_agents")
        },
        response(
            json!({"tool_calls":[
                tool("default", "create_agent", json!({
                    "address":"reviewer","instructions":"Review code for defects.",
                    "tier":"","preset":"","workspace":"","tools":["read_file","send_message"]
                })),
                tool("wrong-route", "create_agent", json!({"address":"second-reviewer","connection":"balanced"})),
                tool("conflicting-tools", "create_agent", json!({
                    "address":"blocked-reviewer","no_agents":true,"tools":["read_file","send_message"]
                })),
                tool("denied", "create_agent", json!({"address":"unauthorized-reviewer","tier":""})),
                tool("invalid-address", "create_agent", json!({"address":"review/team","tier":""}))
            ]}),
            "tool_calls",
        ),
    )
    .await;
    let corrected = fixture(
        &server,
        |request| {
            let body = request_body(request);
            let messages = body["messages"].as_array().unwrap();
            let content = |call_id: &str| {
                messages
                    .iter()
                    .find(|message| message["role"] == "tool" && message["tool_call_id"] == call_id)
                    .and_then(|message| message["content"].as_str())
                    .unwrap_or_default()
            };
            tool_count(request) == 5
                && content("wrong-route").contains("unknown field `connection`")
                && content("conflicting-tools").contains("unknown field `no_agents`")
                && content("invalid-address").contains("must be flat")
                && content("denied").contains("not dispatched")
        },
        response(
            json!({"tool_calls":[tool("correct-route", "create_agent", json!({"address":"second-reviewer","tier":"balanced","reasoning_effort":"high","service_tier":"flex"}))]}),
            "tool_calls",
        ),
    )
    .await;
    let last = fixture(
        &server,
        |request| tool_count(request) == 6,
        response(json!({"content":"Reviewers configured."}), "stop"),
    )
    .await;
    let mut options = options(&server, workspace.path(), data.path());
    options.authorizer = Arc::new(BatchPolicy);
    let connection = options.config.providers.get_mut("openai").unwrap();
    connection.options.reasoning_efforts = Some(vec!["high".into()]);
    connection.options.service_tiers = Some(vec!["flex".into(), "priority".into()]);
    let parent_provider = connection.clone();
    options
        .config
        .providers
        .insert("parent-route".into(), parent_provider);
    let session = Session::open(options).await.unwrap();
    session
        .create_agent(
            "create",
            "main",
            AgentConfig {
                connection: Some("parent-route".into()),
                model: Some("parent-override".into()),
                ..AgentConfig::default()
            },
        )
        .unwrap();
    let mistake = session
        .create_agent(
            "mistaken-preset",
            "mistaken-reviewer",
            AgentConfig {
                preset: Some("flex".into()),
                tier: Some("balanced".into()),
                instructions: Some("Reply with OK.".into()),
                tools: Some(Vec::new()),
                ..AgentConfig::default()
            },
        )
        .unwrap_err();
    assert!(mistake.to_string().contains("preset 'flex' was not found"));
    assert!(session.agent("mistaken-reviewer").is_err());
    session
        .send(Input::new("input", "main", "Configure two reviewers"))
        .unwrap();
    let state = idle(&session).await;
    assert_eq!(
        Some("Reviewers configured."),
        state.last_output.as_deref(),
        "{:?}",
        state.last_error
    );
    let parent = session.agent_selection("main").unwrap();
    assert_eq!("parent-override", parent.model);
    assert_eq!("parent-route", parent.connection);
    for address in ["reviewer", "second-reviewer"] {
        let selected = session.agent_selection(address).unwrap();
        assert_eq!("session-contract", selected.model);
        assert_eq!("openai", selected.connection);
    }
    let reviewer = session.agent("reviewer").unwrap();
    assert_eq!(
        None,
        session
            .agent_selection("reviewer")
            .unwrap()
            .settings
            .service_tier
    );
    let selected = session.agent_selection("second-reviewer").unwrap();
    assert_eq!(Some("high"), selected.settings.reasoning_effort.as_deref());
    assert_eq!(Some("flex"), selected.settings.service_tier.as_deref());
    assert_eq!(None, reviewer.config.max_output_tokens);
    assert!(!reviewer.config.no_agents);
    assert_eq!(
        Some(vec!["read_file".into(), "send_message".into()]),
        reviewer.config.tools
    );
    assert!(session.agent("blocked-reviewer").is_err());
    assert!(session.agent("unauthorized-reviewer").is_err());
    assert!(session.agent("review/team").is_err());
    session.close().await.unwrap();
    first.assert_hits_async(1).await;
    corrected.assert_hits_async(1).await;
    last.assert_hits_async(1).await;
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
            Observation::Live { .. } | Observation::ReplyPreview { .. } => retained += 1,
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
    let first = fixture(&server, |request| tool_count(request) == 0, response(json!({"tool_calls":[tool("first", "run_command", common::shell_args("echo first > first.txt"))]}), "tool_calls")).await;
    let (stream, text) = streaming_response();
    let burst = fixture(
        &server,
        |request| tool_count(request) == 1 && !second_round(request),
        stream,
    )
    .await;
    let second = fixture(&server, |request| tool_count(request) == 1 && second_round(request), response(json!({"tool_calls":[tool("second", "run_command", common::shell_args("echo second > second.txt"))]}), "tool_calls")).await;
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
