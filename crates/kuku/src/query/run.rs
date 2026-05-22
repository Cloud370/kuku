use std::collections::VecDeque;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::permission::append_project_allow_rule;
use crate::provider::chunk::ProviderChunk;
use crate::provider::types::ProviderToolCall;
use crate::tool::ToolDefinition;

use super::helpers::{
    append_permission_decision, execute_tool_call, permission_candidate, permission_rule,
};
use super::types::{
    PendingRun, PendingStep, PermissionChoice, Run, RunState, StreamingChunkState, UiEvent,
};

impl Drop for Run {
    fn drop(&mut self) {
        crate::session::release_lock(&self.lock_path);
    }
}

impl Run {
    /// The session ID for this run.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The workspace directory for this run.
    pub fn workspace(&self) -> &std::path::Path {
        match &self.state {
            RunState::Pending(p) => &p.workspace,
            RunState::Streaming(s) => &s.pending.workspace,
            RunState::WaitingForPermission(w) => &w.pending.workspace,
            RunState::BatchEvents(p, _) => &p.workspace,
            RunState::InSubexec { pending, .. } => &pending.workspace,
            RunState::Cancelled { .. } | RunState::Done(_) => std::path::Path::new(""),
        }
    }

    /// A token that is notified when the run is cancelled.
    pub fn cancel_token(&self) -> Arc<tokio::sync::Notify> {
        self.cancel_token.clone()
    }

    /// Cancel the current run. Streaming is aborted, pending permissions are denied,
    /// and the cancelled model.response enters history.
    pub fn cancel(&mut self) {
        if let RunState::InSubexec { ref mut child_run, .. } = self.state {
            child_run.cancel();
        }
        let (events_path, turn) = match &self.state {
            RunState::Pending(p) => (p.events_path.clone(), p.turn),
            RunState::Streaming(s) => (s.pending.events_path.clone(), s.pending.turn),
            RunState::WaitingForPermission(w) => (w.pending.events_path.clone(), w.pending.turn),
            RunState::BatchEvents(p, _) => (p.events_path.clone(), p.turn),
            RunState::InSubexec { pending, .. } => (pending.events_path.clone(), pending.turn),
            RunState::Cancelled { .. } | RunState::Done(_) => return,
        };
        self.state = RunState::Cancelled { events_path, turn };
        self.cancel_token.notify_waiters();
    }

