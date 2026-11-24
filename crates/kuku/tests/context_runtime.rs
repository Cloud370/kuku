mod common;

#[path = "context_runtime_order/mod.rs"]
mod prelude_order;
#[path = "context_runtime_capacity/mod.rs"]
mod request_capacity;

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use httpmock::prelude::{HttpMockRequest, MockServer, POST};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use kuku::context::{CanonicalMessage, MessageBlock, Role};
use kuku::event::{EventPayload, EventStore, StoredEvent};
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
    tier.max_output_tokens = Some(128);
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

fn request_tool_names(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap().to_string())
        .collect()
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
            assert!(
                body["messages"]
                    .as_array()
                    .unwrap()
                    .starts_with(bodies[index - 1]["messages"].as_array().unwrap())
            );
        }
    }
    let segments = facts(&events, "runtime.context_segment", "main");
    let prepared = facts(&events, "runtime.step_prepared", "main");
    let admitted = facts(&events, "runtime.context_admitted", "main");
    assert_eq!(1, segments.len());
    assert_eq!(3, prepared.len());
    assert!(!segments[0]["prelude"].to_string().contains("Current date:"));
    assert_eq!(0, prepared[0]["prefix_context_head"]);
    assert!(prepared[1]["prefix_context_head"].as_u64().unwrap() > 0);
    assert!(prepared[2]["prefix_context_head"].as_u64().unwrap() > 0);
    assert_ne!(
        prepared[0]["history_prefix_hash"],
        prepared[1]["history_prefix_hash"]
    );
    assert_ne!(
        prepared[1]["history_prefix_hash"],
        prepared[2]["history_prefix_hash"]
    );
    assert!(
        prepared
            .iter()
            .all(|step| step["prefix_hash"] == prepared[0]["prefix_hash"])
    );
    for (body, input) in bodies
        .iter()
        .zip(["FIRST_INPUT", "SECOND_INPUT", "THIRD_INPUT"])
    {
        assert_eq!(1, body.to_string().matches("<kuku_runtime_date>").count());
        let last_user_message = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|message| message["role"] == "user")
            .unwrap();
        assert!(last_user_message["content"].to_string().contains(input));
    }
    let date_admissions = admitted
        .iter()
        .filter(|event| event["origin"] == "runtime")
        .filter_map(|event| {
            serde_json::from_value::<CanonicalMessage>(event["message"].clone()).ok()
        })
        .filter(|message| {
            text_message(message)["content"]
                .as_str()
                .unwrap()
                .contains("<kuku_runtime_date>")
        })
        .count();
    assert_eq!(1, date_admissions);
    for (step, body) in prepared.iter().zip(&bodies) {
        assert!(step.get("body").is_none());
        assert!(step["context_prepare_us"].is_u64());
        assert!(step["request_prepare_us"].is_u64());
        assert_eq!(
            format!("{:x}", Sha256::digest(serde_json::to_vec(body).unwrap())),
            step["body_hash"]
        );
        let prefix = admitted
            .iter()
            .take(step["prefix_context_head"].as_u64().unwrap() as usize)
            .map(|value| {
                serde_json::from_value::<CanonicalMessage>(value["message"].clone()).unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            format!("{:x}", Sha256::digest(serde_json::to_vec(&prefix).unwrap())),
            step["history_prefix_hash"]
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

#[tokio::test]
async fn provider_context_reflects_each_agents_tools_and_keeps_main_frozen() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("AGENTS.md"), "PROJECT_POLICY_MARKER").unwrap();
    let (original, path) = options(&server, workspace.path(), data.path());
    let model = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .matches(common::capture_http_request);
            then.status(200)
                .body(response(json!({"content":"done"}), "stop"));
        })
        .await;
    let session = Session::open(original).await.unwrap();
    for (address, tools) in [
        ("main", None),
        (
            "reader",
            Some(vec!["read_output".into(), "list_values".into()]),
        ),
        ("tool-free", Some(vec![])),
    ] {
        session
            .create_agent(
                &format!("create-{address}"),
                address,
                AgentConfig {
                    tools,
                    instructions: Some(format!("ROLE_{address}")),
                    ..Default::default()
                },
            )
            .unwrap();
        let mut input = Input::new(format!("input-{address}"), address, "reply to this task");
        if address != "main" {
            input.source = kuku::InputSource::Agent("main".into());
        }
        session.send(input).unwrap();
        idle(&session, address).await;
    }
    let mut reader = session.agent("reader").unwrap().config;
    reader.tools = Some(vec![]);
    session
        .configure_agent("restrict-reader", "reader", reader)
        .unwrap();
    session
        .send(Input::new("reader-restricted", "reader", "reply directly"))
        .unwrap();
    idle(&session, "reader").await;
    session
        .send(Input::new("input-main-again", "main", "follow up"))
        .unwrap();
    idle(&session, "main").await;
    let prepared = facts(
        &session.events(0, 256).unwrap(),
        "runtime.step_prepared",
        "main",
    );
    session.close().await.unwrap();
    model.assert_hits_async(5).await;
    let bodies = common::take_http_requests(&path);
    let main = bodies[0]["messages"].to_string();
    assert!(main.contains("<kuku_models>"));
    assert!(main.contains("<kuku_delegation>"));
    assert!(main.contains("Use list_skills"));
    assert!(main.contains("Use search_skills"));
    assert!(request_tool_names(&bodies[0]).contains(&"create_agent".into()));
    for (body, address) in bodies[1..3].iter().zip(["reader", "tool-free"]) {
        let messages = body["messages"].to_string();
        assert!(messages.contains("PROJECT_POLICY_MARKER"));
        assert!(messages.contains(&format!("ROLE_{address}")));
        assert!(messages.contains(&format!("Your Session Agent address is {address}.")));
        assert!(body["messages"].as_array().unwrap().iter().any(|message| {
            message["content"]
                .as_str()
                .is_some_and(|text| text.starts_with(r#"<kuku_agent_message from="main">"#))
        }));
        for marker in [
            "<kuku_models>",
            "<kuku_skills>",
            "<kuku_delegation>",
            "<kuku_memory_guidance>",
            "run_command",
            "edit_file",
        ] {
            assert!(!messages.contains(marker), "{address} saw {marker}");
        }
    }
    assert_eq!(
        request_tool_names(&bodies[1]),
        ["read_output", "list_values"]
    );
    assert!(bodies[2]["tools"].is_null() || request_tool_names(&bodies[2]).is_empty());
    assert!(
        bodies[2]["messages"]
            .to_string()
            .contains("put the content in your reply")
    );
    assert!(
        !bodies[2]["messages"]
            .to_string()
            .contains("<kuku_tool_guidance>")
    );
    assert!(bodies[3]["tools"].is_null() || request_tool_names(&bodies[3]).is_empty());
    assert!(
        !bodies[3]["messages"]
            .to_string()
            .contains("<kuku_tool_guidance>")
    );
    assert_eq!(
        serde_json::to_vec(&bodies[0]["tools"]).unwrap(),
        serde_json::to_vec(&bodies[4]["tools"]).unwrap()
    );
    let before = bodies[0]["messages"].as_array().unwrap();
    let after = bodies[4]["messages"].as_array().unwrap();
    assert_eq!(
        serde_json::to_vec(before).unwrap(),
        serde_json::to_vec(&after[..before.len()]).unwrap()
    );
    assert_eq!(prepared[0]["prefix_hash"], prepared[1]["prefix_hash"]);
}

#[tokio::test]
async fn agent_senders_remain_attached_to_each_input_in_one_turn_and_after_reopen() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (mut original, path) = options(&server, workspace.path(), data.path());
    for (before, after, id) in [
        ("", "peer step one", "first"),
        ("peer step one", "peer step two", "second"),
    ] {
        let path = path.clone();
        let marker = if before.is_empty() {
            "begin worker turn"
        } else {
            before
        };
        server
            .mock_async(move |when, then| {
                when.method(POST)
                    .path(&path)
                    .matches(common::capture_http_request)
                    .body_contains(marker)
                    .matches(if after == "peer step one" {
                        |request| tool_count(request) == 0
                    } else {
                        |request| tool_count(request) == 1
                    });
                then.status(200).delay(Duration::from_millis(150)).body(response(json!({
                "tool_calls":[{
                    "id":id, "type":"function", "function":{"name":"list_values","arguments":"{}"}
                }]
            }), "tool_calls"));
            })
            .await;
    }
    let last = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .matches(common::capture_http_request)
                .body_contains("peer step two");
            then.status(200)
                .body(response(json!({"content":"done"}), "stop"));
        })
        .await;
    let session = Session::open(original.clone()).await.unwrap();
    for address in ["main", "helper", "worker"] {
        session
            .create_agent(
                address,
                address,
                AgentConfig {
                    tools: Some(vec!["list_values".into()]),
                    ..agent()
                },
            )
            .unwrap();
    }
    session
        .send(Input::new("start", "worker", "begin worker turn"))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !common::take_http_requests(&path).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the first provider request started");
    for (sender, text) in [("main", "peer step one"), ("helper", "peer step two")] {
        let mut input = Input::new(format!("delivery-{sender}"), "worker", text);
        input.source = kuku::InputSource::Agent(sender.into());
        input.activation = kuku::Activation::Enqueue;
        session.send(input).unwrap();
    }
    idle(&session, "worker").await;
    last.assert_hits_async(1).await;
    let messages = session
        .events(0, 256)
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.payload {
            EventPayload::MessageUser {
                turn,
                from: Some(sender),
                text,
                ..
            } => Some((turn, sender, text)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let turn = messages[0].0;
    assert_eq!(
        messages,
        [
            (turn, "main".into(), "peer step one".into()),
            (turn, "helper".into(), "peer step two".into()),
        ]
    );
    let before = common::take_http_requests(&path).pop().unwrap();
    original.session_id = Some(session.id().into());
    session.close().await.unwrap();
    let reopened = Session::open(original).await.unwrap();
    reopened
        .send(Input::new("after", "worker", "continue after reopen"))
        .unwrap();
    idle(&reopened, "worker").await;
    let after = common::take_http_requests(&path).pop().unwrap();
    let prefix = before["messages"].as_array().unwrap();
    let history = after["messages"].as_array().unwrap();
    assert_eq!(
        serde_json::to_vec(prefix).unwrap(),
        serde_json::to_vec(&history[..prefix.len()]).unwrap()
    );
    for (sender, text) in [("main", "peer step one"), ("helper", "peer step two")] {
        let expected =
            format!("<kuku_agent_message from=\"{sender}\">\n{text}\n</kuku_agent_message>");
        assert!(history.iter().any(|message| message["content"] == expected));
    }
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn unprepared_catalog_does_not_freeze_the_first_model_request() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let prompts = tempfile::tempdir().unwrap();
    std::fs::write(prompts.path().join("system.md"), "UNPREPARED_SYSTEM").unwrap();
    let (mut original, path) = options(&server, workspace.path(), data.path());
    original.prompts_dir = Some(prompts.path().into());
    let model = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .matches(common::capture_http_request);
            then.status(200)
                .body(response(json!({"content":"done"}), "stop"));
        })
        .await;
    let session = Session::open(original.clone()).await.unwrap();
    session.create_agent("create", "main", agent()).unwrap();
    original.session_id = Some(session.id().into());
    let events_path = session.events_path().to_path_buf();
    session.close().await.unwrap();
    let catalog = kuku::PromptCatalog::load_from_dir(prompts.path()).unwrap();
    let orphan = EventStore::open(&events_path)
        .unwrap()
        .append(EventPayload::Unknown(json!({
            "kind":"runtime.context_segment", "agent":"main", "catalog":catalog,
        })))
        .unwrap();
    std::fs::write(prompts.path().join("system.md"), "CURRENT_SYSTEM").unwrap();
    let session = Session::open(original).await.unwrap();
    session
        .send(Input::new("first", "main", "first request"))
        .unwrap();
    idle(&session, "main").await;
    let events = session.events(0, 256).unwrap();
    session.close().await.unwrap();
    model.assert_hits_async(1).await;
    let bodies = common::take_http_requests(&path);
    assert_eq!("CURRENT_SYSTEM", bodies[0]["messages"][0]["content"]);
    assert!(!bodies[0].to_string().contains("UNPREPARED_SYSTEM"));
    let prepared = facts(&events, "runtime.step_prepared", "main");
    assert_eq!(1, prepared.len());
    assert_ne!(json!(orphan.id), prepared[0]["segment"]);
}

