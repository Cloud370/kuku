mod common;

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use httpmock::prelude::{HttpMockRequest, MockServer, POST};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use kuku::context::{CanonicalMessage, MessageBlock, Role};
use kuku::event::{EventPayload, StoredEvent};
use kuku::{AgentConfig, AgentStatus, Input, Session, SessionOptions};

fn options(server: &MockServer, workspace: &Path, data: &Path) -> (SessionOptions, String) {
    static CASE: AtomicU64 = AtomicU64::new(0);
    let endpoint = format!("/context-{}", CASE.fetch_add(1, Ordering::Relaxed));
    let mut config = common::test_config();
    config.providers.get_mut("openai").unwrap().base_url = server.url(&endpoint);
    let tier = config.tiers.get_mut("balanced").unwrap();
    tier.provider = "openai".into();
    tier.model = "context-fixture".into();
    tier.think = kuku::config::ThinkLevel::Off;
    tier.max_output_tokens = 128;
    tier.context_window = 16_384;
    config.handoff.enabled = false;
    config.plugin.enabled = false;
    config.discovery.auto_discover = false;
    (
        SessionOptions::new(workspace, data, config),
        format!("{endpoint}/chat/completions"),
    )
}

fn agent() -> AgentConfig {
    AgentConfig {
        no_skills: true,
        no_agents: true,
        ..Default::default()
    }
}

fn response(message: Value, reason: &str) -> String {
    common::openai_sse_response(
        json!({"id":"context-fixture","model":"context-fixture","choices":[{"message":message,"finish_reason":reason}],"usage":{"prompt_tokens":100,"completion_tokens":10}}),
    )
}

async fn idle(session: &Session, address: &str) {
    let state = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let state = session.agent(address).unwrap();
            if state.status != AgentStatus::Running {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("agent did not settle");
    assert_eq!(AgentStatus::Idle, state.status, "{:?}", state.last_error);
}

fn facts(events: &[StoredEvent], kind: &str, agent: &str) -> Vec<Value> {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::Unknown(value) if value["kind"] == kind && value["agent"] == agent => {
                Some(value.clone())
            }
            _ => None,
        })
        .collect()
}

fn text_message(message: &CanonicalMessage) -> Value {
    let text = message
        .blocks
        .iter()
        .map(|block| match block {
            MessageBlock::Text(text) => text.as_str(),
            _ => panic!("text fixture admitted another block kind"),
        })
        .collect::<Vec<_>>()
        .join(if message.role == Role::User {
            "\n\n"
        } else {
            ""
        });
    json!({"role":if message.role == Role::User {"user"} else {"assistant"},"content":text})
}

