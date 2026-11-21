use super::common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
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

fn anthropic_retry_options(server: &MockServer, workspace: &Path, data: &Path) -> SessionOptions {
    anthropic_session_options(
        server,
        workspace,
        data,
        "/retry-body-identical",
        ProviderProfile::Anthropic,
        None,
    )
}

fn anthropic_session_options(
    server: &MockServer,
    workspace: &Path,
    data: &Path,
    base_path: &str,
    profile: ProviderProfile,
    session_id: Option<&str>,
) -> SessionOptions {
    let mut config = common::test_config();
    let provider = config.providers.get_mut("anthropic").unwrap();
    provider.base_url = server.url(base_path);
    provider.options.profile = profile;
    config.handoff.enabled = false;
    config.plugin.enabled = false;
    let mut options = SessionOptions::new(workspace, data, config);
    options.session_id = session_id.map(str::to_owned);
    options
}

fn retry_request_bodies() -> &'static Mutex<Vec<Vec<u8>>> {
    static BODIES: OnceLock<Mutex<Vec<Vec<u8>>>> = OnceLock::new();
    BODIES.get_or_init(|| Mutex::new(Vec::new()))
}

fn capture_retry_request_body(request: &HttpMockRequest) -> bool {
    if let Some(body) = &request.body {
        retry_request_bodies().lock().unwrap().push(body.clone());
        true
    } else {
        false
    }
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

fn provider_replay_value(events: &[StoredEvent]) -> Value {
    events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::Unknown(value) if value["kind"] == "runtime.provider_replay" => {
                Some(value["value"].clone())
            }
            _ => None,
        })
        .expect("provider replay event should be durable")
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
    readable_responses_continue(ProviderProfile::OpenRouter).await;
}

#[tokio::test]
async fn deepseek_responses_keep_readable_reasoning_and_native_continuation() {
    readable_responses_continue(ProviderProfile::DeepSeek).await;
}

async fn readable_responses_continue(profile: ProviderProfile) {
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
    let mut settings = options(
        &server,
        workspace.path(),
        data.path(),
        ProviderFormat::OpenAiResponses,
    );
    settings
        .config
        .providers
        .get_mut("openai")
        .unwrap()
        .options
        .profile = profile;
    let session = Session::open(settings).await.unwrap();
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
    assert!(events.iter().any(|event| matches!(&event.payload,
        EventPayload::ModelResponse { text, thinking: Some(thinking), stop_reason: Some(ModelStopReason::ToolUse), .. }
            if text.is_empty() && thinking == "Let me read seed.txt first."
    )));
    assert!(events.iter().any(|event| matches!(&event.payload, EventPayload::Unknown(value) if value["kind"] == "runtime.provider_report" && value["report"]["cost_usd"] == 0.000157636)));
    first.assert_hits_async(1).await;
    second.assert_hits_async(1).await;
}

#[tokio::test]
async fn transient_provider_retry_sends_identical_request_body_bytes() {
    retry_request_bodies().lock().unwrap().clear();
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let first = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/retry-body-identical/v1/messages")
                .matches(capture_retry_request_body);
            then.status(503).body("temporarily unavailable");
        })
        .await;
    let session = Session::open(anthropic_retry_options(
        &server,
        workspace.path(),
        data.path(),
    ))
    .await
    .unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("retry_once", "main", "retry this model request"))
        .unwrap();

    tokio::time::timeout(Duration::from_secs(5), async {
        while first.hits_async().await == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("initial provider request was not observed");
    first.assert_hits_async(1).await;
    first.delete_async().await;

    let retry = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/retry-body-identical/v1/messages")
                .matches(capture_retry_request_body);
            then.status(200).body(common::anthropic_sse_response(json!({
                "id": "retry_success",
                "type": "message",
                "role": "assistant",
                "content": [{"type":"text","text":"retry succeeded"}],
                "stop_reason": "end_turn"
            })));
        })
        .await;

    let snapshot = settle(&session).await;
    assert_eq!(
        AgentStatus::Idle,
        snapshot.status,
        "{:?}",
        snapshot.last_error
    );
    assert_eq!(None, snapshot.last_error);
    assert_eq!(Some("retry succeeded"), snapshot.last_output.as_deref());
    retry.assert_hits_async(1).await;
    session.close().await.unwrap();

    let bodies = std::mem::take(&mut *retry_request_bodies().lock().unwrap());
    assert_eq!(2, bodies.len());
    assert_eq!(bodies[0], bodies[1]);

    let events = EventStore::replay(session.events_path()).unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::Unknown(value) if value["kind"] == "runtime.model_retry"
    )));
}

