#[path = "../common/mod.rs"]
mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use httpmock::prelude::{HttpMockRequest, MockServer, POST};
use httpmock::Mock;
use serde_json::{json, Value};

use kuku::config::{ProviderFormat, ProviderProfile, ThinkLevel};
use kuku::event::{EventPayload, EventStore, ModelStopReason, StoredEvent};
use kuku::{AgentConfig, AgentSnapshot, AgentStatus, Input, Session, SessionOptions};

const CHAT: &str = include_str!("fixtures/openrouter_chat_duplicate_finish.sse");
const RESPONSES: &str = include_str!("fixtures/openrouter_responses_done.sse");
const SEED: &str = "gateway seed content\n";

fn options(
    server: &MockServer,
    workspace: &Path,
    data: &Path,
    format: ProviderFormat,
) -> SessionOptions {
    let mut config = common::test_config();
    let provider = config.providers.get_mut("openai").unwrap();
    provider.base_url = server.url("");
    provider.format = format;
    provider.options.profile = ProviderProfile::OpenRouter;
    let tier = config.tiers.get_mut("balanced").unwrap();
    tier.provider = "openai".into();
    tier.model = "deepseek/deepseek-v4.1-flash-1017".into();
    tier.think = ThinkLevel::Off;
    config.handoff.enabled = false;
    config.plugin.enabled = false;
    SessionOptions::new(workspace, data, config)
}

fn body(request: &HttpMockRequest) -> Value {
    serde_json::from_slice(request.body.as_deref().unwrap()).unwrap()
}

fn initial(request: &HttpMockRequest) -> bool {
    let body = body(request);
    body["messages"]
        .as_array()
        .or_else(|| body["input"].as_array())
        .unwrap()
        .iter()
        .all(|item| item["role"] != "tool" && item["type"] != "function_call_output")
}

fn responses_continuation(request: &HttpMockRequest) -> bool {
    let body = body(request);
    let input = body["input"].as_array().unwrap();
    let calls = input
        .iter()
        .filter(|item| item["type"] == "function_call")
        .collect::<Vec<_>>();
    let results = input
        .iter()
        .filter(|item| item["type"] == "function_call_output")
        .collect::<Vec<_>>();
    calls.len() == 2
        && results.len() == 2
        && input
            .iter()
            .any(|item| item["type"] == "reasoning" && item["id"] == "gateway_2")
        && [
            ("gateway_4", "find_files", json!({"path":"."})),
            ("gateway_6", "read_file", json!({"path":"seed.txt"})),
        ]
        .iter()
        .all(|(id, name, args)| {
            calls.iter().any(|item| {
                item["call_id"] == *id
                    && item["name"] == *name
                    && item["arguments"]
                        .as_str()
                        .and_then(|args| serde_json::from_str::<Value>(args).ok())
                        .as_ref()
                        == Some(args)
            }) && results.iter().any(|item| {
                item["call_id"] == *id
                    && item["output"].as_str().is_some_and(|output| {
                        output.contains(if *name == "read_file" {
                            "gateway seed content"
                        } else {
                            "seed.txt"
                        })
                    })
            })
        })
}

#[cfg(target_os = "linux")]
fn chat_continuation(request: &HttpMockRequest) -> bool {
    let body = body(request);
    let messages = body["messages"].as_array().unwrap();
    let calls = messages
        .iter()
        .filter_map(|message| message["tool_calls"].as_array())
        .flatten()
        .collect::<Vec<_>>();
    let results = messages
        .iter()
        .filter(|message| message["role"] == "tool")
        .collect::<Vec<_>>();
    calls.len() == 1
        && calls[0]["id"] == "gateway_2"
        && calls[0]["function"]["name"] == "run_command"
        && calls[0]["function"]["arguments"].as_str().and_then(|args| serde_json::from_str::<Value>(args).ok()).is_some_and(|args| args["shell"] == "sh" && args["cwd"] == "." && args["command"] == "ls -la && echo \"---\" && sha256sum seed.txt && echo \"---\" && xxd seed.txt | head -5" && args["timeout"] == 30)
        && results.len() == 1
        && results[0]["tool_call_id"] == "gateway_2"
        && results[0]["content"].as_str().is_some_and(|output| output.contains("seed.txt") && output.contains("gateway seed con"))
}

