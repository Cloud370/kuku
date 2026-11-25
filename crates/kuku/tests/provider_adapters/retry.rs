use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use httpmock::prelude::{MockServer, POST};
use serde_json::{Value, json};

use super::common;
use kuku::config::{ProviderFormat, ProviderProfile, ThinkLevel};
use kuku::event::{EventPayload, EventStore};
use kuku::{
    AdapterFuture, AgentConfig, AgentStatus, Error, Input, ModelAdapter, ModelRequest, ModelStream,
    ProviderFailureKind, Session, SessionOptions,
};

type AttemptRecord = (String, Vec<u8>);
type RecordedAttempts = Arc<Mutex<Vec<AttemptRecord>>>;

#[derive(Debug)]
struct AlwaysTransientModel(RecordedAttempts);

impl ModelAdapter for AlwaysTransientModel {
    fn stream(&self, request: ModelRequest) -> AdapterFuture<'_, ModelStream> {
        let attempts = self.0.clone();
        Box::pin(async move {
            attempts.lock().unwrap().push((
                request.request_id,
                serde_json::to_vec(&request.body).unwrap(),
            ));
            Err(Error::Provider {
                kind: ProviderFailureKind::RateLimited,
                message: "transient fixture failure".into(),
                provider: None,
                model: None,
            })
        })
    }
}

fn openai_options(
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
    let model_settings =
        common::set_test_model(&mut config, "openai", "deepseek/deepseek-v4.1-flash-1017");
    model_settings.think = ThinkLevel::Off;
    config.handoff.enabled = false;
    config.plugin.enabled = false;
    SessionOptions::new(workspace, data, config)
}

fn anthropic_options(server: &MockServer, workspace: &Path, data: &Path) -> SessionOptions {
    let mut config = common::test_config();
    let provider = config.providers.get_mut("anthropic").unwrap();
    provider.base_url = server.url("/retry-stream");
    provider.options.profile = ProviderProfile::Anthropic;
    config.handoff.enabled = false;
    config.plugin.enabled = false;
    SessionOptions::new(workspace, data, config)
}

async fn open_session(options: SessionOptions) -> Session {
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
}

async fn settle(session: &Session) -> kuku::AgentSnapshot {
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
    .expect("provider fixture did not settle")
}

async fn wait_for_retry_phase(session: &Session, phase: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(value) = EventStore::replay(session.events_path())
                .unwrap()
                .iter()
                .find_map(|event| match &event.payload {
                    EventPayload::Unknown(value)
                        if value["kind"] == "runtime.model_retry" && value["phase"] == phase =>
                    {
                        Some(value.clone())
                    }
                    _ => None,
                })
            {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("retry phase was not persisted")
}

fn retry_events(session: &Session) -> Vec<Value> {
    EventStore::replay(session.events_path())
        .unwrap()
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::Unknown(value) if value["kind"] == "runtime.model_retry" => {
                Some(value.clone())
            }
            _ => None,
        })
        .collect()
}

async fn wait_for_retry_phase_count(session: &Session, phase: &str, count: usize) -> Vec<Value> {
    for _ in 0..100 {
        let events = retry_events(session);
        if events
            .iter()
            .filter(|event| event["phase"] == phase)
            .count()
            >= count
        {
            return events;
        }
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
    }
    panic!("expected retry phase was not persisted");
}

async fn wait_for_model_attempts(attempts: &RecordedAttempts, count: usize) {
    for _ in 0..100 {
        if attempts.lock().unwrap().len() >= count {
            return;
        }
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
    }
    panic!("expected provider attempt was not dispatched");
}