    /// Poll for the next UI event from the running query.
    pub async fn next(&mut self) -> Result<Option<UiEvent>> {
        loop {
            match std::mem::replace(&mut self.state, RunState::Done(None)) {
                RunState::Pending(pending) => match super::step::advance_pending(*pending).await? {
                    PendingStep::Pending(next_pending) => {
                        self.state = RunState::Pending(next_pending);
                    }
                    PendingStep::NeedPermission(waiting) => {
                        let request = waiting.request.clone();
                        self.state = RunState::WaitingForPermission(waiting);
                        return Ok(Some(UiEvent::PermissionRequested { request }));
                    }
                    PendingStep::Streaming(streaming) => {
                        self.state = RunState::Streaming(streaming);
                    }
                    PendingStep::ToolResultReady { pending, ui_event } => {
                        self.state = RunState::Pending(pending);
                        return Ok(Some(ui_event));
                    }
                    PendingStep::BatchReady { pending, ui_events } => {
                        let mut events = VecDeque::from(ui_events);
                        let first = events.pop_front().unwrap();
                        if events.is_empty() {
                            self.state = RunState::Pending(pending);
                        } else {
                            self.state = RunState::BatchEvents(pending, events);
                        }
                        return Ok(Some(first));
                    }
                    PendingStep::InSubexec {
                        pending,
                        stage_id,
                        kind,
                        child_run,
                        label,
                        tool_call_id,
                        agent_name,
                    } => {
                        let subexec_label = label.clone();
                        self.state = RunState::InSubexec {
                            pending,
                            stage_id: stage_id.clone(),
                            kind: kind.clone(),
                            child_run,
                            label,
                            tool_call_id,
                            agent_name,
                        };
                        return Ok(Some(UiEvent::SubexecStart {
                            stage_id,
                            kind,
                            label: subexec_label,
                        }));
                    }
                    PendingStep::Done(output, usage, turn) => {
                        self.state = RunState::Done(None);
                        return Ok(Some(UiEvent::Done {
                            output,
                            usage,
                            turn,
                        }));
                    }
                },
                RunState::InSubexec {
                    pending,
                    stage_id,
                    kind,
                    mut child_run,
                    label,
                    tool_call_id,
                    agent_name,
                } => {
                    let cancel_token = self.cancel_token.clone();
                    let child_result = tokio::select! {
                        result = Box::pin(child_run.next()) => result,
                        _ = cancel_token.notified() => {
                            child_run.cancel();
                            let summary = format!("{} cancelled", &agent_name);
                            {
                                let mut store = crate::event::EventStore::open(&pending.events_path)?;
                                store.append(crate::event::EventPayload::ToolResult {
                                    turn: pending.turn,
                                    ts: super::helpers::now_timestamp()?,
                                    tool_call_id: tool_call_id.clone(),
                                    status: "cancelled".to_string(),
                                    summary: summary.clone(),
                                    model_content: String::new(),
                                    truncated: false,
                                    structured: None,
                                })?;
                            }
                            self.state = RunState::Pending(pending);
                            return Ok(Some(UiEvent::SubexecEnd {
                                stage_id,
                                status: "cancelled".to_string(),
                                summary,
                                result: None,
                            }));
                        }
                    };
                    match child_result {
                        Err(e) => {
                            let summary = format!("{} error: {e}", &agent_name);
                            {
                                let mut store = crate::event::EventStore::open(&pending.events_path)?;
                                store.append(crate::event::EventPayload::ToolResult {
                                    turn: pending.turn,
                                    ts: super::helpers::now_timestamp()?,
                                    tool_call_id: tool_call_id.clone(),
                                    status: "error".to_string(),
                                    summary: summary.clone(),
                                    model_content: String::new(),
                                    truncated: false,
                                    structured: None,
                                })?;
                            }
                            self.state = RunState::Pending(pending);
                            return Ok(Some(UiEvent::SubexecEnd {
                                stage_id,
                                status: "error".to_string(),
                                summary,
                                result: None,
                            }));
                        }
                        Ok(Some(UiEvent::Done { output, .. })) => {
                            let summary = format!("{} completed in {} turns", &agent_name, output.turn);
                            let structured = Some(serde_json::json!({
                                "kind": "subagent_result",
                                "child_session_id": stage_id,
                                "turns_completed": output.turn,
                            }));
                            {
                                let mut store = crate::event::EventStore::open(&pending.events_path)?;
                                store.append(crate::event::EventPayload::ToolResult {
                                    turn: pending.turn,
                                    ts: super::helpers::now_timestamp()?,
                                    tool_call_id: tool_call_id.clone(),
                                    status: "ok".to_string(),
                                    summary: summary.clone(),
                                    model_content: output.text,
                                    truncated: false,
                                    structured: structured.clone(),
                                })?;
                            }
                            self.state = RunState::Pending(pending);
                            return Ok(Some(UiEvent::SubexecEnd {
                                stage_id: stage_id.clone(),
                                status: "ok".to_string(),
                                summary,
                                result: structured,
                            }));
                        }
                        Ok(Some(child_event)) => {
                            let subexec_event = map_child_to_subexec_event(child_event);
                            self.state = RunState::InSubexec {
                                pending,
                                stage_id: stage_id.clone(),
                                kind,
                                child_run,
                                label,
                                tool_call_id,
                                agent_name,
                            };
                            return Ok(Some(UiEvent::SubexecOutput {
                                stage_id,
                                event: subexec_event,
                            }));
                        }
                        Ok(None) => {
                            let summary = format!("{} error: stream ended unexpectedly", &agent_name);
                            {
                                let mut store = crate::event::EventStore::open(&pending.events_path)?;
                                store.append(crate::event::EventPayload::ToolResult {
                                    turn: pending.turn,
                                    ts: super::helpers::now_timestamp()?,
                                    tool_call_id: tool_call_id.clone(),
                                    status: "error".to_string(),
                                    summary: summary.clone(),
                                    model_content: String::new(),
                                    truncated: false,
                                    structured: None,
                                })?;
                            }
                            self.state = RunState::Pending(pending);
                            return Ok(Some(UiEvent::SubexecEnd {
                                stage_id,
                                status: "error".to_string(),
                                summary,
                                result: None,
                            }));
                        }
                    }
                }
                RunState::Streaming(mut streaming) => {
                    if let Some(event) = streaming.lead_events.pop() {
                        self.state = RunState::Streaming(streaming);
                        return Ok(Some(event));
                    }
                    match self.poll_stream_chunk(&mut streaming).await? {
                        Some(event) => {
                            self.state = RunState::Streaming(streaming);
                            return Ok(Some(event));
                        }
                        None => {
                            let step = super::step::finish_streaming(*streaming).await?;
                            match step {
                                PendingStep::Pending(next_pending) => {
                                    self.state = RunState::Pending(next_pending);
                                }
                                PendingStep::Done(output, usage, turn) => {
                                    self.state = RunState::Done(None);
                                    return Ok(Some(UiEvent::Done {
                                        output,
                                        usage,
                                        turn,
                                    }));
                                }
                                _ => {
                                    self.state = RunState::Done(None);
                                }
                            }
                        }
                    }
                }
                RunState::WaitingForPermission(waiting) => {
                    let request = waiting.request.clone();
                    self.state = RunState::WaitingForPermission(waiting);
                    return Ok(Some(UiEvent::PermissionRequested { request }));
                }
                RunState::BatchEvents(pending, mut events) => {
                    let event = events.pop_front().unwrap();
                    if events.is_empty() {
                        self.state = RunState::Pending(pending);
                    } else {
                        self.state = RunState::BatchEvents(pending, events);
                    }
                    return Ok(Some(event));
                }
                RunState::Cancelled { events_path, turn } => {
                    let mut store = crate::event::EventStore::open(&events_path)?;
                    store.append(crate::event::EventPayload::TurnEnd {
                        turn,
                        ts: super::helpers::now_timestamp()?,
                    })?;
                    self.state = RunState::Done(None);
                    return Ok(None);
                }
                RunState::Done(Some((output, usage, turn))) => {
                    self.state = RunState::Done(None);
                    return Ok(Some(UiEvent::Done {
                        output,
                        usage,
                        turn,
                    }));
                }
                RunState::Done(None) => {
                    self.state = RunState::Done(None);
                    return Ok(None);
                }
            }
        }
    }