fn completion(format: ProviderFormat) -> String {
    match format {
        ProviderFormat::OpenAiChat => common::openai_sse_response(json!({
            "id":"gateway_final", "choices":[{"message":{"content":"gateway complete"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":10,"completion_tokens":2}
        })),
        ProviderFormat::OpenAiResponses => concat!(
            "event: response.created\ndata: {\"response\":{\"id\":\"gateway_final\",\"status\":\"in_progress\"}}\n\n",
            "event: response.output_text.delta\ndata: {\"delta\":\"gateway complete\"}\n\n",
            "event: response.completed\ndata: {\"response\":{\"id\":\"gateway_final\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}}\n\n",
            "data: [DONE]\n\n"
        ).into(),
        ProviderFormat::Anthropic => unreachable!(),
    }
}

async fn mock<'a>(
    server: &'a MockServer,
    path: &str,
    matcher: fn(&HttpMockRequest) -> bool,
    response: &str,
) -> Mock<'a> {
    server
        .mock_async(|when, then| {
            when.method(POST).path(path).matches(matcher);
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(response);
        })
        .await
}

async fn settle(session: &Session) -> AgentSnapshot {
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
    .expect("gateway fixture did not settle through the public Session")
}

fn assert_tools(events: &[StoredEvent], expected: &[(&str, &str)]) {
    let calls = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ToolCall {
                tool_call_id, tool, ..
            } => Some((tool_call_id.as_str(), tool.as_str())),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(expected.iter().copied().collect::<BTreeMap<_, _>>(), calls);
    for (id, _) in expected {
        assert_eq!(1, events.iter().filter(|event| matches!(&event.payload, EventPayload::ToolResult { tool_call_id, status, .. } if tool_call_id == id && status == "ok")).count());
        assert_eq!(1, events.iter().filter(|event| matches!(&event.payload, EventPayload::Unknown(value) if value["kind"] == "runtime.tool_dispatched" && value["tool_call_id"] == *id)).count());
    }
    let reasons = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ModelResponse { stop_reason, .. } => stop_reason.clone(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        vec![ModelStopReason::ToolUse, ModelStopReason::EndTurn],
        reasons
    );
}

fn assert_interrupted_without_tools(events: &[StoredEvent], turn: u64, reason: &str) {
    assert!(
        events.iter().any(|event| matches!(&event.payload,
            EventPayload::TurnInterrupted { conversation, turn: interrupted, reason: actual, .. }
                if conversation == "main" && *interrupted == turn && actual == reason
        )),
        "missing interruption {reason} for turn {turn}"
    );
    assert!(!events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolCall { .. } | EventPayload::ToolResult { .. }
    )));
    assert!(!events.iter().any(|event| matches!(&event.payload, EventPayload::Unknown(value) if value["kind"] == "runtime.tool_dispatched")));
}

#[tokio::test]
async fn recorded_openrouter_responses_done_dispatches_both_tools_and_continues() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("seed.txt"), SEED).unwrap();
    let first = mock(&server, "/responses", initial, RESPONSES).await;
    let second = mock(
        &server,
        "/responses",
        responses_continuation,
        &completion(ProviderFormat::OpenAiResponses),
    )
    .await;
    let session = Session::open(options(
        &server,
        workspace.path(),
        data.path(),
        ProviderFormat::OpenAiResponses,
    ))
    .await
    .unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("input", "main", "read the seed file"))
        .unwrap();
    let snapshot = settle(&session).await;
    assert_eq!(
        AgentStatus::Idle,
        snapshot.status,
        "{:?}",
        snapshot.last_error
    );
    assert_eq!(None, snapshot.last_error);
    assert_eq!(Some("gateway complete"), snapshot.last_output.as_deref());
    assert_eq!(Some(3361), snapshot.usage.input_tokens);
    assert_eq!(Some(76), snapshot.usage.output_tokens);
    assert_eq!(2, snapshot.usage.model_requests);
    session.close().await.unwrap();
    let events = EventStore::replay(session.events_path()).unwrap();
    assert_tools(
        &events,
        &[("gateway_4", "find_files"), ("gateway_6", "read_file")],
    );
    assert!(events.iter().any(|event| matches!(&event.payload, EventPayload::Unknown(value) if value["kind"] == "runtime.provider_report" && value["report"]["cost_usd"] == 0.000157636)));
    first.assert_hits_async(1).await;
    second.assert_hits_async(1).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn recorded_openrouter_chat_duplicate_finish_dispatches_command_and_continues() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("seed.txt"), SEED).unwrap();
    let first = mock(&server, "/chat/completions", initial, CHAT).await;
    let second = mock(
        &server,
        "/chat/completions",
        chat_continuation,
        &completion(ProviderFormat::OpenAiChat),
    )
    .await;
    let session = Session::open(options(
        &server,
        workspace.path(),
        data.path(),
        ProviderFormat::OpenAiChat,
    ))
    .await
    .unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("input", "main", "inspect the seed file"))
        .unwrap();
    let snapshot = settle(&session).await;
    assert_eq!(
        AgentStatus::Idle,
        snapshot.status,
        "{:?}",
        snapshot.last_error
    );
    assert_eq!(None, snapshot.last_error);
    assert_eq!(Some("gateway complete"), snapshot.last_output.as_deref());
    assert_eq!(Some(3363), snapshot.usage.input_tokens);
    assert_eq!(Some(116), snapshot.usage.output_tokens);
    assert_eq!(2, snapshot.usage.model_requests);
    session.close().await.unwrap();
    let events = EventStore::replay(session.events_path()).unwrap();
    assert_tools(&events, &[("gateway_2", "run_command")]);
    assert!(events.iter().any(|event| matches!(&event.payload, EventPayload::Unknown(value) if value["kind"] == "runtime.provider_report" && value["report"]["cost_usd"] == 0.000177708 && value["report"]["provider"] == "Wafer")));
    first.assert_hits_async(1).await;
    second.assert_hits_async(1).await;
}