#[tokio::test]
async fn rendered_policy_boundary_survives_behavior_reconfiguration() {
    for (policy, behavior) in [
        ("", "OLD_BEHAVIOR"),
        ("OLD_BEHAVIOR", "OLD_BEHAVIOR"),
        ("", ""),
    ] {
        let server = MockServer::start_async().await;
        let workspace = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let prompts = tempfile::tempdir().unwrap();
        std::fs::create_dir(prompts.path().join("blocks")).unwrap();
        std::fs::write(
            prompts.path().join("blocks/project-policy.md"),
            "{{project_instructions_rendered}}",
        )
        .unwrap();
        std::fs::write(workspace.path().join("AGENTS.md"), policy).unwrap();
        let (mut original, path) = options(&server, workspace.path(), data.path());
        original.prompts_dir = Some(prompts.path().into());
        let model = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path(&path)
                    .matches(common::capture_http_request);
                then.status(200)
                    .body(response(json!({"content":"done"}), "stop"));
            })
            .await;
        let session = Session::open(original.clone()).await.unwrap();
        let mut config = agent();
        config.instructions = Some(behavior.into());
        session
            .create_agent("create", "main", config.clone())
            .unwrap();
        session.send(Input::new("first", "main", "first")).unwrap();
        idle(&session, "main").await;
        original.session_id = Some(session.id().into());
        session.close().await.unwrap();
        std::fs::remove_dir_all(prompts.path()).unwrap();
        let session = Session::open(original).await.unwrap();
        config.instructions = Some("NEW_BEHAVIOR".into());
        session
            .configure_agent("configure", "main", config)
            .unwrap();
        session.send(Input::new("next", "main", "next")).unwrap();
        idle(&session, "main").await;
        let events = session.events(0, 256).unwrap();
        session.close().await.unwrap();
        model.assert_hits_async(2).await;
        let bodies = common::take_http_requests(&path);
        let segments = facts(&events, "runtime.context_segment", "main");
        let slot = 2;
        assert_eq!("NEW_BEHAVIOR", bodies[1]["messages"][slot + 1]["content"]);
        if !policy.is_empty() {
            assert_eq!(policy, bodies[1]["messages"][slot + 2]["content"]);
        }
        assert_eq!(2, segments.len());
        assert_eq!(json!(slot), segments[0]["identity_slot"]);
        assert_eq!(json!(slot), segments[1]["identity_slot"]);
    }
}

