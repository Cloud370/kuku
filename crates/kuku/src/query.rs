use std::collections::VecDeque;
use std::path::PathBuf;
use std::pin::Pin;

use futures_core::Stream;
use sha2::Digest;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::context::{
    assemble_context, build_request_provenance, rebuild_history, ContextInput, EnvironmentSource,
    FileSource, HistoryRange, InstructionSource, MemorySource, RequestProvenanceInput,
    ToolRegistryProvenance,
};
use crate::error::{Error, Result};
use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::notice::{
    build_runtime_notices, compute_context_headroom, render_notice_block, NoticeAssemblyInput,
};
use crate::permission::{
    append_project_allow_rule, decide_tool_call, load_project_policy, recover_session_grants,
    GateDecisionKind, GateSource,
};
use crate::provider::chunk::ProviderChunk;
use crate::provider::config::{resolve_config, ResolveConfigInput};
use crate::provider::types::{
    ProviderFailure, ProviderKind, ProviderRequest, ProviderToolCall, ResolvedProvider,
};
use crate::provider::{self, Provider};
use crate::session::{
    current_workspace, global_memory_path, kuku_home, new_session_id, project_memory_path,
    project_policy_path, session_events_path, validate_session_id,
};
use crate::tool::{self, ToolDefinition};

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    prompt: String,
    session_id: Option<String>,
    provider: Option<Provider>,
    model: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    max_output_tokens: Option<u32>,
    temperature: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub session_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
    pub id: String,
    pub tool_call_id: String,
    pub tool: String,
    pub risk: String,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionChoice {
    Once,
    Session,
    Project,
    Deny,
}

/// Host-facing runtime event stream.
///
/// This enum is non-exhaustive; hosts must keep a fallback arm when matching it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolCall {
        tool_call_id: String,
        tool: String,
        summary: String,
    },
    ToolResult {
        tool_call_id: String,
        summary: String,
    },
    PermissionRequested {
        request: PermissionRequest,
    },
    Done {
        output: RunOutput,
    },
}

#[derive(Debug)]
pub struct Run {
    session_id: String,
    state: RunState,
}

#[derive(Debug)]
enum RunState {
    Pending(Box<PendingRun>),
    Streaming(Box<StreamingChunkState>),
    WaitingForPermission(Box<PendingPermission>),
    Done(Option<RunOutput>),
}

#[derive(Debug)]
struct PendingRun {
    session_id: String,
    query: Query,
    events_path: PathBuf,
    kuku_home: PathBuf,
    workspace: PathBuf,
    policy_path: PathBuf,
    turn: u64,
    request_num: u64,
    resolved: Option<ResolvedRuntime>,
    queued_tool_calls: VecDeque<QueuedToolCall>,
    saved_tool_call: Option<QueuedToolCall>,
}

#[derive(Debug)]
struct ResolvedRuntime {
    config: ResolvedProvider,
    registry: Vec<ToolDefinition>,
    registry_hash: String,
    ordered_tool_names: Vec<String>,
    tool_count: usize,
    provider_name: String,
}

#[derive(Debug)]
struct QueuedToolCall {
    tool_call: ProviderToolCall,
    summary: String,
}

#[derive(Debug)]
struct PendingPermission {
    pending: PendingRun,
    queued_tool_call: QueuedToolCall,
    request: PermissionRequest,
}

enum PendingStep {
    Pending(Box<PendingRun>),
    NeedPermission(Box<PendingPermission>),
    Streaming(Box<StreamingChunkState>),
    ToolCallReady {
        pending: Box<PendingRun>,
        ui_event: UiEvent,
    },
    ToolResultReady {
        pending: Box<PendingRun>,
        ui_event: UiEvent,
    },
    Done(RunOutput),
}

struct StreamingChunkState {
    pending: PendingRun,
    request_id: String,
    stream: Pin<Box<dyn Stream<Item = std::result::Result<ProviderChunk, ProviderFailure>> + Send>>,
    accumulated_text: String,
    accumulated_thinking: String,
    stop_reason: Option<String>,
    tool_calls: Vec<ProviderToolCall>,
    tool_arg_buffers: Vec<(u64, String)>, // (index, accumulated JSON args)
    provider_request_id: Option<String>,
    usage: Option<crate::provider::types::ProviderUsage>,
}