#[tokio::test]
async fn responses_done_without_terminal_event_does_not_dispatch_tools() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("seed.txt"), SEED).unwrap();
    let truncated = RESPONSES
        .split_inclusive('\n')
        .filter(|line| !line.contains("\"type\":\"response.completed\""))
        .collect::<String>();
    let first = mock(&server, "/responses", initial, &truncated).await;
    let session = Session::open(options(
        &server,
        workspace.path(),
        data.path(),
        ProviderFormat::OpenAiResponses,
    ))
    .await
    .unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("input", "main", "read the seed file"))
        .unwrap();
    let snapshot = settle(&session).await;
    assert_eq!(AgentStatus::Interrupted, snapshot.status);
    let expected_error = "[DONE] before a terminal response event";
    assert!(
        snapshot
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains(expected_error)),
        "unexpected provider error: {:?}",
        snapshot.last_error
    );
    session.close().await.unwrap();
    let events = EventStore::replay(session.events_path()).unwrap();
    let turn = snapshot.turn.unwrap();
    assert_interrupted_without_tools(&events, turn, "invalid_request");
    assert!(events.iter().any(|event| matches!(&event.payload,
        EventPayload::ModelError { conversation: Some(agent), turn: requested, kind, message, .. }
            if agent == "main" && *requested == turn && kind == "invalid_request" && message.contains(expected_error)
    )));
    first.assert_hits_async(1).await;
}

#[tokio::test]
async fn chat_conflicting_finish_reason_does_not_dispatch_tools() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (prefix, suffix) = CHAT
        .rsplit_once("\"finish_reason\":\"tool_calls\"")
        .unwrap();
    let conflicting = format!("{prefix}\"finish_reason\":\"stop\"{suffix}");
    let first = mock(&server, "/chat/completions", initial, &conflicting).await;
    let session = Session::open(options(
        &server,
        workspace.path(),
        data.path(),
        ProviderFormat::OpenAiChat,
    ))
    .await
    .unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("input", "main", "inspect the seed file"))
        .unwrap();
    let snapshot = settle(&session).await;
    assert_eq!(AgentStatus::Interrupted, snapshot.status);
    assert!(
        snapshot
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("invalid_response")),
        "unexpected provider error: {:?}",
        snapshot.last_error
    );
    session.close().await.unwrap();
    let events = EventStore::replay(session.events_path()).unwrap();
    let turn = snapshot.turn.unwrap();
    assert_interrupted_without_tools(&events, turn, "invalid_response");
    assert!(events.iter().any(|event| matches!(&event.payload,
        EventPayload::ModelResponse { conversation: Some(agent), turn: requested, stop_reason: Some(ModelStopReason::InvalidResponse), .. }
            if agent == "main" && *requested == turn
    )));
    first.assert_hits_async(1).await;
}
