use std::collections::VecDeque;
use std::time::Duration;

use kuku::event::{EventPayload, EventStore, ModelStopReason, StoredEvent};
use kuku::{AgentSnapshot, Session};

pub(super) async fn settled(session: &Session, address: &str) -> AgentSnapshot {
    settled_during(session, address, "session execution").await
}

pub(super) async fn settled_during(
    session: &Session,
    address: &str,
    stage: &'static str,
) -> AgentSnapshot {
    settled_with_stage_timeout(session, address, stage, Duration::from_secs(10)).await
}

pub(super) async fn settled_with_timeout(
    session: &Session,
    address: &str,
    timeout: Duration,
) -> AgentSnapshot {
    settled_with_stage_timeout(session, address, "session execution", timeout).await
}

async fn settled_with_stage_timeout(
    session: &Session,
    address: &str,
    stage: &'static str,
    timeout: Duration,
) -> AgentSnapshot {
    match wait_for_agent_with_timeout(session, address, stage, timeout).await {
        Ok(snapshot) => snapshot,
        Err(error) => panic!(
            "agent wait failed during {stage}: {}; {}",
            error_kind(&error),
            execution_wait_diagnostics(session, address)
        ),
    }
}

#[cfg(unix)]
pub(super) async fn wait_for_agent_during(
    session: &Session,
    address: &str,
    stage: &'static str,
) -> std::result::Result<AgentSnapshot, kuku::Error> {
    wait_for_agent_with_timeout(session, address, stage, Duration::from_secs(10)).await
}

async fn wait_for_agent_with_timeout(
    session: &Session,
    address: &str,
    stage: &'static str,
    timeout: Duration,
) -> std::result::Result<AgentSnapshot, kuku::Error> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!(
                "agent did not settle during {stage}; {}",
                execution_wait_diagnostics(session, address)
            );
        }
        match tokio::time::timeout(remaining, session.wait_for_agent(address)).await {
            Ok(Err(kuku::Error::AgentWaitTimeout)) => {}
            Ok(outcome) => return outcome,
            Err(_) => panic!(
                "agent did not settle during {stage}; {}",
                execution_wait_diagnostics(session, address)
            ),
        }
    }
}

fn execution_wait_diagnostics(session: &Session, address: &str) -> String {
    let view = session.agent_view(address);
    let cursor = view.as_ref().ok().map(|view| view.cursor);
    let agent = view.map(|view| view.snapshot);
    let snapshot = match agent {
        Ok(agent) => format!(
            "status={:?}, turn={:?}, execution_id={:?}, pending_inputs={}, model_requests={}, last_error_present={}",
            agent.status,
            agent.turn,
            agent.execution_id,
            agent.pending_inputs,
            agent.usage.model_requests,
            agent.last_error.is_some()
        ),
        Err(error) => format!("unavailable: {}", error_kind(&error)),
    };
    let mut phase = "unknown";
    let mut recent = VecDeque::new();
    let before = cursor.and_then(|cursor| cursor.checked_add(1));
    let journal_error = match EventStore::read_page_before(session.events_path(), before, 256) {
        Ok(events) => {
            for event in events {
                if let Some(latest) = execution_phase(&event.payload, address) {
                    phase = latest;
                    if recent.len() == 8 {
                        recent.pop_front();
                    }
                    recent.push_back(execution_marker(&event));
                }
            }
            None
        }
        Err(error) => Some(error_kind(&error)),
    };
    format!(
        "last recorded execution phase: {phase}; cursor={cursor:?}; agent: {snapshot}; recent committed event markers: {recent:?}; journal read error: {journal_error:?}"
    )
}