#[tokio::test]
async fn reopen_replays_frozen_assets_and_exact_admitted_inputs_after_source_changes() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let prompts = tempfile::tempdir().unwrap();
    std::fs::create_dir(prompts.path().join("blocks")).unwrap();
    std::fs::write(prompts.path().join("system.md"), "ORIGINAL_SYSTEM").unwrap();
    std::fs::write(
        prompts.path().join("blocks/project-policy.md"),
        &kuku::builtin_prompt_catalog().blocks["project-policy"].text,
    )
    .unwrap();
    std::fs::write(workspace.path().join("AGENTS.md"), "ORIGINAL_POLICY").unwrap();
    std::fs::write(
        kuku::session::global_memory_path(data.path()),
        "ORIGINAL_MEMORY",
    )
    .unwrap();
    let (mut original, path) = options(&server, workspace.path(), data.path());
    original.prompts_dir = Some(prompts.path().into());
    let model = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .matches(common::capture_http_request);
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(response(json!({"content":"answer"}), "stop"));
        })
        .await;
    let session = Session::open(original.clone()).await.unwrap();
    session.create_agent("create", "main", agent()).unwrap();
    session
        .send(Input::new("first", "main", "FIRST_INPUT"))
        .unwrap();
    idle(&session, "main").await;
    original.session_id = Some(session.id().into());
    session.close().await.unwrap();
    std::fs::remove_file(prompts.path().join("system.md")).unwrap();
    std::fs::write(
        prompts.path().join("blocks/project-policy.md"),
        [0xff, 0xfe],
    )
    .unwrap();
    std::fs::write(workspace.path().join("AGENTS.md"), "CHANGED_POLICY").unwrap();
    std::fs::write(
        kuku::session::global_memory_path(data.path()),
        "CHANGED_MEMORY",
    )
    .unwrap();
    let reopened = Session::open(original.clone()).await.unwrap();
    reopened
        .send(Input::new("second", "main", "SECOND_INPUT"))
        .unwrap();
    idle(&reopened, "main").await;
    reopened.close().await.unwrap();
    std::fs::remove_dir_all(prompts.path()).unwrap();
    let reopened = Session::open(original).await.unwrap();
    reopened
        .send(Input::new("third", "main", "THIRD_INPUT"))
        .unwrap();
    idle(&reopened, "main").await;
    let events = reopened.events(0, 256).unwrap();
    reopened.close().await.unwrap();
    model.assert_hits_async(3).await;
    let bodies = common::take_http_requests(&path);
    assert_eq!(3, bodies.len());
    for (index, body) in bodies.iter().enumerate() {
        let encoded = body.to_string();
        for expected in ["ORIGINAL_SYSTEM", "ORIGINAL_POLICY", "ORIGINAL_MEMORY"] {
            assert!(encoded.contains(expected));
        }
        assert!(!encoded.contains("CHANGED_POLICY"));
        assert!(!encoded.contains("CHANGED_MEMORY"));
        if index > 0 {
            assert!(body["messages"]
                .as_array()
                .unwrap()
                .starts_with(bodies[index - 1]["messages"].as_array().unwrap()));
        }
    }
    let segments = facts(&events, "runtime.context_segment", "main");
    let prepared = facts(&events, "runtime.step_prepared", "main");
    let admitted = facts(&events, "runtime.context_admitted", "main");
    assert_eq!(1, segments.len());
    assert_eq!(3, prepared.len());
    for (step, body) in prepared.iter().zip(&bodies) {
        assert!(step.get("body").is_none());
        assert!(step["context_prepare_us"].is_u64());
        assert!(step["request_prepare_us"].is_u64());
        assert_eq!(
            format!("{:x}", Sha256::digest(serde_json::to_vec(body).unwrap())),
            step["body_hash"]
        );
        let mut messages = vec![json!({"role":"system","content":segments[0]["system_prompt"]})];
        let prelude: Vec<CanonicalMessage> =
            serde_json::from_value(segments[0]["prelude"].clone()).unwrap();
        messages.extend(prelude.iter().map(text_message));
        messages.extend(
            admitted
                .iter()
                .take(step["context_head"].as_u64().unwrap() as usize)
                .map(|value| {
                    text_message(&serde_json::from_value(value["message"].clone()).unwrap())
                }),
        );
        assert_eq!(json!(messages), body["messages"]);
    }
}

fn tool_count(request: &HttpMockRequest) -> usize {
    let body: Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .count()
}

