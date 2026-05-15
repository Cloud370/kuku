use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

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
use crate::permission::{
    append_project_allow_rule, decide_tool_call, load_project_policy, recover_session_grants,
    GateDecisionKind, GateSource,
};
use crate::provider::config::{resolve_config, ResolveConfigInput};
use crate::provider::types::{
    ProviderKind, ProviderRequest, ProviderResponse, ProviderToolCall, ResolvedProvider,
};
use crate::provider::{self, Provider};
use crate::session::{
    current_workspace, global_memory_path, kuku_home, new_session_id, project_memory_path,
    project_policy_path, session_events_path, validate_session_id,
};
use crate::notice::{
    render_notice_block, ContextDriftEntry, ContextDriftStatus, Notice, NoticeKind, NoticeSeverity,
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
    PermissionRequested { request: PermissionRequest },
    Done { output: RunOutput },
}

#[derive(Debug)]
pub struct Run {
    session_id: String,
    state: RunState,
}

#[derive(Debug)]
enum RunState {
    Pending(Box<PendingRun>),
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrackedFileSnapshot {
    path: String,
    hash: String,
}

#[derive(Debug)]
struct QueuedToolCall {
    tool_call: ProviderToolCall,
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
    Done(RunOutput),
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
            })),
        })
    }

    pub async fn run(self) -> Result<RunOutput> {
        let mut run = self.start_session().await?;
        loop {
            match run.next().await? {
                Some(UiEvent::PermissionRequested { .. }) => run.deny_pending().await?,
                Some(UiEvent::Done { output }) => return Ok(output),
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
                    PendingStep::Done(output) => {
                        self.state = RunState::Done(None);
                        return Ok(Some(UiEvent::Done { output }));
                    }
                },
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

    pub async fn decide(
        &mut self,
        request_id: impl AsRef<str>,
        choice: PermissionChoice,
    ) -> Result<()> {
        self.apply_choice(request_id.as_ref(), choice, "host").await
    }

    async fn deny_pending(&mut self) -> Result<()> {
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
    ) -> Result<()> {
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
        self.state = RunState::Pending(Box::new(pending));
        Ok(())
    }
}

async fn advance_pending(mut pending: PendingRun) -> Result<PendingStep> {
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
                            summary: permission_summary(
                                &queued_tool_call.tool_call.name,
                                &queued_tool_call.tool_call.args,
                            ),
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
                        execute_tool_call(&mut pending, &queued_tool_call.tool_call).await?;
                        continue;
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
                                summary: permission_summary(
                                    &queued_tool_call.tool_call.name,
                                    &queued_tool_call.tool_call.args,
                                ),
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
                        execute_tool_call(&mut pending, &queued_tool_call.tool_call).await?;
                        continue;
                    }
                }
            }

            execute_tool_call(&mut pending, &queued_tool_call.tool_call).await?;
            continue;
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
    if pending.turn > 1 {
        if let Some(drift_notice) =
            build_context_drift_notice(&pending.workspace, &existing_events)?
        {
            assembly
                .prelude_messages
                .insert(1, crate::context::CanonicalMessage::user_text(drift_notice));
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
    };

    match provider::call_provider(&resolved.config, &request).await {
        Ok(response) => handle_provider_response(pending, request_id, response).await,
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

async fn handle_provider_response(
    mut pending: PendingRun,
    request_id: String,
    response: ProviderResponse,
) -> Result<PendingStep> {
    let has_tool_calls = !response.tool_calls.is_empty();
    {
        let mut store = EventStore::open(&pending.events_path)?;
        store.append(EventPayload::ModelResponse {
            turn: pending.turn,
            ts: now_timestamp()?,
            request_id: request_id.clone(),
            text: response.assistant_text.clone(),
            stop_reason: response.stop_reason.clone().unwrap_or_else(|| {
                if has_tool_calls {
                    "tool_use".to_string()
                } else {
                    "end_turn".to_string()
                }
            }),
            tool_call_count: has_tool_calls.then_some(response.tool_calls.len() as u64),
            usage: serde_json::to_value(&response.usage).unwrap_or_default(),
        })?;

        if !has_tool_calls {
            store.append(EventPayload::TurnEnd {
                turn: pending.turn,
                ts: now_timestamp()?,
            })?;
            return Ok(PendingStep::Done(RunOutput {
                session_id: pending.session_id.clone(),
                text: response.assistant_text,
            }));
        }

        for tool_call in &response.tool_calls {
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

    for tool_call in response.tool_calls {
        pending
            .queued_tool_calls
            .push_back(QueuedToolCall { tool_call });
    }

    Ok(PendingStep::Pending(Box::new(pending)))
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
        provider::types::ProviderFailureKind::Parse => "parse",
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

fn now_timestamp() -> Result<String> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}

fn build_context_drift_notice(workspace: &Path, events: &[StoredEvent]) -> Result<Option<String>> {
    let tracked = rebuild_tracked_file_snapshots(events);
    if tracked.is_empty() {
        return Ok(None);
    }

    let mut entries = Vec::new();
    for snapshot in tracked.values() {
        let path = PathBuf::from(&snapshot.path);
        let label = path
            .strip_prefix(workspace)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        match std::fs::read(&path) {
            Ok(current_bytes) => {
                let current_hash = content_hash_bytes(&current_bytes);
                if current_hash == snapshot.hash {
                    continue;
                }
                entries.push(ContextDriftEntry {
                    path: label,
                    status: ContextDriftStatus::Updated,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                entries.push(ContextDriftEntry {
                    path: label,
                    status: ContextDriftStatus::Deleted,
                });
            }
            Err(_) => continue,
        }
    }

    if entries.is_empty() {
        return Ok(None);
    }

    let notice = Notice {
        kind: NoticeKind::ContextDrift { entries },
        severity: NoticeSeverity::Info,
    };
    Ok(Some(render_notice_block(&notice)))
}

fn rebuild_tracked_file_snapshots(events: &[StoredEvent]) -> BTreeMap<String, TrackedFileSnapshot> {
    let mut tracked = tracked_files_from_latest_model_request(events);

    for event in events {
        let EventPayload::ToolResult {
            status,
            structured: Some(structured),
            ..
        } = &event.payload
        else {
            continue;
        };
        if status != "ok" {
            continue;
        }
        update_tracked_snapshot_from_tool_result(&mut tracked, structured);
    }

    tracked
}

fn tracked_files_from_latest_model_request(
    events: &[StoredEvent],
) -> BTreeMap<String, TrackedFileSnapshot> {
    let mut tracked = BTreeMap::new();
    let Some(provenance) = events.iter().rev().find_map(|event| match &event.payload {
        EventPayload::ModelRequest {
            provenance: Some(provenance),
            ..
        } => Some(provenance),
        _ => None,
    }) else {
        return tracked;
    };

    for source in provenance
        .get("project_instruction_sources")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        if let (Some(path), Some(hash)) = (
            source.get("path").and_then(serde_json::Value::as_str),
            source.get("hash").and_then(serde_json::Value::as_str),
        ) {
            tracked.insert(
                path.to_string(),
                TrackedFileSnapshot {
                    path: path.to_string(),
                    hash: hash.to_string(),
                },
            );
        }
    }
    for source in provenance
        .get("memory_sources")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        if let (Some(path), Some(hash)) = (
            source.get("path").and_then(serde_json::Value::as_str),
            source.get("hash").and_then(serde_json::Value::as_str),
        ) {
            tracked.insert(
                path.to_string(),
                TrackedFileSnapshot {
                    path: path.to_string(),
                    hash: hash.to_string(),
                },
            );
        }
    }

    tracked
}

fn update_tracked_snapshot_from_tool_result(
    tracked: &mut BTreeMap<String, TrackedFileSnapshot>,
    structured: &serde_json::Value,
) {
    let Some(kind) = structured.get("kind").and_then(serde_json::Value::as_str) else {
        return;
    };
    match kind {
        "file_content" => {
            let is_full = structured
                .get("is_full_file_snapshot")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if !is_full {
                return;
            }
            let Some(path) = structured
                .get("canonical_path")
                .and_then(serde_json::Value::as_str)
            else {
                return;
            };
            let Some(hash) = structured
                .get("content_hash")
                .and_then(serde_json::Value::as_str)
            else {
                return;
            };
            tracked.insert(
                path.to_string(),
                TrackedFileSnapshot {
                    path: path.to_string(),
                    hash: hash.to_string(),
                },
            );
        }
        "file_edit" | "file_write" | "memory_write" | "memory_forget" => {
            let Some(path) = structured
                .get("canonical_path")
                .and_then(serde_json::Value::as_str)
            else {
                return;
            };
            let Some(existing) = tracked.get_mut(path) else {
                return;
            };
            let Some(hash) = structured
                .get("content_hash_after")
                .or_else(|| structured.get("content_hash"))
                .and_then(serde_json::Value::as_str)
            else {
                return;
            };
            existing.hash = hash.to_string();
        }
        _ => {}
    }
}

fn content_hash_bytes(bytes: &[u8]) -> String {
    let digest = sha2::Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

#[cfg(test)]
mod tests {
    use crate::tool::dispatch;
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
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
