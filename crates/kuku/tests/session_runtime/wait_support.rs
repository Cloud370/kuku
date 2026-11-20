use std::time::Duration;

use kuku::event::{EventPayload, ModelStopReason};
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
        Err(_) => panic!(
            "agent wait failed during {stage}; last recorded execution phase: {}",
            last_recorded_execution_phase(session, address)
        ),
    }
}

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
                "agent did not settle during {stage}; last recorded execution phase: {}",
                last_recorded_execution_phase(session, address)
            );
        }
        match tokio::time::timeout(remaining, session.wait_for_agent(address)).await {
            Ok(Err(kuku::Error::AgentWaitTimeout)) => {}
            Ok(outcome) => return outcome,
            Err(_) => panic!(
                "agent did not settle during {stage}; last recorded execution phase: {}",
                last_recorded_execution_phase(session, address)
            ),
        }
    }
}

fn last_recorded_execution_phase(session: &Session, address: &str) -> &'static str {
    let mut after = 0;
    let mut phase = None;
    loop {
        let Ok(events) = session.events(after, 256) else {
            return "unknown";
        };
        if events.is_empty() {
            return phase.unwrap_or("unknown");
        }
        for event in events {
            if let Some(latest) = execution_phase(&event.payload, address) {
                phase = Some(latest);
            }
            after = event.id;
        }
    }
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