#[tokio::test]
async fn tool_result_first_view_is_frozen_and_continued_steps_read_only_new_facts() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("value.txt"), "ORIGINAL_FILE\n").unwrap();
    let (original, path) = options(&server, workspace.path(), data.path());
    let first = server.mock_async(|when, then| {
        when.method(POST).path(&path).matches(common::capture_http_request).matches(|request| tool_count(request) == 0);
        then.status(200).body(response(json!({"tool_calls":[{"id":"read","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"value.txt\"}"}}]}), "tool_calls"));
    }).await;
    let second = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .matches(common::capture_http_request)
                .matches(|request| tool_count(request) == 1);
            then.status(200)
                .body(response(json!({"content":"read complete"}), "stop"));
        })
        .await;
    let session = Session::open(original).await.unwrap();
    session.create_agent("create", "main", agent()).unwrap();
    session
        .send(Input::new("first", "main", "read value.txt"))
        .unwrap();
    idle(&session, "main").await;
    std::fs::write(workspace.path().join("value.txt"), "CHANGED_FILE\n").unwrap();
    session
        .send(Input::new("followup", "main", "keep the previous evidence"))
        .unwrap();
    idle(&session, "main").await;
    let events = session.events(0, 256).unwrap();
    session.close().await.unwrap();
    first.assert_hits_async(1).await;
    second.assert_hits_async(2).await;
    let bodies = common::take_http_requests(&path);
    assert_eq!(3, bodies.len());
    assert!(bodies[1]["messages"]
        .as_array()
        .unwrap()
        .starts_with(bodies[0]["messages"].as_array().unwrap()));
    assert!(bodies[2]["messages"]
        .as_array()
        .unwrap()
        .starts_with(bodies[1]["messages"].as_array().unwrap()));
    assert!(bodies[2].to_string().contains("ORIGINAL_FILE"));
    assert!(!bodies[2].to_string().contains("CHANGED_FILE"));
    let source_view = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::ToolResult {
                tool_call_id,
                model_content,
                structured,
                ..
            } if tool_call_id == "read" => {
                assert!(structured.is_some());
                Some(model_content.clone())
            }
            _ => None,
        })
        .unwrap();
    let views = facts(&events, "runtime.context_admitted", "main");
    let results = views
        .iter()
        .flat_map(|value| {
            serde_json::from_value::<CanonicalMessage>(value["message"].clone())
                .unwrap()
                .blocks
        })
        .filter_map(|block| match block {
            MessageBlock::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(1, results.len());
    assert_eq!(source_view, results[0].model_content);
    assert!(results[0].structured.is_none());
    for body in &bodies[1..] {
        let wire_result = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["role"] == "tool")
            .unwrap();
        assert_eq!(
            json!({"role":"tool","tool_call_id":"read","content":source_view}),
            *wire_result
        );
    }
    let steps = facts(&events, "runtime.step_prepared", "main");
    assert!(steps[1]["context_events_read"].as_u64().unwrap() < events.len() as u64);
}

#[tokio::test]
async fn a_handoff_selects_its_agents_turns_and_does_not_change_another_prefix() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (mut original, path) = options(&server, workspace.path(), data.path());
    original.config.handoff.enabled = true;
    original.config.handoff.threshold = 0.001;
    original.config.handoff.keep_turns = 1;
    let model = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .matches(common::capture_http_request)
                .matches(|request| {
                    !request
                        .body
                        .as_ref()
                        .unwrap()
                        .windows(b"MAIN_HANDOFF".len())
                        .any(|window| window == b"MAIN_HANDOFF")
                });
            then.status(200)
                .body(response(json!({"content":"answer"}), "stop"));
        })
        .await;
    let summary = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .matches(common::capture_http_request)
                .body_contains("MAIN_HANDOFF");
            then.status(200).body(response(
                json!({"content":"<kuku_handoff>MAIN_SUMMARY</kuku_handoff>"}),
                "stop",
            ));
        })
        .await;
    let session = Session::open(original).await.unwrap();
    session
        .create_agent("create-main", "main", agent())
        .unwrap();
    session
        .create_agent("create-reviewer", "reviewer", agent())
        .unwrap();
    for (command, target, text) in [
        ("main-first", "main", "MAIN_OLD"),
        ("review-first", "reviewer", "REVIEW_OLD"),
        ("main-handoff", "main", "MAIN_HANDOFF"),
        ("review-next", "reviewer", "REVIEW_NEXT"),
        ("main-next", "main", "MAIN_NEXT"),
    ] {
        session.send(Input::new(command, target, text)).unwrap();
        idle(&session, target).await;
    }
    let events = session.events(0, 256).unwrap();
    session.close().await.unwrap();
    model.assert_hits_async(3).await;
    summary.assert_hits_async(2).await;
    let bodies = common::take_http_requests(&path);
    assert_eq!(5, bodies.len());
    assert!(bodies[3]["messages"]
        .as_array()
        .unwrap()
        .starts_with(bodies[1]["messages"].as_array().unwrap()));
    assert!(!bodies[3].to_string().contains("MAIN_SUMMARY"));
    assert!(!bodies[4].to_string().contains("MAIN_OLD"));
    assert!(bodies[4].to_string().contains("MAIN_SUMMARY"));
    assert!(bodies[4].to_string().contains("MAIN_HANDOFF"));
    assert_eq!(
        1,
        facts(&events, "runtime.context_segment", "reviewer").len()
    );
    assert_eq!(2, facts(&events, "runtime.context_segment", "main").len());
}