    async fn poll_stream_chunk(
        &self,
        streaming: &mut StreamingChunkState,
    ) -> Result<Option<UiEvent>> {
        use tokio_stream::StreamExt;
        loop {
            let chunk = tokio::select! {
                chunk = streaming.stream.next() => match chunk {
                    Some(Ok(chunk)) => chunk,
                    Some(Err(_failure)) => return Ok(None),
                    None => return Ok(None),
                },
                _ = self.cancel_token.notified() => {
                    streaming.stop_reason = Some("cancelled".to_string());
                    return Ok(None);
                }
            };

            match chunk {
                ProviderChunk::StreamStart { request_id: rid } => {
                    streaming.provider_request_id = Some(rid);
                }
                ProviderChunk::TextDelta { text } => {
                    streaming.accumulated_text.push_str(&text);
                    return Ok(Some(UiEvent::TextDelta { text }));
                }
                ProviderChunk::ThinkingDelta { text } => {
                    streaming.accumulated_thinking.push_str(&text);
                    return Ok(Some(UiEvent::ThinkingDelta { text }));
                }
                ProviderChunk::ToolCallStart { index, id, name } => {
                    streaming.tool_calls.push(ProviderToolCall {
                        id,
                        name,
                        args: serde_json::json!({}),
                        index,
                    });
                    streaming.tool_arg_buffers.push((index, String::new()));
                }
                ProviderChunk::ToolCallArgDelta { index, fragment } => {
                    if let Some((_, buf)) = streaming
                        .tool_arg_buffers
                        .iter_mut()
                        .find(|(i, _)| *i == index)
                    {
                        buf.push_str(&fragment);
                    }
                }
                ProviderChunk::ContentBlockStop { index } => {
                    if let Some((_, buf)) =
                        streaming.tool_arg_buffers.iter().find(|(i, _)| *i == index)
                    {
                        if let Ok(args) = serde_json::from_str::<serde_json::Value>(buf) {
                            if let Some(tc) =
                                streaming.tool_calls.iter_mut().find(|t| t.index == index)
                            {
                                tc.args = args;
                            }
                        }
                    }
                }
                ProviderChunk::StopReason { reason } => {
                    streaming.stop_reason = Some(reason);
                }
                ProviderChunk::StreamUsage {
                    input_tokens,
                    output_tokens,
                    cache_read_input_tokens,
                    cache_creation_input_tokens,
                } => {
                    let entry =
                        streaming
                            .usage
                            .get_or_insert(crate::provider::types::ProviderUsage {
                                input_tokens: Some(0),
                                output_tokens: Some(0),
                                cache_read_input_tokens: Some(0),
                                cache_creation_input_tokens: Some(0),
                            });
                    entry.input_tokens = Some(entry.input_tokens.unwrap_or(0) + input_tokens);
                    entry.output_tokens = Some(entry.output_tokens.unwrap_or(0) + output_tokens);
                    entry.cache_read_input_tokens =
                        Some(entry.cache_read_input_tokens.unwrap_or(0) + cache_read_input_tokens);
                    entry.cache_creation_input_tokens = Some(
                        entry.cache_creation_input_tokens.unwrap_or(0)
                            + cache_creation_input_tokens,
                    );
                }
                ProviderChunk::StreamEnd => {}
            }
        }
    }