#[tokio::test(start_paused = true)]
async fn actual_provider_retries_stop_after_six_total_attempts() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let attempts = Arc::new(Mutex::new(Vec::<AttemptRecord>::new()));
    let mut options = SessionOptions::new(workspace.path(), data.path(), common::test_config());
    options.model_adapter = Some(Arc::new(AlwaysTransientModel(attempts.clone())));
    options.limits.max_requests_per_open = Some(7);
    let session = Session::open(options).await.unwrap();
    // This paused-clock fixture controls retry sleeps, not on-disk Skill extraction.
    session
        .create_agent(
            "create",
            "main",
            AgentConfig {
                no_skills: true,
                ..AgentConfig::default()
            },
        )
        .unwrap();
    session
        .send(Input::new("six-attempts", "main", "keep request unchanged"))
        .unwrap();

    for failed_attempt in 1..=6 {
        wait_for_model_attempts(&attempts, failed_attempt).await;
        if failed_attempt < 6 {
            let events = wait_for_retry_phase_count(&session, "waiting", failed_attempt).await;
            let waiting = events
                .iter()
                .filter(|event| event["phase"] == "waiting")
                .nth(failed_attempt - 1)
                .unwrap();
            assert_eq!(failed_attempt + 1, waiting["attempt"]);
            assert_eq!(failed_attempt, waiting["failed_attempt"]);
            let delay_ms = waiting["delay_ms"].as_u64().unwrap();
            let base_ms = [2_000, 4_000, 8_000, 16_000, 32_000][failed_attempt - 1];
            assert!((base_ms..=base_ms * 5 / 4).contains(&delay_ms));
            tokio::time::advance(Duration::from_millis(delay_ms + 1)).await;
            tokio::task::yield_now().await;
        }
    }

    wait_for_retry_phase_count(&session, "stopped", 1).await;
    let final_snapshot = settle(&session).await;
    assert_eq!(AgentStatus::Interrupted, final_snapshot.status);
    {
        let recorded = attempts.lock().unwrap();
        assert_eq!(6, recorded.len());
        assert!(
            recorded.iter().all(|(request_id, body)| {
                request_id == &recorded[0].0 && body == &recorded[0].1
            })
        );
    }

    assert_eq!(6, session.agent("main").unwrap().usage.model_requests);
    let events = retry_events(&session);
    assert_eq!(11, events.len());
    for (attempt_index, pair) in events[..10].as_chunks::<2>().0.iter().enumerate() {
        let upcoming_attempt = attempt_index + 2;
        assert_eq!("waiting", pair[0]["phase"]);
        assert_eq!("started", pair[1]["phase"]);
        for event in pair {
            assert_eq!(upcoming_attempt, event["attempt"]);
            assert_eq!(upcoming_attempt - 1, event["failed_attempt"]);
            assert_eq!("rate_limited", event["failed_kind"]);
            assert_eq!(true, event["will_retry"]);
        }
        assert_eq!(0, pair[1]["delay_ms"]);
    }
    let stopped = events.last().unwrap();
    assert_eq!("stopped", stopped["phase"]);
    assert_eq!(6, stopped["attempt"]);
    assert_eq!(6, stopped["failed_attempt"]);
    assert_eq!(6, stopped["max_attempts"]);
    assert_eq!("attempt_limit", stopped["stop_reason"]);
    assert_eq!(false, stopped["will_retry"]);
    assert_eq!(0, stopped["delay_ms"]);
    session.close().await.unwrap();
}

#[tokio::test]
async fn anthropic_first_overload_error_retries_before_any_stream_effect() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let path = "/retry-stream/v1/messages";
    let first = server
        .mock_async(|when, then| {
            when.method(POST).path(path);
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(concat!(
                    "event: error\n",
                    r#"data: {"type":"error","error":{"type":"overloaded_error","message":"overloaded \u001b[31mprivate-token"}}"#,
                    "\n\n"
                ));
        })
        .await;
    let session = open_session(anthropic_options(&server, workspace.path(), data.path())).await;
    session
        .send(Input::new("retry", "main", "retry on initial overload"))
        .unwrap();

    let waiting = wait_for_retry_phase(&session, "waiting").await;
    assert_eq!("provider_unavailable", waiting["failed_kind"]);
    assert_eq!(1, waiting["failed_attempt"]);
    assert_eq!(2, waiting["attempt"]);
    assert_eq!(6, waiting["max_attempts"]);
    assert_eq!(true, waiting["will_retry"]);
    assert_eq!(Value::Null, waiting["status"]);
    assert_eq!(Value::Null, waiting["stop_reason"]);
    assert_eq!("provider unavailable", waiting["message"]);
    assert!(
        waiting["delay_ms"]
            .as_u64()
            .is_some_and(|delay| (2_000..=2_500).contains(&delay))
    );
    first.assert_hits_async(1).await;
    first.delete_async().await;
    let retry = server
        .mock_async(|when, then| {
            when.method(POST).path(path);
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(common::anthropic_sse_response(json!({
                    "id":"retry_success","type":"message","role":"assistant",
                    "content":[{"type":"text","text":"retried once"}],"stop_reason":"end_turn"
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
    assert_eq!(Some("retried once"), snapshot.last_output.as_deref());
    assert_eq!(2, snapshot.usage.model_requests);
    retry.assert_hits_async(1).await;
    let events = retry_events(&session);
    assert_eq!(2, events.len());
    assert_eq!("waiting", events[0]["phase"]);
    assert_eq!("started", events[1]["phase"]);
    assert_eq!(2, events[1]["attempt"]);
    assert_eq!(1, events[1]["failed_attempt"]);
    assert_eq!(0, events[1]["delay_ms"]);
    session.close().await.unwrap();
}

#[tokio::test]
async fn response_failed_with_usage_is_preflighted_as_rate_limit() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let first = server
        .mock_async(|when, then| {
            when.method(POST).path("/responses");
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(concat!(
                    "event: response.failed\n",
                    r#"data: {"type":"response.failed","response":{"id":"failed_with_usage","status":"failed","usage":{"input_tokens":12,"output_tokens":0},"error":{"code":"rate_limit_error","message":"retry later"}}}"#,
                    "\n\n"
                ));
        })
        .await;
    let session = open_session(openai_options(
        &server,
        workspace.path(),
        data.path(),
        ProviderFormat::OpenAiResponses,
    ))
    .await;
    session
        .send(Input::new("retry", "main", "retry response failure"))
        .unwrap();
    let waiting = wait_for_retry_phase(&session, "waiting").await;
    assert_eq!("rate_limited", waiting["failed_kind"]);
    assert_eq!(Value::Null, waiting["status"]);
    first.assert_hits_async(1).await;
    first.delete_async().await;
    let retry = server
        .mock_async(|when, then| {
            when.method(POST).path("/responses");
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(concat!(
                    "event: response.created\ndata: {\"response\":{\"id\":\"retry\",\"status\":\"in_progress\"}}\n\n",
                    "event: response.output_text.delta\ndata: {\"delta\":\"response retried\"}\n\n",
                    "event: response.completed\ndata: {\"response\":{\"id\":\"retry\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":12,\"output_tokens\":2}}}\n\n",
                    "data: [DONE]\n\n"
                ));
        })
        .await;
    let snapshot = settle(&session).await;
    assert_eq!(
        AgentStatus::Idle,
        snapshot.status,
        "{:?}",
        snapshot.last_error
    );
    assert_eq!(Some("response retried"), snapshot.last_output.as_deref());
    retry.assert_hits_async(1).await;
    session.close().await.unwrap();
}

#[tokio::test]
async fn chat_code_only_quota_error_is_not_retried() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let quota = server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200)
                .header("content-type", "text/event-stream")
                .body("data: {\"error\":{\"code\":\"insufficient_quota\",\"message\":\"quota unavailable\"}}\n\n");
        })
        .await;
    let session = open_session(openai_options(
        &server,
        workspace.path(),
        data.path(),
        ProviderFormat::OpenAiChat,
    ))
    .await;
    session
        .send(Input::new("quota", "main", "do not retry quota"))
        .unwrap();
    let stopped = wait_for_retry_phase(&session, "stopped").await;
    assert_eq!("quota_exhausted", stopped["failed_kind"]);
    assert_eq!("non_retryable", stopped["stop_reason"]);
    assert_eq!(false, stopped["will_retry"]);
    assert_eq!(1, stopped["attempt"]);
    let snapshot = settle(&session).await;
    assert_eq!(AgentStatus::Interrupted, snapshot.status);
    quota.assert_hits_async(1).await;
    assert_eq!(1, retry_events(&session).len());
    session.close().await.unwrap();
}