impl std::fmt::Debug for StreamingChunkState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamingChunkState")
            .field("request_id", &self.request_id)
            .field("accumulated_text", &self.accumulated_text)
            .field("stop_reason", &self.stop_reason)
            .field("tool_calls", &self.tool_calls)
            .finish_non_exhaustive()
    }
}

pub fn query(prompt: impl Into<String>) -> Query {
    Query::new(prompt)
}

impl Query {
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            session_id: None,
            provider: None,
            model: None,
            base_url: None,
            api_key: None,
            max_output_tokens: None,
            temperature: None,
        }
    }

    pub fn session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn provider(mut self, provider: Provider) -> Self {
        self.provider = Some(provider);
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    pub fn max_output_tokens(mut self, max_output_tokens: u32) -> Self {
        self.max_output_tokens = Some(max_output_tokens);
        self
    }

    pub fn temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub async fn start(self) -> Result<Run> {
        self.start_session().await
    }

    async fn start_session(self) -> Result<Run> {
        let kuku_home = kuku_home()?;
        let workspace = current_workspace()?;
        let session_id = match self.session_id.as_deref() {
            Some(session_id) => {
                validate_session_id(session_id)?;
                session_id.to_string()
            }
            None => new_session_id(),
        };
        validate_session_id(&session_id)?;

        let events_path = session_events_path(&kuku_home, &workspace, &session_id)?;
        let policy_path = project_policy_path(&kuku_home, &workspace)?;
        let existing_events = EventStore::replay(&events_path)?;
        validate_existing_session(&existing_events)?;
        let turn = next_turn(&existing_events);
        let is_new_session = existing_events.is_empty();

        let mut store = EventStore::open(&events_path)?;
        if is_new_session {
            let created_at = now_timestamp()?;
            store.append(EventPayload::SessionMeta {
                ts: created_at.clone(),
                schema_version: 1,
                session_id: session_id.clone(),
                created_at,
                kuku_version: env!("CARGO_PKG_VERSION").to_string(),
            })?;
        }

        store.append(EventPayload::TurnStart {
            turn,
            ts: now_timestamp()?,
        })?;
        store.append(EventPayload::UserInput {
            turn,
            ts: now_timestamp()?,
            text: self.prompt.clone(),
        })?;

        Ok(Run {
            session_id: session_id.clone(),
            state: RunState::Pending(Box::new(PendingRun {
                session_id,
                query: self,
                events_path,
                kuku_home,
                workspace,
                policy_path,
                turn,
                request_num: 0,
                resolved: None,
                queued_tool_calls: VecDeque::new(),
                saved_tool_call: None,
            })),
        })
    }

    pub async fn run(self) -> Result<RunOutput> {
        let mut run = self.start_session().await?;
        loop {
            match run.next().await? {
                Some(UiEvent::PermissionRequested { .. }) => {
                    run.deny_pending().await?;
                }
                Some(UiEvent::Done { output }) => return Ok(output),
                Some(_) => continue,
                None => {
                    return Err(Error::InvalidEventStream(
                        "run ended without producing Done".to_string(),
                    ))
                }
            }
        }
    }
}