#[tokio::test]
async fn unconfirmed_runtime_suffix_can_be_replaced_before_a_retried_preparation() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let prompts = tempfile::tempdir().unwrap();
    std::fs::create_dir(prompts.path().join("runtime")).unwrap();
    std::fs::write(
        prompts.path().join("runtime/current-date.md"),
        "FIXED_RUNTIME_DATE",
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
                .body(response(json!({"content":"done"}), "stop"));
        })
        .await;
    let session = Session::open(original.clone()).await.unwrap();
    session.create_agent("create", "main", agent()).unwrap();
    session
        .send(Input::new("first", "main", "first-input"))
        .unwrap();
    idle(&session, "main").await;
    let mut pending = Input::new("candidate", "main", "retry-input");
    pending.activation = kuku::Activation::Enqueue;
    session.send(pending).unwrap();
    original.session_id = Some(session.id().into());
    let events_path = session.events_path().to_path_buf();
    let prepared = facts(
        &session.events(0, 256).unwrap(),
        "runtime.step_prepared",
        "main",
    );
    assert_eq!(2, prepared[0]["context_head"]);
    let segment = prepared[0]["segment"].as_u64().unwrap();
    let initial_turn = prepared[0]["turn"].as_u64().unwrap();
    session.close().await.unwrap();
    let unconfirmed_source_head = {
        let mut writer = EventStore::open(&events_path).unwrap();
        let turn = writer
            .append(EventPayload::Unknown(
                json!({"kind":"runtime.operation", "record":{
                    "operation":"inputs_accepted", "address":"main", "input_ids":["candidate"]
                }}),
            ))
            .unwrap()
            .id;
        writer
            .append(EventPayload::TurnStarted {
                ts: "fixture".into(),
                conversation: "main".into(),
                turn,
            })
            .unwrap();
        let source_head = writer
            .append(EventPayload::MessageUser {
                ts: "fixture".into(),
                conversation: "main".into(),
                turn,
                text: "retry-input".into(),
                from: None,
                job_delivery: false,
            })
            .unwrap()
            .id;
        for (ordinal, origin, turn, message) in [
            (
                2,
                "response",
                initial_turn,
                CanonicalMessage::assistant(vec![MessageBlock::Text("done".into())]),
            ),
            (3, "input", turn, CanonicalMessage::user_text("retry-input")),
            (
                4,
                "runtime",
                turn,
                CanonicalMessage::user_text("UNCONFIRMED_DYNAMIC_NOTICE"),
            ),
        ] {
            writer.append(EventPayload::Unknown(json!({"kind":"runtime.context_admitted", "agent":"main", "segment":segment,
                "ordinal":ordinal, "origin":origin, "turn":turn, "source_head":source_head, "message":message}))).unwrap();
        }
        source_head
    };
    let session = Session::open(original.clone()).await.unwrap();
    assert_eq!(
        AgentStatus::Interrupted,
        session.agent("main").unwrap().status
    );
    session.advance("main").unwrap();
    idle(&session, "main").await;
    let retry_events = session.events(0, 256).unwrap();
    let admissions = facts(&retry_events, "runtime.context_admitted", "main");
    let preparations = facts(&retry_events, "runtime.step_prepared", "main");
    assert_eq!(2, preparations.len());
    let retried = &preparations[1];
    assert_eq!(segment, retried["segment"]);
    assert_eq!(2, retried["prefix_context_head"]);
    assert_eq!(4, retried["context_head"]);
    assert_eq!(3, retried["current_input"]);
    let retried_source_head = retried["source_head"].as_u64().unwrap();
    assert!(retried_source_head > unconfirmed_source_head);
    for ordinal in [2, 3] {
        let versions = admissions
            .iter()
            .filter(|value| value["segment"] == segment && value["ordinal"] == ordinal)
            .collect::<Vec<_>>();
        assert_eq!(2, versions.len(), "ordinal {ordinal}");
        assert_eq!(versions[0]["message"], versions[1]["message"]);
        assert_eq!(unconfirmed_source_head, versions[0]["source_head"]);
        assert_eq!(retried_source_head, versions[1]["source_head"]);
    }
    let abandoned_sources: Vec<_> = admissions
        .iter()
        .filter(|value| value["segment"] == segment && value["ordinal"] == 4)
        .map(|value| value["source_head"].as_u64().unwrap())
        .collect();
    assert_eq!(vec![unconfirmed_source_head], abandoned_sources);
    session.close().await.unwrap();
    let session = Session::open(original).await.unwrap();
    session
        .send(Input::new("after-retry", "main", "after-retry"))
        .unwrap();
    idle(&session, "main").await;
    let reopened_events = session.events(0, 256).unwrap();
    let reopened_preparations = facts(&reopened_events, "runtime.step_prepared", "main");
    assert_eq!(3, reopened_preparations.len());
    let reopened = &reopened_preparations[2];
    assert_eq!(segment, reopened["segment"]);
    assert_eq!(4, reopened["prefix_context_head"]);
    assert_eq!(6, reopened["context_head"]);
    let reopened_admissions = facts(&reopened_events, "runtime.context_admitted", "main");
    let replacement = reopened_admissions
        .iter()
        .find(|value| {
            value["segment"] == segment
                && value["ordinal"] == 4
                && value["source_head"] == reopened["source_head"]
        })
        .unwrap();
    assert_eq!("response", replacement["origin"]);
    assert_eq!(
        serde_json::to_value(CanonicalMessage::assistant(vec![MessageBlock::Text(
            "done".into()
        )]))
        .unwrap(),
        replacement["message"]
    );
    session.close().await.unwrap();
    model.assert_hits_async(3).await;
    let bodies = common::take_http_requests(&path);
    assert_eq!(
        bodies[0]["messages"].as_array().unwrap().len() + 2,
        bodies[1]["messages"].as_array().unwrap().len()
    );
    for body in &bodies[1..] {
        assert_eq!(
            1,
            body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|message| message["role"] == "user" && message["content"] == "retry-input")
                .count()
        );
    }
    assert!(
        bodies[2]["messages"]
            .as_array()
            .unwrap()
            .starts_with(bodies[1]["messages"].as_array().unwrap())
    );
    assert!(
        !bodies
            .iter()
            .any(|body| body.to_string().contains("UNCONFIRMED_DYNAMIC_NOTICE"))
    );
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
    assert!(
        bodies[1]["messages"]
            .as_array()
            .unwrap()
            .starts_with(bodies[0]["messages"].as_array().unwrap())
    );
    assert!(
        bodies[2]["messages"]
            .as_array()
            .unwrap()
            .starts_with(bodies[1]["messages"].as_array().unwrap())
    );
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
async fn custom_runtime_date_template_is_deduplicated_by_rendered_message() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let prompts = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(prompts.path().join("runtime")).unwrap();
    std::fs::write(
        prompts.path().join("runtime/current-date.md"),
        "<kuku_custom_runtime_date>\n今天：{{current_date}}\n</kuku_custom_runtime_date>",
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
                .body(response(json!({"content":"answer"}), "stop"));
        })
        .await;
    let session = Session::open(original).await.unwrap();
    session.create_agent("create", "main", agent()).unwrap();
    for (command, text) in [("first", "FIRST_INPUT"), ("second", "SECOND_INPUT")] {
        session.send(Input::new(command, "main", text)).unwrap();
        idle(&session, "main").await;
    }
    let events = session.events(0, 256).unwrap();
    session.close().await.unwrap();
    model.assert_hits_async(2).await;
    let bodies = common::take_http_requests(&path);
    assert_eq!(2, bodies.len());
    for body in &bodies {
        assert_eq!(
            1,
            body.to_string()
                .matches("<kuku_custom_runtime_date>")
                .count()
        );
    }
    let date_admissions = facts(&events, "runtime.context_admitted", "main")
        .iter()
        .filter(|event| event["origin"] == "runtime")
        .filter_map(|event| {
            serde_json::from_value::<CanonicalMessage>(event["message"].clone()).ok()
        })
        .filter(|message| {
            text_message(message)["content"]
                .as_str()
                .is_some_and(|text| text.contains("<kuku_custom_runtime_date>"))
        })
        .count();
    assert_eq!(1, date_admissions);
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