#[tokio::test]
async fn native_unsigned_thinking_only_turn_reopens_without_replaying_thinking() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let session_id = "native_thinking_only_reopen";
    let base_path = "/native-thinking-only-reopen";
    let endpoint = format!("{base_path}/v1/messages");
    let first = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(endpoint.as_str())
                .body_contains("native thinking-only seed")
                .matches(common::capture_http_request);
            then.status(200).body(common::anthropic_sse_response(json!({
                "id": "thinking_only",
                "type": "message",
                "role": "assistant",
                "content": [{"type":"thinking","thinking":"private reasoning"}],
                "stop_reason": "end_turn"
            })));
        })
        .await;
    let followup = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(endpoint.as_str())
                .body_contains("followup after empty native replay")
                .matches(common::capture_http_request);
            then.status(200).body(common::anthropic_sse_response(json!({
                "id": "after_thinking_only",
                "type": "message",
                "role": "assistant",
                "content": [{"type":"text","text":"follow-up accepted"}],
                "stop_reason": "end_turn"
            })));
        })
        .await;

    let session = Session::open(anthropic_session_options(
        &server,
        workspace.path(),
        data.path(),
        base_path,
        ProviderProfile::Anthropic,
        Some(session_id),
    ))
    .await
    .unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new(
            "thinking_only_seed",
            "main",
            "native thinking-only seed",
        ))
        .unwrap();
    let first_snapshot = settle(&session).await;
    assert_eq!(AgentStatus::Idle, first_snapshot.status);
    assert_eq!(None, first_snapshot.last_error);
    first.assert_hits_async(1).await;
    first.delete_async().await;
    let events_path = session.events_path().to_path_buf();
    session.close().await.unwrap();
    let history_before_reopen = std::fs::read(&events_path).unwrap();

    let reopened = Session::open(anthropic_session_options(
        &server,
        workspace.path(),
        data.path(),
        base_path,
        ProviderProfile::Anthropic,
        Some(session_id),
    ))
    .await
    .unwrap();
    reopened
        .send(Input::new(
            "after_thinking_only",
            "main",
            "followup after empty native replay",
        ))
        .unwrap();
    let reopened_snapshot = settle(&reopened).await;
    assert_eq!(
        AgentStatus::Idle,
        reopened_snapshot.status,
        "{:?}",
        reopened_snapshot.last_error
    );
    assert_eq!(None, reopened_snapshot.last_error);
    assert_eq!(
        Some("follow-up accepted"),
        reopened_snapshot.last_output.as_deref()
    );
    followup.assert_hits_async(1).await;
    reopened.close().await.unwrap();

    let history_after_reopen = std::fs::read(&events_path).unwrap();
    assert!(history_after_reopen.starts_with(&history_before_reopen));
    let bodies = common::take_http_requests(&endpoint);
    assert_eq!(2, bodies.len());
    assert!(bodies[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|message| {
            message["content"]
                .as_array()
                .is_none_or(|content| content.iter().all(|block| block["type"] != "thinking"))
        }));
    let events = EventStore::replay(&events_path).unwrap();
    assert_eq!(
        json!({"role":"assistant","content":[]}),
        provider_replay_value(&events)
    );
}

#[tokio::test]
async fn reopened_native_session_rejects_passive_compatible_profile_change() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let session_id = "native_to_compatible_reopen";
    let base_path = "/native-to-compatible-reopen";
    let endpoint = format!("{base_path}/v1/messages");
    let first = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(endpoint.as_str())
                .body_contains("native profile seed")
                .matches(common::capture_http_request);
            then.status(200).body(common::anthropic_sse_response(json!({
                "id": "native_unsigned",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type":"thinking","thinking":"private native reasoning"},
                    {"type":"text","text":"native answer"}
                ],
                "stop_reason": "end_turn"
            })));
        })
        .await;
    let second = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(endpoint.as_str())
                .body_contains("compatible profile follow-up")
                .matches(common::capture_http_request);
            then.status(200).body(common::anthropic_sse_response(json!({
                "id": "compatible_followup",
                "type": "message",
                "role": "assistant",
                "content": [{"type":"text","text":"compatible answer"}],
                "stop_reason": "end_turn"
            })));
        })
        .await;

    let session = Session::open(anthropic_session_options(
        &server,
        workspace.path(),
        data.path(),
        base_path,
        ProviderProfile::Anthropic,
        Some(session_id),
    ))
    .await
    .unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("native_seed", "main", "native profile seed"))
        .unwrap();
    let snapshot = settle(&session).await;
    assert_eq!(
        AgentStatus::Idle,
        snapshot.status,
        "{:?}",
        snapshot.last_error
    );
    assert_eq!(Some("native answer"), snapshot.last_output.as_deref());
    first.assert_hits_async(1).await;
    first.delete_async().await;
    let events_path = session.events_path().to_path_buf();
    let replay_before_flip = provider_replay_value(&EventStore::replay(&events_path).unwrap());
    assert_eq!(
        json!({"role":"assistant","content":[{"type":"text","text":"native answer"}]}),
        replay_before_flip
    );
    session.close().await.unwrap();
    let history_before_flip = std::fs::read(&events_path).unwrap();

    let reopened = Session::open(anthropic_session_options(
        &server,
        workspace.path(),
        data.path(),
        base_path,
        ProviderProfile::Compatible,
        Some(session_id),
    ))
    .await
    .unwrap();
    reopened
        .send(Input::new(
            "compatible_followup",
            "main",
            "compatible profile follow-up",
        ))
        .unwrap();
    let reopened_snapshot = settle(&reopened).await;
    assert_eq!(AgentStatus::Interrupted, reopened_snapshot.status);
    assert!(reopened_snapshot
        .last_error
        .as_deref()
        .is_some_and(|error| { error.contains("frozen provider route or capabilities") }));
    second.assert_hits_async(0).await;
    reopened.close().await.unwrap();

    let history_after_flip = std::fs::read(&events_path).unwrap();
    assert!(history_after_flip.starts_with(&history_before_flip));
    let events_after_flip = EventStore::replay(&events_path).unwrap();
    assert_eq!(
        replay_before_flip,
        provider_replay_value(&events_after_flip)
    );
    let bodies = common::take_http_requests(&endpoint);
    assert_eq!(1, bodies.len());
}