fn execution_marker(event: &StoredEvent) -> String {
    let kind = event.payload.kind_name();
    match &event.payload {
        EventPayload::ToolCall {
            tool, tool_call_id, ..
        } => format!(
            "{}:{kind} tool={} call={}",
            event.id,
            limited(tool, 64),
            limited(tool_call_id, 64)
        ),
        EventPayload::ToolResult {
            tool_call_id,
            status,
            ..
        } => format!(
            "{}:{kind} call={} status={}",
            event.id,
            limited(tool_call_id, 64),
            limited(status, 32)
        ),
        EventPayload::ModelResponse { stop_reason, .. } => {
            format!("{}:{kind} stop={stop_reason:?}", event.id)
        }
        EventPayload::ModelError {
            kind: error_kind, ..
        } => format!("{}:{kind} kind={}", event.id, limited(error_kind, 64)),
        _ => format!("{}:{kind}", event.id),
    }
}

fn error_kind(error: &kuku::Error) -> String {
    match error {
        kuku::Error::Io(error) => {
            format!("Io(kind={:?}, os={:?})", error.kind(), error.raw_os_error())
        }
        kuku::Error::Json(error) => format!("Json({:?})", error.classify()),
        kuku::Error::AgentWaitTimeout => "AgentWaitTimeout".into(),
        kuku::Error::InvalidArgument(_) => "InvalidArgument".into(),
        kuku::Error::InvalidEventStream(message) => match message.as_str() {
            "running agent has no active worker" => "InvalidEventStream(MissingWorker)".into(),
            "agent worker ended without publishing a completion result" => {
                "InvalidEventStream(MissingWorkerCompletion)".into()
            }
            _ => "InvalidEventStream".into(),
        },
        kuku::Error::Provider { kind, .. } => format!("Provider({kind:?})"),
        _ => "Other".into(),
    }
}

fn limited(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let mut result: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}

fn execution_phase(event: &EventPayload, address: &str) -> Option<&'static str> {
    match event {
        EventPayload::TurnCompleted { conversation, .. }
        | EventPayload::TurnCancelled { conversation, .. }
        | EventPayload::TurnInterrupted { conversation, .. }
            if conversation == address =>
        {
            Some("settlement")
        }
        EventPayload::TurnStarted { conversation, .. } if conversation == address => {
            Some("provider")
        }
        EventPayload::ModelRecovery { conversation, .. } if conversation == address => {
            Some("provider")
        }
        EventPayload::ToolCall { conversation, .. }
        | EventPayload::ToolResult { conversation, .. }
            if conversation.as_deref() == Some(address) =>
        {
            Some("tool")
        }
        EventPayload::ModelResponse {
            conversation,
            stop_reason: Some(ModelStopReason::EndTurn),
            ..
        } if conversation.as_deref() == Some(address) => Some("settlement"),
        EventPayload::ModelResponse {
            conversation,
            stop_reason: Some(ModelStopReason::ToolUse),
            ..
        } if conversation.as_deref() == Some(address) => Some("tool"),
        EventPayload::ModelResponse { conversation, .. }
        | EventPayload::ModelError { conversation, .. }
            if conversation.as_deref() == Some(address) =>
        {
            Some("provider")
        }
        _ => None,
    }
}