impl Run {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub async fn next(&mut self) -> Result<Option<UiEvent>> {
        loop {
            match std::mem::replace(&mut self.state, RunState::Done(None)) {
                RunState::Pending(pending) => match advance_pending(*pending).await? {
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
                            // Stream finished — process collected response
                            let step = finish_streaming(*streaming).await?;
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
                ProviderChunk::StreamStart {
                    request_id: rid,
                    model: _,
                } => {
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

    pub async fn decide(
        &mut self,
        request_id: impl AsRef<str>,
        choice: PermissionChoice,
    ) -> Result<Option<UiEvent>> {
        self.apply_choice(request_id.as_ref(), choice, "host").await
    }

    async fn deny_pending(&mut self) -> Result<Option<UiEvent>> {
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

async fn finish_streaming(state: StreamingChunkState) -> Result<PendingStep> {
    let StreamingChunkState {
        mut pending,
        request_id,
        accumulated_text,
        accumulated_thinking,
        stop_reason,
        tool_calls,
        usage,
        ..
    } = state;

    let has_tool_calls = !tool_calls.is_empty();
    let final_stop_reason = stop_reason.unwrap_or_else(|| {
        if has_tool_calls {
            "tool_use".to_string()
        } else {
            "end_turn".to_string()
        }
    });

    {
        let mut store = EventStore::open(&pending.events_path)?;
        store.append(EventPayload::ModelResponse {
            turn: pending.turn,
            ts: now_timestamp()?,
            request_id: request_id.clone(),
            text: accumulated_text.clone(),
            thinking: if accumulated_thinking.is_empty() {
                None
            } else {
                Some(accumulated_thinking.clone())
            },
            stop_reason: final_stop_reason.clone(),
            tool_call_count: has_tool_calls.then_some(tool_calls.len() as u64),
            usage: serde_json::to_value(&usage).unwrap_or_default(),
        })?;

        if !has_tool_calls {
            store.append(EventPayload::TurnEnd {
                turn: pending.turn,
                ts: now_timestamp()?,
            })?;
            return Ok(PendingStep::Done(RunOutput {
                session_id: pending.session_id.clone(),
                text: accumulated_text,
            }));
        }

        for tool_call in &tool_calls {
            store.append(EventPayload::ToolCall {
                turn: pending.turn,
                ts: now_timestamp()?,
                tool_call_id: tool_call.id.clone(),
                request_id: request_id.clone(),
                index: tool_call.index,
                tool: tool_call.name.clone(),
                args: tool_call.args.clone(),
            })?;
        }
    }

    for tool_call in tool_calls {
        let summary = permission_summary(&tool_call.name, &tool_call.args);
        pending
            .queued_tool_calls
            .push_back(QueuedToolCall { tool_call, summary });
    }

    Ok(PendingStep::Pending(Box::new(pending)))
}

async fn advance_pending(mut pending: PendingRun) -> Result<PendingStep> {
    // If a saved tool call exists, execute it and yield ToolResult.
    if let Some(saved) = pending.saved_tool_call.take() {
        execute_tool_call(&mut pending, &saved.tool_call).await?;
        return Ok(PendingStep::ToolResultReady {
            pending: Box::new(pending),
            ui_event: UiEvent::ToolResult {
                tool_call_id: saved.tool_call.id.clone(),
                summary: saved.summary,
            },
        });
    }

    loop {
        if let Some(queued_tool_call) = pending.queued_tool_calls.pop_front() {
            if let Some(definition) =
                find_tool_definition(&pending, &queued_tool_call.tool_call.name)
            {
                let candidate = permission_candidate(
                    &pending.kuku_home,
                    &pending.workspace,
                    &queued_tool_call.tool_call.name,
                    &queued_tool_call.tool_call.args,
                );
                let policy = load_project_policy(&pending.policy_path)?;
                let prior_events = EventStore::replay(&pending.events_path)?;
                let session_grants = recover_session_grants(&prior_events);
                let decision = decide_tool_call(
                    &queued_tool_call.tool_call.name,
                    &definition.risk,
                    &candidate,
                    &policy,
                    &session_grants,
                );
                match decision.kind {
                    GateDecisionKind::Ask => {
                        let request = PermissionRequest {
                            id: queued_tool_call.tool_call.id.clone(),
                            tool_call_id: queued_tool_call.tool_call.id.clone(),
                            tool: queued_tool_call.tool_call.name.clone(),
                            risk: definition.risk.clone(),
                            summary: queued_tool_call.summary.clone(),
                        };
                        append_permission_request(&pending.events_path, pending.turn, &request)?;
                        return Ok(PendingStep::NeedPermission(Box::new(PendingPermission {
                            pending,
                            queued_tool_call,
                            request,
                        })));
                    }
                    GateDecisionKind::Allow => {
                        if !matches!(decision.source, GateSource::TrustPosture) {
                            let choice = if matches!(decision.source, GateSource::ProjectPolicy) {
                                PermissionChoice::Project
                            } else if matches!(decision.source, GateSource::SessionGrant) {
                                PermissionChoice::Session
                            } else {
                                PermissionChoice::Once
                            };
                            append_permission_decision(
                                &pending.events_path,
                                pending.turn,
                                &queued_tool_call.tool_call.id,
                                choice,
                                gate_source_name(decision.source),
                                &permission_rule(
                                    &pending.kuku_home,
                                    &pending.workspace,
                                    &queued_tool_call.tool_call.name,
                                    &queued_tool_call.tool_call.args,
                                ),
                            )?;
                        }
                        // Save for next call, yield ToolCall first
                        pending.saved_tool_call = Some(queued_tool_call);
                        let saved = pending.saved_tool_call.as_ref().unwrap();
                        let tc_id = saved.tool_call.id.clone();
                        let tc_name = saved.tool_call.name.clone();
                        let tc_summary = saved.summary.clone();
                        return Ok(PendingStep::ToolCallReady {
                            pending: Box::new(pending),
                            ui_event: UiEvent::ToolCall {
                                tool_call_id: tc_id,
                                tool: tc_name,
                                summary: tc_summary,
                            },
                        });
                    }
                    GateDecisionKind::Deny => {
                        append_permission_request(
                            &pending.events_path,
                            pending.turn,
                            &PermissionRequest {
                                id: queued_tool_call.tool_call.id.clone(),
                                tool_call_id: queued_tool_call.tool_call.id.clone(),
                                tool: queued_tool_call.tool_call.name.clone(),
                                risk: definition.risk.clone(),
                                summary: queued_tool_call.summary.clone(),
                            },
                        )?;
                        append_permission_decision(
                            &pending.events_path,
                            pending.turn,
                            &queued_tool_call.tool_call.id,
                            PermissionChoice::Deny,
                            gate_source_name(decision.source),
                            &permission_rule(
                                &pending.kuku_home,
                                &pending.workspace,
                                &queued_tool_call.tool_call.name,
                                &queued_tool_call.tool_call.args,
                            ),
                        )?;
                        let tc_id = queued_tool_call.tool_call.id.clone();
                        let summary = queued_tool_call.summary.clone();
                        execute_tool_call(&mut pending, &queued_tool_call.tool_call).await?;
                        return Ok(PendingStep::ToolResultReady {
                            pending: Box::new(pending),
                            ui_event: UiEvent::ToolResult {
                                tool_call_id: tc_id,
                                summary,
                            },
                        });
                    }
                }
            }

            // No definition found — save and yield ToolCall, execute on next call
            pending.saved_tool_call = Some(queued_tool_call);
            let saved = pending.saved_tool_call.as_ref().unwrap();
            let tc_id = saved.tool_call.id.clone();
            let tc_name = saved.tool_call.name.clone();
            let tc_summary = saved.summary.clone();
            return Ok(PendingStep::ToolCallReady {
                pending: Box::new(pending),
                ui_event: UiEvent::ToolCall {
                    tool_call_id: tc_id,
                    tool: tc_name,
                    summary: tc_summary,
                },
            });
        }

        return call_provider_step(pending).await;
    }
}

async fn call_provider_step(mut pending: PendingRun) -> Result<PendingStep> {
    ensure_resolved(&mut pending)?;
    pending.request_num += 1;

    if pending.request_num > 20 {
        let provider_name = pending
            .resolved
            .as_ref()
            .map(|resolved| resolved.provider_name.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let model = pending
            .resolved
            .as_ref()
            .map(|resolved| resolved.config.model.clone())
            .unwrap_or_else(|| "unknown".to_string());
        append_model_error(
            &pending.events_path,
            pending.turn,
            format!("req_{}", pending.request_num),
            "loop_limit",
            "tool loop exceeded maximum provider requests",
            Some(provider_name),
            Some(model),
        )?;
        append_turn_end(&pending.events_path, pending.turn)?;
        return Err(Error::Provider(
            "tool loop exceeded maximum provider requests".to_string(),
        ));
    }

    let resolved = pending.resolved.as_ref().expect("resolved runtime exists");
    let existing_events = EventStore::replay(&pending.events_path)?;
    let history = rebuild_history(&existing_events);
    let project_instructions = load_project_instruction_sources(&pending.workspace)?;
    let (global_memory, project_memory) =
        load_memory_sources(&pending.kuku_home, &pending.workspace)?;
    let platform = platform_label().to_string();
    let current_date = current_date_string();
    let mut assembly = match assemble_context(ContextInput {
        environment: EnvironmentSource {
            workspace_path: pending.workspace.display().to_string(),
            platform: platform.clone(),
            current_date: current_date.clone(),
        },
        project_instructions,
        global_memory,
        project_memory,
        history,
        tools: tool::to_tool_schemas(&resolved.registry),
    }) {
        Ok(assembly) => assembly,
        Err(error) => {
            let request_id = format!("req_{}", pending.request_num);
            append_model_error(
                &pending.events_path,
                pending.turn,
                request_id,
                "prompt_render",
                &error.to_string(),
                Some(resolved.provider_name.clone()),
                Some(resolved.config.model.clone()),
            )?;
            append_turn_end(&pending.events_path, pending.turn)?;
            return Err(error);
        }
    };
    let estimated_input = last_input_tokens(&resolved.config.kind, &existing_events);
    let context_headroom = compute_context_headroom(
        resolved.config.max_context_tokens,
        pending.query.max_output_tokens,
        estimated_input,
    );
    if pending.turn > 1 {
        let notices = build_runtime_notices(NoticeAssemblyInput {
            workspace: &pending.workspace,
            events: &existing_events,
            context_budget_tier: context_headroom.tier,
        });
        for (offset, notice) in notices.into_iter().enumerate() {
            assembly.prelude_messages.insert(
                1 + offset,
                crate::context::CanonicalMessage::user_text(render_notice_block(&notice)),
            );
        }
    }
    let request_id = format!("req_{}", pending.request_num);
    let params = serde_json::json!({
        "max_output_tokens": pending.query.max_output_tokens,
        "temperature": pending.query.temperature,
    });
    let provenance = build_request_provenance(RequestProvenanceInput {
        request_id: request_id.clone(),
        role: "default".to_string(),
        workspace: pending.workspace.display().to_string(),
        platform: platform.clone(),
        current_date: current_date.clone(),
        project_instruction_sources: assembly
            .project_instruction_sources
            .iter()
            .map(|source| FileSource {
                path: source.path.clone(),
                hash: source.hash.clone(),
            })
            .collect(),
        memory_sources: assembly
            .memory_sources
            .iter()
            .map(|source| FileSource {
                path: source.path.clone(),
                hash: source.hash.clone(),
            })
            .collect(),
        prompt_asset_sources: assembly.prompt_asset_sources.clone(),
        history_range: HistoryRange {
            first_event_id: existing_events.first().map(|event| event.id),
            last_event_id: existing_events.last().map(|event| event.id),
        },
        tool_registry: ToolRegistryProvenance {
            hash: resolved.registry_hash.clone(),
            ordered_tool_names: resolved.ordered_tool_names.clone(),
            tool_count: resolved.tool_count,
        },
        provider_alias: resolved.provider_name.clone(),
        provider_format: provider_format_name(&resolved.config.kind).to_string(),
        resolved_provider: resolved.provider_name.clone(),
        resolved_model: resolved.config.model.clone(),
        params: params.clone(),
        token_estimate: None,
        context_budget_tier: context_headroom.tier.as_str().to_string(),
        max_context_tokens: Some(context_headroom.max_context_tokens),
        remaining_input_tokens: context_headroom.remaining_input_tokens,
    });

    {
        let mut store = EventStore::open(&pending.events_path)?;
        store.append(EventPayload::ModelRequest {
            turn: pending.turn,
            ts: now_timestamp()?,
            request_id: request_id.clone(),
            role: "default".to_string(),
            alias: resolved.provider_name.clone(),
            resolved_provider: resolved.provider_name.clone(),
            resolved_model: resolved.config.model.clone(),
            params,
            base_url: Some(resolved.config.base_url.clone()),
            message_count: Some(1 + assembly.prelude_messages.len() + assembly.history.len()),
            history_range_first: existing_events.first().map(|event| event.id),
            history_range_last: existing_events.last().map(|event| event.id),
            tool_registry_hash: Some(resolved.registry_hash.clone()),
            tool_count: Some(resolved.tool_count),
            ordered_tool_names: Some(resolved.ordered_tool_names.clone()),
            provenance: Some(serde_json::to_value(&provenance)?),
        })?;
    }

    let request = ProviderRequest {
        assembly,
        model: resolved.config.model.clone(),
        max_output_tokens: pending.query.max_output_tokens,
        temperature: pending.query.temperature,
        stream: true,
    };

    match provider::stream_provider(&resolved.config, &request).await {
        Ok(stream) => Ok(PendingStep::Streaming(Box::new(StreamingChunkState {
            pending,
            request_id,
            stream,
            accumulated_text: String::new(),
            accumulated_thinking: String::new(),
            stop_reason: None,
            tool_calls: Vec::new(),
            tool_arg_buffers: Vec::new(),
            provider_request_id: None,
            usage: None,
        }))),
        Err(failure) => {
            append_model_error(
                &pending.events_path,
                pending.turn,
                request_id,
                provider_failure_kind(&failure.kind),
                &failure.message,
                Some(resolved.provider_name.clone()),
                Some(resolved.config.model.clone()),
            )?;
            append_turn_end(&pending.events_path, pending.turn)?;
            Err(Error::Provider(failure.message))
        }
    }
}

fn ensure_resolved(pending: &mut PendingRun) -> Result<()> {
    if pending.resolved.is_some() {
        return Ok(());
    }

    let config = match resolve_config(ResolveConfigInput {
        provider: pending.query.provider,
        model: pending.query.model.clone(),
        base_url: pending.query.base_url.clone(),
        api_key: pending.query.api_key.clone(),
    }) {
        Ok(config) => config,
        Err(error) => {
            let request_id = format!(
                "req_{}",
                EventStore::replay(&pending.events_path)?.len() + 1
            );
            append_model_error(
                &pending.events_path,
                pending.turn,
                request_id,
                "missing_config",
                &error.to_string(),
                None,
                None,
            )?;
            append_turn_end(&pending.events_path, pending.turn)?;
            return Err(error);
        }
    };

    let registry = tool::builtin_registry();
    let registry_hash = tool::registry_hash(&registry);
    let ordered_tool_names = tool::ordered_tool_names(&registry);
    let tool_count = registry.len();
    if let Ok(policy_text) = std::fs::read_to_string(&pending.policy_path) {
        let policy_hash = sha2::Sha256::digest(policy_text.as_bytes());
        let mut store = EventStore::open(&pending.events_path)?;
        store.append(EventPayload::PolicyLoaded {
            ts: now_timestamp()?,
            policy_hash: format!("sha256:{:x}", policy_hash),
            mode: "default".to_string(),
        })?;
    }
    let provider_name = match config.kind {
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::OpenAiCompatible => "openai-compatible",
    }
    .to_string();

    pending.resolved = Some(ResolvedRuntime {
        config,
        registry,
        registry_hash,
        ordered_tool_names,
        tool_count,
        provider_name,
    });
    Ok(())
}

fn find_tool_definition<'a>(pending: &'a PendingRun, name: &str) -> Option<&'a ToolDefinition> {
    pending
        .resolved
        .as_ref()
        .and_then(|resolved| resolved.registry.iter().find(|tool| tool.name == name))
}

async fn execute_tool_call(pending: &mut PendingRun, tool_call: &ProviderToolCall) -> Result<()> {
    let prior_events = EventStore::replay(&pending.events_path)?;
    let result_event_id = EventStore::open(&pending.events_path)?.next_id();
    let result = tool::dispatch(
        &tool_call.name,
        &tool_call.args,
        &pending.workspace,
        &pending.kuku_home,
        &prior_events,
        result_event_id,
        Some(&tool_call.id),
    )
    .await;
    let mut store = EventStore::open(&pending.events_path)?;
    store.append(EventPayload::ToolResult {
        turn: pending.turn,
        ts: now_timestamp()?,
        tool_call_id: tool_call.id.clone(),
        status: result.status,
        summary: result.summary,
        model_content: result.model_content,
        truncated: result.truncated,
        structured: result.structured,
    })?;
    Ok(())
}

fn append_permission_request(
    events_path: &PathBuf,
    turn: u64,
    request: &PermissionRequest,
) -> Result<()> {
    let mut store = EventStore::open(events_path)?;
    store.append(EventPayload::PermissionRequest {
        turn,
        ts: now_timestamp()?,
        tool_call_id: request.tool_call_id.clone(),
        tool: request.tool.clone(),
        risk: request.risk.clone(),
        summary: request.summary.clone(),
    })?;
    Ok(())
}

fn append_permission_decision(
    events_path: &PathBuf,
    turn: u64,
    tool_call_id: &str,
    choice: PermissionChoice,
    source: &str,
    rule: &str,
) -> Result<()> {
    let mut store = EventStore::open(events_path)?;
    store.append(EventPayload::PermissionDecision {
        turn,
        ts: now_timestamp()?,
        tool_call_id: tool_call_id.to_string(),
        decision: permission_decision(choice).to_string(),
        scope: permission_scope(choice).to_string(),
        source: source.to_string(),
        rule: rule.to_string(),
    })?;
    Ok(())
}

fn append_model_error(
    events_path: &PathBuf,
    turn: u64,
    request_id: String,
    kind: &str,
    message: &str,
    resolved_provider: Option<String>,
    resolved_model: Option<String>,
) -> Result<()> {
    let mut store = EventStore::open(events_path)?;
    store.append(EventPayload::ModelError {
        turn,
        ts: now_timestamp()?,
        request_id,
        kind: kind.to_string(),
        message: message.to_string(),
        status: None,
        retryable: Some(false),
        resolved_provider,
        resolved_model,
    })?;
    Ok(())
}

fn append_turn_end(events_path: &PathBuf, turn: u64) -> Result<()> {
    let mut store = EventStore::open(events_path)?;
    store.append(EventPayload::TurnEnd {
        turn,
        ts: now_timestamp()?,
    })?;
    Ok(())
}

fn permission_summary(name: &str, args: &serde_json::Value) -> String {
    format!(
        "{} {}",
        name,
        serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string())
    )
}

fn permission_rule(
    kuku_home: &std::path::Path,
    workspace: &std::path::Path,
    name: &str,
    args: &serde_json::Value,
) -> String {
    format!(
        "{}({})",
        name,
        permission_candidate(kuku_home, workspace, name, args)
    )
}

fn permission_candidate(
    kuku_home: &std::path::Path,
    workspace: &std::path::Path,
    name: &str,
    args: &serde_json::Value,
) -> String {
    match name {
        "run_command" => args
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
        "memory.remember" | "memory.forget" => match args
            .get("scope")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
        {
            Some("global") => global_memory_path(kuku_home).display().to_string(),
            Some("project") => project_memory_path(kuku_home, workspace)
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            _ => String::new(),
        },
        _ => args
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
    }
}

fn permission_decision(choice: PermissionChoice) -> &'static str {
    match choice {
        PermissionChoice::Deny => "deny",
        PermissionChoice::Once | PermissionChoice::Session | PermissionChoice::Project => "allow",
    }
}

fn permission_scope(choice: PermissionChoice) -> &'static str {
    match choice {
        PermissionChoice::Once | PermissionChoice::Deny => "once",
        PermissionChoice::Session => "session",
        PermissionChoice::Project => "project",
    }
}

fn gate_source_name(source: GateSource) -> &'static str {
    match source {
        GateSource::HardGuard => "hard_guard",
        GateSource::ProjectPolicy => "project_policy",
        GateSource::SessionGrant => "session_grant",
        GateSource::TrustPosture => "trust_posture",
        GateSource::Host => "host",
        GateSource::DefaultAsk => "default_ask",
    }
}

fn validate_existing_session(events: &[StoredEvent]) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }

    match events.first().map(|event| &event.payload) {
        Some(EventPayload::SessionMeta { .. }) => Ok(()),
        _ => Err(Error::InvalidEventStream(
            "first event must be session.meta".to_string(),
        )),
    }
}

fn next_turn(events: &[StoredEvent]) -> u64 {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::TurnStart { turn, .. } => Some(*turn),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        + 1
}

fn provider_failure_kind(kind: &provider::types::ProviderFailureKind) -> &'static str {
    match kind {
        provider::types::ProviderFailureKind::Authentication => "authentication",
        provider::types::ProviderFailureKind::RateLimited => "rate_limited",
        provider::types::ProviderFailureKind::ContextTooLarge => "context_too_large",
        provider::types::ProviderFailureKind::InvalidRequest => "invalid_request",
        provider::types::ProviderFailureKind::ProviderUnavailable => "provider_unavailable",
        provider::types::ProviderFailureKind::Transport => "transport",
        provider::types::ProviderFailureKind::Unknown => "unknown",
    }
}

fn platform_label() -> &'static str {
    match std::env::consts::OS {
        "linux" => "linux",
        "windows" => "windows",
        "macos" => "macos",
        _ => "unknown",
    }
}

fn current_date_string() -> String {
    OffsetDateTime::now_utc().date().to_string()
}