    /// Apply a permission decision for a pending tool call.
    pub async fn decide(
        &mut self,
        request_id: impl AsRef<str>,
        choice: PermissionChoice,
    ) -> Result<Option<UiEvent>> {
        self.apply_choice(request_id.as_ref(), choice, "host").await
    }

    pub(super) async fn deny_pending(&mut self) -> Result<Option<UiEvent>> {
        let request_id = match &self.state {
            RunState::WaitingForPermission(waiting) => waiting.request.id.clone(),
            _ => {
                return Err(Error::PermissionRequestNotPending(
                    "no permission request is pending".to_string(),
                ))
            }
        };
        self.apply_choice(&request_id, PermissionChoice::Deny, "runtime")
            .await
    }

    async fn apply_choice(
        &mut self,
        request_id: &str,
        choice: PermissionChoice,
        source: &str,
    ) -> Result<Option<UiEvent>> {
        let state = std::mem::replace(&mut self.state, RunState::Done(None));
        let waiting = match state {
            RunState::WaitingForPermission(waiting) if waiting.request.id == request_id => *waiting,
            other => {
                self.state = other;
                return Err(Error::PermissionRequestNotPending(request_id.to_string()));
            }
        };

        let mut pending = waiting.pending;
        let rule = permission_rule(
            &pending.kuku_home,
            &pending.workspace,
            &waiting.queued_tool_call.tool_call.name,
            &waiting.queued_tool_call.tool_call.args,
        );
        if matches!(choice, PermissionChoice::Project) {
            append_project_allow_rule(
                &pending.policy_path,
                &waiting.queued_tool_call.tool_call.name,
                &permission_candidate(
                    &pending.kuku_home,
                    &pending.workspace,
                    &waiting.queued_tool_call.tool_call.name,
                    &waiting.queued_tool_call.tool_call.args,
                ),
            )?;
        }
        append_permission_decision(
            &pending.events_path,
            pending.turn,
            &waiting.queued_tool_call.tool_call.id,
            choice,
            source,
            &rule,
        )?;
        let result = execute_tool_call(&mut pending, &waiting.queued_tool_call.tool_call).await?;
        let tool_result_event = Some(UiEvent::ToolResult {
            tool_call_id: waiting.queued_tool_call.tool_call.id.clone(),
            name: waiting.queued_tool_call.tool_call.name.clone(),
            status: result.status,
            summary: result.summary,
            structured: result.structured,
        });
        self.state = RunState::Pending(Box::new(pending));
        Ok(tool_result_event)
    }
}

pub(super) fn find_tool_definition<'a>(
    pending: &'a PendingRun,
    name: &str,
) -> Option<&'a ToolDefinition> {
    pending
        .resolved
        .as_ref()
        .and_then(|resolved| resolved.registry.iter().find(|tool| tool.name == name))
}