#[tokio::test]
async fn ambiguous_chat_stream_keeps_partial_reply_out_of_the_reopened_context() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (mut options, path) = options(&server, workspace.path(), data.path());
    let partial = "PARTIAL_VISIBLE_REPLY";
    let failure = "provider reasoning fragment matches multiple items";
    let malformed = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"PARTIAL_VISIBLE_REPLY\",\"tool_calls\":[{\"index\":0,\"id\":\"never-dispatch\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"index\":0,\"id\":\"a\",\"type\":\"reasoning.encrypted\",\"data\":\"OPAQUE_A\"},{\"index\":0,\"id\":\"b\",\"type\":\"reasoning.encrypted\",\"data\":\"OPAQUE_B\"}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"index\":0,\"data\":\"ambiguous-fragment\"}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let first = server
        .mock_async(|when, then| {
            when.method(POST).path(&path);
            then.status(200).body(malformed);
        })
        .await;
    let session = Session::open(options.clone()).await.unwrap();
    session.create_agent("create", "main", agent()).unwrap();
    session
        .send(Input::new("first", "main", "fail the stream"))
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while session.agent("main").unwrap().status == kuku::AgentStatus::Running {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        session.agent("main").unwrap().status,
        kuku::AgentStatus::Interrupted
    );
    options.session_id = Some(session.id().into());
    session.close().await.unwrap();
    first.assert_hits_async(1).await;
    first.delete_async().await;

    let reopened = Session::open(options).await.unwrap();
    let events = reopened.events(0, 256).unwrap();
    assert_eq!(events.iter().filter(|event| matches!(&event.payload,
        EventPayload::TurnInterrupted { partial_reply: Some(reply), .. } if reply.text == partial && !reply.truncated
    )).count(), 1);
    assert!(events.iter().any(|event| matches!(&event.payload,
        EventPayload::ModelError { kind, message, .. } if kind == "invalid_response" && message == failure
    )));
    assert!(!events.iter().any(|event| matches!(&event.payload,
        EventPayload::ModelResponse { .. } | EventPayload::ToolCall { .. }
    ) || matches!(&event.payload, EventPayload::Unknown(value) if value["kind"] == "runtime.provider_replay")));

    let followup = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .matches(common::capture_http_request);
            then.status(200)
                .body(response(json!({"content":"recovered"}), "stop"));
        })
        .await;
    reopened
        .send(Input::new("followup", "main", "continue after failure"))
        .unwrap();
    reopened.advance("main").unwrap();
    idle(&reopened, "main").await;
    followup.assert_hits_async(1).await;
    let body = common::take_http_requests(&path).remove(0).to_string();
    for text in [partial, "OPAQUE_A", "OPAQUE_B", "never-dispatch"] {
        assert!(!body.contains(text), "{text}");
    }
    reopened.close().await.unwrap();
}