fn provider_format_name(kind: &ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::OpenAiCompatible => "openai-compatible",
    }
}

fn sha256_text(text: &str) -> String {
    let digest = sha2::Sha256::digest(text.as_bytes());
    format!("sha256:{digest:x}")
}

fn load_project_instruction_sources(workspace: &std::path::Path) -> Result<Vec<InstructionSource>> {
    let mut sources = Vec::new();
    for (name, kind) in [("AGENTS.md", "agents"), ("CLAUDE.md", "claude")] {
        let path = workspace.join(name);
        if let Ok(content) = std::fs::read_to_string(&path) {
            sources.push(InstructionSource {
                path: path.display().to_string(),
                kind: kind.to_string(),
                hash: sha256_text(&content),
                content,
            });
        }
    }
    Ok(sources)
}

fn load_memory_sources(
    kuku_home: &std::path::Path,
    workspace: &std::path::Path,
) -> Result<(Option<MemorySource>, Option<MemorySource>)> {
    let global_memory = std::fs::read_to_string(global_memory_path(kuku_home))
        .ok()
        .map(|content| MemorySource {
            path: global_memory_path(kuku_home).display().to_string(),
            hash: sha256_text(&content),
            content,
        });

    let project_path = project_memory_path(kuku_home, workspace)?;
    let project_memory = std::fs::read_to_string(&project_path)
        .ok()
        .map(|content| MemorySource {
            path: project_path.display().to_string(),
            hash: sha256_text(&content),
            content,
        });

    Ok((global_memory, project_memory))
}