fn map_child_to_subexec_event(event: UiEvent) -> crate::query::types::SubexecEvent {
    match event {
        UiEvent::TextDelta { text } => crate::query::types::SubexecEvent::TextDelta { text },
        UiEvent::ThinkingDelta { text } => crate::query::types::SubexecEvent::ThinkingDelta { text },
        UiEvent::ToolCall {
            tool_call_id,
            tool,
            summary,
        } => crate::query::types::SubexecEvent::ToolCall {
            tool_call_id,
            tool,
            summary,
        },
        UiEvent::ToolResult {
            tool_call_id,
            name,
            status,
            summary,
            ..
        } => crate::query::types::SubexecEvent::ToolResult {
            tool_call_id,
            name,
            status,
            summary,
        },
        UiEvent::Error { code, message } => {
            crate::query::types::SubexecEvent::Stderr(format!("[{code}] {message}"))
        }
        UiEvent::PermissionRequested { request } => {
            crate::query::types::SubexecEvent::Stderr(format!(
                "[unexpected] permission requested in subexec: {} ({})",
                request.tool, request.summary
            ))
        }
        UiEvent::Done { .. } => {
            crate::query::types::SubexecEvent::Stderr("[unexpected] Done in subexec map".into())
        }
        UiEvent::SubexecStart { .. }
        | UiEvent::SubexecOutput { .. }
        | UiEvent::SubexecEnd { .. } => {
            crate::query::types::SubexecEvent::Stderr("[unexpected] nested subexec".into())
        }
        UiEvent::TurnStart { .. } | UiEvent::ModelRequest { .. } => {
            crate::query::types::SubexecEvent::TextDelta {
                text: String::new(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventPayload, EventStore};

    fn test_config() -> crate::config::Config {
        crate::config::Config {
            tiers: std::collections::BTreeMap::new(),
            providers: std::collections::BTreeMap::new(),
            default_tier: "balanced".to_string(),
        }
    }

    fn make_cancelled_run(events_path: std::path::PathBuf, turn: u64) -> Run {
        Run {
            session_id: "test".to_string(),
            state: RunState::Cancelled {
                events_path: events_path.clone(),
                turn,
            },
            cancel_token: std::sync::Arc::new(tokio::sync::Notify::new()),
            lock_path: std::path::PathBuf::new(),
        }
    }

    #[tokio::test]
    async fn cancel_when_idle_produces_turn_end() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        {
            let mut store = EventStore::open(&events_path).unwrap();
            store
                .append(EventPayload::SessionMeta {
                    ts: "2026-05-20T00:00:00Z".to_string(),
                    schema_version: 1,
                    session_id: "test".to_string(),
                    created_at: "2026-05-20T00:00:00Z".to_string(),
                    kuku_version: "0.1.0".to_string(),
                })
                .unwrap();
            store
                .append(EventPayload::TurnStart {
                    turn: 1,
                    ts: "2026-05-20T00:00:00Z".to_string(),
                })
                .unwrap();
        }

        let mut run = make_cancelled_run(events_path.clone(), 1);
        let result = run.next().await.unwrap();
        assert!(result.is_none());

        let events = EventStore::replay(&events_path).unwrap();
        let last = events.last().unwrap();
        assert!(matches!(
            &last.payload,
            EventPayload::TurnEnd { turn: 1, .. }
        ));
    }

    #[tokio::test]
    async fn cancel_sets_token_and_transitions_state() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());
        let run = Run {
            session_id: "test".to_string(),
            state: RunState::Cancelled {
                events_path: events_path.clone(),
                turn: 1,
            },
            cancel_token: cancel_token.clone(),
            lock_path: std::path::PathBuf::new(),
        };

        cancel_token.notify_waiters();
        assert!(matches!(&run.state, RunState::Cancelled { .. }));
    }

    #[tokio::test]
    async fn cancel_during_streaming_aborts_stream() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        std::fs::write(&events_path, "").unwrap();
        let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());

        let token_clone = cancel_token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            token_clone.notify_waiters();
        });

        let pending = PendingRun {
            session_id: "test".to_string(),
            query: crate::query::types::Query::new("test"),
            events_path: events_path.clone(),
            kuku_home: dir.path().to_path_buf(),
            workspace: dir.path().to_path_buf(),
            policy_path: dir.path().join("policy.md"),
            turn: 1,
            request_num: 1,
            cumulative_input_tokens: 0,
            cumulative_output_tokens: 0,
            cumulative_cache_read_input_tokens: 0,
            cumulative_cache_creation_input_tokens: 0,
            resolved: None,
            queued_tool_calls: std::collections::VecDeque::new(),
            saved_tool_call: None,
            config: std::sync::Arc::new(test_config()),
            prompts_dir: None,
            subagent_registry: None,
            skill_registry: None,
            skill_content_hash: None,
            skill_body: None,
            child_session_count: 0,
            tool_registry_override: None,
            cancel_token: cancel_token.clone(),
        };

        let stream: std::pin::Pin<
            Box<
                dyn futures_core::Stream<
                        Item = std::result::Result<
                            crate::provider::chunk::ProviderChunk,
                            crate::provider::types::ProviderFailure,
                        >,
                    > + Send
                    + Sync,
            >,
        > = Box::pin(tokio_stream::pending());

        let mut streaming = StreamingChunkState {
            pending,
            request_id: "req_1".to_string(),
            stream,
            accumulated_text: "partial".to_string(),
            accumulated_thinking: String::new(),
            stop_reason: None,
            tool_calls: Vec::new(),
            tool_arg_buffers: Vec::new(),
            provider_request_id: None,
            usage: None,
            lead_events: Vec::new(),
        };

        let run = Run {
            session_id: "test".to_string(),
            state: RunState::Pending(Box::new(PendingRun {
                session_id: "test".to_string(),
                query: crate::query::types::Query::new("test"),
                events_path: events_path.clone(),
                kuku_home: dir.path().to_path_buf(),
                workspace: dir.path().to_path_buf(),
                policy_path: dir.path().join("policy.md"),
                turn: 1,
                request_num: 1,
                cumulative_input_tokens: 0,
                cumulative_output_tokens: 0,
                cumulative_cache_read_input_tokens: 0,
                cumulative_cache_creation_input_tokens: 0,
                resolved: None,
                queued_tool_calls: std::collections::VecDeque::new(),
                saved_tool_call: None,
                config: std::sync::Arc::new(test_config()),
                prompts_dir: None,
                subagent_registry: None,
                skill_registry: None,
                skill_content_hash: None,
                skill_body: None,
                child_session_count: 0,
                tool_registry_override: None,
                cancel_token: cancel_token.clone(),
            })),
            cancel_token: cancel_token.clone(),
            lock_path: std::path::PathBuf::new(),
        };

        let result = run.poll_stream_chunk(&mut streaming).await.unwrap();
        assert!(result.is_none());
        assert_eq!(streaming.stop_reason.as_deref(), Some("cancelled"));
    }

    #[tokio::test]
    async fn cancelled_tool_result_envelope_has_correct_fields() {
        let result = crate::tool::ToolResultEnvelope::cancelled("test cancel");
        assert_eq!(result.status, "cancelled");
        assert_eq!(result.summary, "test cancel");
        assert!(result.model_content.is_empty());
        assert!(!result.truncated);
        assert_eq!(
            result.structured,
            Some(serde_json::json!({"kind": "cancelled"}))
        );
    }

    #[tokio::test]
    async fn resume_after_cancel_includes_turn_end_in_history() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        {
            let mut store = EventStore::open(&events_path).unwrap();
            store
                .append(EventPayload::SessionMeta {
                    ts: "2026-05-20T00:00:00Z".to_string(),
                    schema_version: 1,
                    session_id: "test".to_string(),
                    created_at: "2026-05-20T00:00:00Z".to_string(),
                    kuku_version: "0.1.0".to_string(),
                })
                .unwrap();
            store
                .append(EventPayload::TurnStart {
                    turn: 1,
                    ts: "2026-05-20T00:00:00Z".to_string(),
                })
                .unwrap();
            store
                .append(EventPayload::UserInput {
                    turn: 1,
                    ts: "2026-05-20T00:00:01Z".to_string(),
                    text: "hello".to_string(),
                })
                .unwrap();
            store
                .append(EventPayload::ModelResponse {
                    turn: 1,
                    ts: "2026-05-20T00:00:02Z".to_string(),
                    request_id: "req_1".to_string(),
                    text: "partial".to_string(),
                    thinking: None,
                    stop_reason: "cancelled".to_string(),
                    tool_call_count: None,
                    usage: serde_json::json!({}),
                })
                .unwrap();
            store
                .append(EventPayload::TurnEnd {
                    turn: 1,
                    ts: "2026-05-20T00:00:03Z".to_string(),
                })
                .unwrap();
        }

        let events = EventStore::replay(&events_path).unwrap();
        let history = crate::context::rebuild_history(&events);
        assert_eq!(history.len(), 2);
        let messages: Vec<_> = history.iter().map(|m| format!("{:?}", m.role)).collect();
        assert!(messages.contains(&"User".to_string()));
        assert!(messages.contains(&"Assistant".to_string()));
    }
}
