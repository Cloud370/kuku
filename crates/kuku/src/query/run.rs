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

impl Run {
    /// The session ID for this run.
    pub fn session_id(&self) -> &str {
        &self.session_id
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
                    PendingStep::ToolCallReady { pending, ui_event } => {
                        self.state = RunState::Pending(pending);
                        return Ok(Some(ui_event));
                    }
                    PendingStep::ToolResultReady { pending, ui_event } => {
                        self.state = RunState::Pending(pending);
                        return Ok(Some(ui_event));
                    }
                    PendingStep::Done(output) => {
                        self.state = RunState::Done(None);
                        return Ok(Some(UiEvent::Done { output }));
                    }
                },
                RunState::Streaming(mut streaming) => {
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
                                PendingStep::Done(output) => {
                                    self.state = RunState::Done(None);
                                    return Ok(Some(UiEvent::Done { output }));
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
                RunState::Done(Some(output)) => {
                    self.state = RunState::Done(None);
                    return Ok(Some(UiEvent::Done { output }));
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
            let chunk = match streaming.stream.next().await {
                Some(Ok(chunk)) => chunk,
                Some(Err(_failure)) => return Ok(None),
                None => return Ok(None),
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
                    ..
                } => {
                    streaming.usage = Some(crate::provider::types::ProviderUsage {
                        input_tokens: Some(input_tokens),
                        output_tokens: Some(output_tokens),
                    });
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
        execute_tool_call(&mut pending, &waiting.queued_tool_call.tool_call).await?;
        let tool_result_event = Some(UiEvent::ToolResult {
            tool_call_id: waiting.queued_tool_call.tool_call.id.clone(),
            summary: waiting.queued_tool_call.summary.clone(),
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