fn last_input_tokens(kind: &ProviderKind, events: &[StoredEvent]) -> Option<u32> {
    events.iter().rev().find_map(|event| match &event.payload {
        EventPayload::ModelResponse { usage, .. } => extract_input_tokens(kind, usage),
        _ => None,
    })
}

fn extract_input_tokens(kind: &ProviderKind, usage: &serde_json::Value) -> Option<u32> {
    let total = match kind {
        ProviderKind::Anthropic => {
            let input = usage
                .get("input_tokens")
                .and_then(serde_json::Value::as_u64)?;
            let cache_read = usage
                .get("cache_read_input_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let cache_creation = usage
                .get("cache_creation_input_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            input + cache_read + cache_creation
        }
        ProviderKind::OpenAiCompatible => usage
            .get("prompt_tokens")
            .and_then(serde_json::Value::as_u64)?,
    };
    u32::try_from(total).ok().filter(|&v| v > 0)
}

fn now_timestamp() -> Result<String> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}

#[cfg(test)]
mod tests {
    use super::extract_input_tokens;
    use crate::provider::types::ProviderKind;
    use crate::tool::dispatch;
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn extract_input_tokens_sums_anthropic_cache_fields() {
        let usage = serde_json::json!({
            "input_tokens": 845,
            "output_tokens": 106,
            "cache_read_input_tokens": 181888,
            "cache_creation_input_tokens": 0,
        });
        let result = extract_input_tokens(&ProviderKind::Anthropic, &usage);
        assert_eq!(result, Some(845 + 181888));
    }

    #[test]
    fn extract_input_tokens_uses_openai_prompt_tokens() {
        let usage = serde_json::json!({
            "prompt_tokens": 500,
            "completion_tokens": 100,
            "total_tokens": 600,
        });
        let result = extract_input_tokens(&ProviderKind::OpenAiCompatible, &usage);
        assert_eq!(result, Some(500));
    }

    #[test]
    fn extract_input_tokens_returns_none_for_empty_usage() {
        let usage = serde_json::json!({});
        assert_eq!(extract_input_tokens(&ProviderKind::Anthropic, &usage), None);
        assert_eq!(
            extract_input_tokens(&ProviderKind::OpenAiCompatible, &usage),
            None
        );
    }

    #[test]
    fn dispatch_uses_the_captured_home_for_memory_tools() {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let session_home = tempfile::tempdir().unwrap();
        let runtime_home = tempfile::tempdir().unwrap();
        let workspace = Path::new(dir.path());
        let args = serde_json::json!({"scope": "project", "kind": "how_to_work", "text": "Keep answers concise"});
        let expected_path = crate::session::project_memory_path(
            session_home.path(),
            &std::fs::canonicalize(workspace).unwrap(),
        )
        .unwrap();
        let unexpected_path = crate::session::project_memory_path(
            runtime_home.path(),
            &std::fs::canonicalize(workspace).unwrap(),
        )
        .unwrap();

        let previous = std::env::var_os("KUKU_HOME");
        std::env::set_var("KUKU_HOME", runtime_home.path());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(async {
            dispatch(
                "memory.remember",
                &args,
                workspace,
                session_home.path(),
                &[],
                1,
                None,
            )
            .await
        });
        match previous {
            Some(value) => std::env::set_var("KUKU_HOME", value),
            None => std::env::remove_var("KUKU_HOME"),
        }

        assert_eq!(result.status, "ok");
        assert!(std::fs::read_to_string(&expected_path)
            .unwrap()
            .contains("Keep answers concise"));
        assert!(!unexpected_path.exists());
    }
}
