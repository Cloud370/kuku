use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::error::{Error, Result};
use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::provider::{self, Provider};
use crate::session::{
    current_workspace, kuku_home, new_session_id, session_events_path, validate_session_id,
};

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

/// Host-facing runtime event stream.
///
/// This enum is non-exhaustive; hosts must keep a fallback arm when matching it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    Done { output: RunOutput },
}

/// Host-facing session run handle.
///
/// `Run` intentionally does not implement `Clone` or equality traits, so hosts
/// cannot duplicate or compare the event stream handle.
///
/// ```compile_fail
/// # async fn assert_run_is_not_clone_or_eq() -> kuku::Result<()> {
/// let run = kuku::query("hello").start().await?;
/// let _duplicate = run.clone();
/// let _same = &run == &run;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Run {
    session_id: String,
    done: Option<RunOutput>,
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

    /// Starts a session writer for this query.
    ///
    /// Callers must not start two writers for the same session concurrently.
    pub async fn start(self) -> Result<Run> {
        self.start_session().await
    }

    async fn start_session(self) -> Result<Run> {
        let kuku_home = kuku_home()?;
        let workspace = current_workspace()?;
        let session_id = match self.session_id {
            Some(session_id) => {
                validate_session_id(&session_id)?;
                session_id
            }
            None => new_session_id(),
        };
        validate_session_id(&session_id)?;

        let events_path = session_events_path(&kuku_home, &workspace, &session_id)?;
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
            text: self.prompt,
        })?;

        let output = RunOutput {
            session_id: session_id.clone(),
            text: String::new(),
        };

        Ok(Run {
            session_id,
            done: Some(output),
        })
    }

    pub async fn run(self) -> Result<RunOutput> {
        use crate::context::{assemble_context, rebuild_history, ContextInput};
        use crate::provider::config::{resolve_config, ResolveConfigInput};
        use crate::provider::types::{ProviderKind, ProviderRequest};
        use crate::tool;

        let run = self.clone().start_session().await?;
        let kuku_home = kuku_home()?;
        let workspace = current_workspace()?;
        let events_path = session_events_path(&kuku_home, &workspace, run.session_id())?;
        let initial_events = EventStore::replay(&events_path)?;
        let turn = current_turn(&initial_events)?;

        let resolved = match resolve_config(ResolveConfigInput {
            provider: self.provider,
            model: self.model.clone(),
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
        }) {
            Ok(resolved) => resolved,
            Err(error) => {
                let mut store = EventStore::open(&events_path)?;
                store.append(EventPayload::ModelError {
                    turn,
                    ts: now_timestamp()?,
                    request_id: format!("req_{}", initial_events.len() + 1),
                    kind: "missing_config".to_string(),
                    message: error.to_string(),
                    status: None,
                    retryable: Some(false),
                    resolved_provider: None,
                    resolved_model: None,
                })?;
                store.append(EventPayload::TurnEnd {
                    turn,
                    ts: now_timestamp()?,
                })?;
                return Err(error);
            }
        };

        let registry = tool::builtin_registry();
        let tool_schemas = tool::to_tool_schemas(&registry);
        let registry_hash = tool::registry_hash(&registry);
        let ordered_tool_names = tool::ordered_tool_names(&registry);
        let tool_count = registry.len();
        let provider_name = match resolved.kind {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAiCompatible => "openai-compatible",
        };
        let mut request_num = 0_u64;

        loop {
            request_num += 1;
            let request_id = format!("req_{request_num}");
            if request_num > 20 {
                let mut store = EventStore::open(&events_path)?;
                store.append(EventPayload::ModelError {
                    turn,
                    ts: now_timestamp()?,
                    request_id,
                    kind: "loop_limit".to_string(),
                    message: "tool loop exceeded maximum provider requests".to_string(),
                    status: None,
                    retryable: Some(false),
                    resolved_provider: Some(provider_name.to_string()),
                    resolved_model: Some(resolved.model.clone()),
                })?;
                store.append(EventPayload::TurnEnd {
                    turn,
                    ts: now_timestamp()?,
                })?;
                return Err(Error::Provider(
                    "tool loop exceeded maximum provider requests".to_string(),
                ));
            }

            let existing_events = EventStore::replay(&events_path)?;
            let history = rebuild_history(&existing_events);
            let assembly = assemble_context(ContextInput {
                project_instructions: Vec::new(),
                global_memory: None,
                project_memory: None,
                history,
                tools: tool_schemas.clone(),
            });
            let params = serde_json::json!({
                "max_output_tokens": self.max_output_tokens,
                "temperature": self.temperature,
            });

            {
                let mut store = EventStore::open(&events_path)?;
                store.append(EventPayload::ModelRequest {
                    turn,
                    ts: now_timestamp()?,
                    request_id: request_id.clone(),
                    role: "default".to_string(),
                    alias: provider_name.to_string(),
                    resolved_provider: provider_name.to_string(),
                    resolved_model: resolved.model.clone(),
                    params,
                    base_url: Some(resolved.base_url.clone()),
                    message_count: Some(assembly.sources.len()),
                    history_range_first: existing_events.first().map(|event| event.id),
                    history_range_last: existing_events.last().map(|event| event.id),
                    tool_registry_hash: Some(registry_hash.clone()),
                    tool_count: Some(tool_count),
                    ordered_tool_names: Some(ordered_tool_names.clone()),
                })?;
            }

            let request = ProviderRequest {
                assembly,
                model: resolved.model.clone(),
                max_output_tokens: self.max_output_tokens,
                temperature: self.temperature,
            };

            match provider::call_provider(&resolved, &request).await {
                Ok(response) => {
                    let has_tool_calls = !response.tool_calls.is_empty();
                    let mut store = EventStore::open(&events_path)?;
                    store.append(EventPayload::ModelResponse {
                        turn,
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
                            turn,
                            ts: now_timestamp()?,
                        })?;
                        return Ok(RunOutput {
                            session_id: run.session_id,
                            text: response.assistant_text,
                        });
                    }

                    for tool_call in &response.tool_calls {
                        store.append(EventPayload::ToolCall {
                            turn,
                            ts: now_timestamp()?,
                            tool_call_id: tool_call.id.clone(),
                            request_id: request_id.clone(),
                            index: tool_call.index,
                            tool: tool_call.name.clone(),
                            args: tool_call.args.clone(),
                        })?;
                    }

                    for tool_call in &response.tool_calls {
                        if let Some(definition) =
                            registry.iter().find(|tool| tool.name == tool_call.name)
                        {
                            if definition.risk == "edit" || definition.risk == "command" {
                                store.append(EventPayload::PermissionRequest {
                                    turn,
                                    ts: now_timestamp()?,
                                    tool_call_id: tool_call.id.clone(),
                                    tool: tool_call.name.clone(),
                                    risk: definition.risk.clone(),
                                    summary: format!(
                                        "{} {}",
                                        tool_call.name,
                                        serde_json::to_string(&tool_call.args)
                                            .unwrap_or_else(|_| "{}".to_string())
                                    ),
                                })?;
                                store.append(EventPayload::PermissionDecision {
                                    turn,
                                    ts: now_timestamp()?,
                                    tool_call_id: tool_call.id.clone(),
                                    decision: "deny".to_string(),
                                    scope: "once".to_string(),
                                    source: "runtime".to_string(),
                                    rule: "permission_gate_unavailable".to_string(),
                                })?;
                            }
                        }
                    }

                    for tool_call in &response.tool_calls {
                        let prior_events = EventStore::replay(&events_path)?;
                        let result_event_id = store.next_id();
                        let result = tool::dispatch(
                            &tool_call.name,
                            &tool_call.args,
                            &workspace,
                            &prior_events,
                            result_event_id,
                            Some(&tool_call.id),
                        )
                        .await;
                        store.append(EventPayload::ToolResult {
                            turn,
                            ts: now_timestamp()?,
                            tool_call_id: tool_call.id.clone(),
                            status: result.status,
                            summary: result.summary,
                            model_content: result.model_content,
                            truncated: result.truncated,
                            structured: result.structured,
                        })?;
                    }
                }
                Err(failure) => {
                    let mut store = EventStore::open(&events_path)?;
                    store.append(EventPayload::ModelError {
                        turn,
                        ts: now_timestamp()?,
                        request_id: request_id.clone(),
                        kind: provider_failure_kind(&failure.kind).to_string(),
                        message: failure.message.clone(),
                        status: failure.status,
                        retryable: Some(failure.retryable),
                        resolved_provider: Some(provider_name.to_string()),
                        resolved_model: Some(resolved.model.clone()),
                    })?;
                    store.append(EventPayload::TurnEnd {
                        turn,
                        ts: now_timestamp()?,
                    })?;
                    return Err(Error::Provider(failure.message));
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
        Ok(self.done.take().map(|output| UiEvent::Done { output }))
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

fn current_turn(events: &[StoredEvent]) -> Result<u64> {
    events
        .iter()
        .rev()
        .find_map(|event| match &event.payload {
            EventPayload::TurnStart { turn, .. } => Some(*turn),
            _ => None,
        })
        .ok_or_else(|| Error::InvalidEventStream("missing turn.start".to_string()))
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

fn now_timestamp() -> Result<String> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}