pub(super) async fn settled_with_output(
    session: &Session,
    address: &str,
    output: &str,
) -> AgentSnapshot {
    let snapshot = settled_with_timeout(session, address, Duration::from_secs(10)).await;
    assert_eq!(Some(output), snapshot.last_output.as_deref());
    snapshot
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::Notify;

    use kuku::{
        AdapterFuture, AgentConfig, AgentStatus, Input, ModelAdapter, ModelEvent, ModelRequest,
        ModelStream,
    };

    use super::{
        Duration, ModelStopReason, Session, error_kind, execution_wait_diagnostics,
        settled_with_stage_timeout,
    };

    #[derive(Debug, Default)]
    struct GatedModel {
        started: Notify,
        release: Notify,
        calls: AtomicUsize,
        failure: Option<String>,
    }

    impl ModelAdapter for GatedModel {
        fn stream(&self, _request: ModelRequest) -> AdapterFuture<'_, ModelStream> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.started.notify_one();
                self.release.notified().await;
                if let Some(failure) = &self.failure {
                    return Err(kuku::Error::InvalidEventStream(failure.clone()));
                }
                Ok(Box::pin(tokio_stream::iter([
                    Ok(ModelEvent::StreamStart {
                        request_id: "held-provider".into(),
                    }),
                    Ok(ModelEvent::TextDelta {
                        text: "released".into(),
                    }),
                    Ok(ModelEvent::StopReason {
                        reason: ModelStopReason::EndTurn,
                    }),
                    Ok(ModelEvent::StreamEnd),
                ])) as ModelStream)
            })
        }
    }

    #[tokio::test]
    async fn held_provider_timeout_reports_its_stage_without_cancelling_execution() {
        let server = httpmock::MockServer::start_async().await;
        let workspace = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let model = Arc::new(GatedModel::default());
        let mut options = super::super::options(&server, workspace.path(), data.path());
        options.model_adapter = Some(model.clone());
        let session = Session::open(options).await.unwrap();
        session
            .create_agent("create", "main", AgentConfig::default())
            .unwrap();
        session
            .send(Input::new("input", "main", "hold the provider"))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), model.started.notified())
            .await
            .unwrap();

        let waiting = session.clone();
        let wait = tokio::spawn(async move {
            settled_with_stage_timeout(&waiting, "main", "held provider", Duration::from_millis(25))
                .await
        })
        .await;
        model.release.notify_one();
        let completed =
            tokio::time::timeout(Duration::from_secs(2), session.wait_for_agent("main"))
                .await
                .unwrap()
                .unwrap();
        session.close().await.unwrap();

        let panic = wait
            .expect_err("held provider unexpectedly settled")
            .into_panic();
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap();
        assert!(message.contains("during held provider"), "{message}");
        assert!(
            message.contains("last recorded execution phase: provider"),
            "{message}"
        );
        assert!(message.contains("status=Running"), "{message}");
        assert!(message.contains("cursor=Some("), "{message}");
        assert!(message.contains("turn.started"), "{message}");
        assert_eq!(AgentStatus::Idle, completed.status);
        assert_eq!(Some("released"), completed.last_output.as_deref());
        assert_eq!(1, completed.usage.model_requests);
        assert_eq!(1, model.calls.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn diagnostics_keep_error_kinds_without_freeform_payloads() {
        const SENTINEL: &str = "private-provider-response-sentinel";
        let server = httpmock::MockServer::start_async().await;
        let workspace = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let model = Arc::new(GatedModel {
            failure: Some(SENTINEL.into()),
            ..GatedModel::default()
        });
        let mut options = super::super::options(&server, workspace.path(), data.path());
        options.model_adapter = Some(model.clone());
        let session = Session::open(options).await.unwrap();
        session
            .create_agent("create", "main", AgentConfig::default())
            .unwrap();
        session
            .send(Input::new("input", "main", "fail the provider"))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), model.started.notified())
            .await
            .unwrap();
        model.release.notify_one();
        let completed =
            tokio::time::timeout(Duration::from_secs(2), session.wait_for_agent("main"))
                .await
                .unwrap()
                .unwrap();
        let diagnostics = execution_wait_diagnostics(&session, "main");
        session.close().await.unwrap();

        assert_eq!(AgentStatus::Interrupted, completed.status);
        assert!(completed.last_error.as_deref().unwrap().contains(SENTINEL));
        assert!(
            diagnostics.contains("last_error_present=true"),
            "{diagnostics}"
        );
        assert!(diagnostics.contains("model.error kind="), "{diagnostics}");
        assert!(!diagnostics.contains(SENTINEL), "{diagnostics}");
        let typed = error_kind(&kuku::Error::InvalidEventStream(SENTINEL.into()));
        assert_eq!("InvalidEventStream", typed);
        assert!(!typed.contains(SENTINEL));
        assert_eq!(
            "InvalidEventStream(MissingWorkerCompletion)",
            error_kind(&kuku::Error::InvalidEventStream(
                "agent worker ended without publishing a completion result".into()
            ))
        );
    }
}