#[tokio::test]
async fn reopened_compatible_session_rejects_passive_native_profile_change() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let session_id = "compatible_to_native_reopen";
    let base_path = "/compatible-to-native-reopen";
    let endpoint = format!("{base_path}/v1/messages");
    let first = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(endpoint.as_str())
                .body_contains("compatible profile seed")
                .matches(common::capture_http_request);
            then.status(200).body(common::anthropic_sse_response(json!({
                "id": "compatible_unsigned",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type":"thinking","thinking":"compatible private reasoning"},
                    {"type":"text","text":"compatible answer"}
                ],
                "stop_reason": "end_turn"
            })));
        })
        .await;
    let should_not_dispatch = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(endpoint.as_str())
                .body_contains("native profile follow-up")
                .matches(common::capture_http_request);
            then.status(200).body(common::anthropic_sse_response(json!({
                "id": "unexpected_native_dispatch",
                "type": "message",
                "role": "assistant",
                "content": [{"type":"text","text":"should not dispatch"}],
                "stop_reason": "end_turn"
            })));
        })
        .await;

    let session = Session::open(anthropic_session_options(
        &server,
        workspace.path(),
        data.path(),
        base_path,
        ProviderProfile::Compatible,
        Some(session_id),
    ))
    .await
    .unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new(
            "compatible_seed",
            "main",
            "compatible profile seed",
        ))
        .unwrap();
    let snapshot = settle(&session).await;
    assert_eq!(
        AgentStatus::Idle,
        snapshot.status,
        "{:?}",
        snapshot.last_error
    );
    first.assert_hits_async(1).await;
    first.delete_async().await;
    let events_path = session.events_path().to_path_buf();
    let replay_before_flip = provider_replay_value(&EventStore::replay(&events_path).unwrap());
    assert_eq!(
        json!([
            {"type":"thinking","thinking":"compatible private reasoning"},
            {"type":"text","text":"compatible answer"}
        ]),
        replay_before_flip["content"]
    );
    session.close().await.unwrap();
    let history_before_flip = std::fs::read(&events_path).unwrap();

    let reopened = Session::open(anthropic_session_options(
        &server,
        workspace.path(),
        data.path(),
        base_path,
        ProviderProfile::Anthropic,
        Some(session_id),
    ))
    .await
    .unwrap();
    reopened
        .send(Input::new(
            "native_followup",
            "main",
            "native profile follow-up",
        ))
        .unwrap();
    let reopened_snapshot = settle(&reopened).await;
    assert_eq!(AgentStatus::Interrupted, reopened_snapshot.status);
    assert!(
        reopened_snapshot
            .last_error
            .as_deref()
            .is_some_and(|error| { error.contains("frozen provider route or capabilities") }),
        "unexpected error: {:?}",
        reopened_snapshot.last_error
    );
    should_not_dispatch.assert_hits_async(0).await;
    reopened.close().await.unwrap();

    let history_after_flip = std::fs::read(&events_path).unwrap();
    assert!(history_after_flip.starts_with(&history_before_flip));
    let events_after_flip = EventStore::replay(&events_path).unwrap();
    assert_eq!(
        replay_before_flip,
        provider_replay_value(&events_after_flip)
    );
    let bodies = common::take_http_requests(&endpoint);
    assert_eq!(1, bodies.len());
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