#[tokio::test]
async fn non_main_text_and_tool_response_is_one_assistant_message() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("value.txt"), "review evidence\n").unwrap();
    let (original, path) = options(&server, workspace.path(), data.path());
    let first = server.mock_async(|when, then| {
        when.method(POST).path(&path).matches(common::capture_http_request).matches(|request| tool_count(request) == 0);
        then.status(200).body(response(json!({"content":"READING_REVIEW_FILE","tool_calls":[{"id":"review-read","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"value.txt\"}"}}]}), "tool_calls"));
    }).await;
    let second = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .matches(common::capture_http_request)
                .matches(|request| tool_count(request) == 1);
            then.status(200)
                .body(response(json!({"content":"review complete"}), "stop"));
        })
        .await;
    let session = Session::open(original).await.unwrap();
    session.create_agent("create", "reviewer", agent()).unwrap();
    session
        .send(Input::new("review", "reviewer", "read the review file"))
        .unwrap();
    idle(&session, "reviewer").await;
    session.close().await.unwrap();
    first.assert_hits_async(1).await;
    second.assert_hits_async(1).await;
    let bodies = common::take_http_requests(&path);
    let assistant = bodies[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "assistant")
        .collect::<Vec<_>>();
    assert_eq!(1, assistant.len());
    assert_eq!("READING_REVIEW_FILE", assistant[0]["content"]);
    assert_eq!(1, assistant[0]["tool_calls"].as_array().unwrap().len());
    assert_eq!("review-read", assistant[0]["tool_calls"][0]["id"]);
}

#[tokio::test]
async fn a_named_connection_change_commits_a_successor_without_native_state_transfer() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (mut original, path) = options(&server, workspace.path(), data.path());
    original
        .config
        .providers
        .insert("other".into(), original.config.providers["openai"].clone());
    let first = server.mock_async(|when, then| {
        when.method(POST).path(&path).matches(common::capture_http_request).matches(|request| !request.body.as_ref().unwrap().windows(b"SECOND_CONNECTION".len()).any(|window| window == b"SECOND_CONNECTION"));
        then.status(200).body(format!("data: {}\n\ndata: {}\n\ndata: [DONE]\n\n", json!({"id":"native","choices":[{"delta":{"content":"PORTABLE_ANSWER","reasoning":"PRIVATE_THINKING","reasoning_details":[{"index":0,"type":"reasoning.encrypted","data":"PRIVATE_NATIVE_STATE"}]},"finish_reason":null}]}), json!({"id":"native","choices":[{"delta":{},"finish_reason":"stop"}]})));
    }).await;
    let second = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .matches(common::capture_http_request)
                .body_contains("SECOND_CONNECTION");
            then.status(200)
                .body(response(json!({"content":"done"}), "stop"));
        })
        .await;
    let session = Session::open(original).await.unwrap();
    let mut config = agent();
    config.instructions = Some("ORIGINAL_BEHAVIOR".into());
    session
        .create_agent("create", "main", config.clone())
        .unwrap();
    session
        .send(Input::new("first", "main", "FIRST_CONNECTION"))
        .unwrap();
    idle(&session, "main").await;
    config.connection = Some("other".into());
    config.model = Some("context-fixture".into());
    config.instructions = Some("REPLACEMENT_BEHAVIOR".into());
    session
        .configure_agent("configure", "main", config)
        .unwrap();
    session
        .send(Input::new("second", "main", "SECOND_CONNECTION"))
        .unwrap();
    idle(&session, "main").await;
    let events = session.events(0, 256).unwrap();
    session.close().await.unwrap();
    first.assert_hits_async(1).await;
    second.assert_hits_async(1).await;
    let bodies = common::take_http_requests(&path);
    let continuation = bodies[1].to_string();
    assert!(continuation.contains("PORTABLE_ANSWER"));
    assert!(continuation.contains("REPLACEMENT_BEHAVIOR"));
    for incompatible in [
        "PRIVATE_THINKING",
        "PRIVATE_NATIVE_STATE",
        "ORIGINAL_BEHAVIOR",
    ] {
        assert!(!continuation.contains(incompatible));
    }
    let segments = facts(&events, "runtime.context_segment", "main");
    assert_eq!(2, segments.len());
    assert_eq!(segments[0]["id"], segments[1]["predecessor"]);
    assert_eq!("other", segments[1]["connection"]);
}