#[tokio::test]
async fn partial_stream_error_stops_with_stream_started_and_no_redispatch() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let partial = server
        .mock_async(|when, then| {
            when.method(POST).path("/retry-stream/v1/messages");
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(concat!(
                    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"partial\",\"usage\":{\"input_tokens\":1}}}\n\n",
                    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial answer\"}}\n\n",
                    "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"after output\"}}\n\n"
                ));
        })
        .await;
    let session = open_session(anthropic_options(&server, workspace.path(), data.path())).await;
    session
        .send(Input::new(
            "partial",
            "main",
            "do not repeat partial output",
        ))
        .unwrap();
    let stopped = wait_for_retry_phase(&session, "stopped").await;
    assert_eq!("provider_unavailable", stopped["failed_kind"]);
    assert_eq!("stream_started", stopped["stop_reason"]);
    assert_eq!(1, stopped["attempt"]);
    assert_eq!(false, stopped["will_retry"]);
    let snapshot = settle(&session).await;
    assert_eq!(AgentStatus::Interrupted, snapshot.status);
    partial.assert_hits_async(1).await;
    let events = EventStore::replay(session.events_path()).unwrap();
    assert!(!events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolCall { .. } | EventPayload::ToolResult { .. }
    )));
    session.close().await.unwrap();
}

#[tokio::test]
async fn cancellation_during_retry_wait_does_not_start_or_count_an_attempt() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let first = server
        .mock_async(|when, then| {
            when.method(POST).path("/retry-stream/v1/messages");
            then.status(503)
                .header("retry-after", "30")
                .body("temporarily unavailable");
        })
        .await;
    let session = open_session(anthropic_options(&server, workspace.path(), data.path())).await;
    session
        .send(Input::new("cancel", "main", "cancel retry wait"))
        .unwrap();
    let waiting = wait_for_retry_phase(&session, "waiting").await;
    assert_eq!(30_000, waiting["delay_ms"]);
    let execution = session.agent("main").unwrap().execution_id.unwrap();
    session
        .cancel_execution("cancel-retry", "main", execution)
        .await
        .unwrap();
    first.assert_hits_async(1).await;
    assert_eq!(AgentStatus::Stopped, session.agent("main").unwrap().status);
    assert_eq!(1, session.agent("main").unwrap().usage.model_requests);
    assert_eq!(
        vec!["waiting"],
        retry_events(&session)
            .iter()
            .map(|event| event["phase"].as_str().unwrap())
            .collect::<Vec<_>>()
    );
    session.close().await.unwrap();
}
