//! Session-owned execution, durable input delivery, and resumable observations.

mod driver;
mod observe;
mod receipts;
mod record;
mod types;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::{broadcast, watch, Semaphore};
use tokio::task::JoinHandle;

use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::query::UiEvent;
use crate::{Error, Result};

pub use crate::tool::builtin::run_command::output::{
    OutputChunk, OutputRead, OutputSegment, OutputStream,
};
pub use observe::{Observation, SessionObserver};
pub use types::{
    Activation, AgentConfig, AgentSnapshot, AgentStatus, Input, InputSource, QuestionRequest,
    Receipt, RuntimeLimits, SessionOptions, UsageTotals,
};

use record::{fingerprint, timestamp, validate_address, validate_key, Record, State};

type CleanupOutcome = Option<std::result::Result<(), String>>;

pub(crate) struct SessionInner {
    id: String,
    options: SessionOptions,
    events_path: PathBuf,
    writer: Mutex<Option<EventStore>>,
    state: Mutex<State>,
    workers: Mutex<HashMap<u64, Worker>>,
    live: broadcast::Sender<(String, UiEvent)>,
    model_capacity: Arc<Semaphore>,
    lease: Mutex<Option<SessionLease>>,
    close_result: Mutex<Option<watch::Receiver<CleanupOutcome>>>,
}

struct SessionLease(PathBuf);

impl SessionLease {
    fn acquire(path: PathBuf) -> Result<Self> {
        let mut leases = session_leases().lock().unwrap();
        if !leases.insert(path.clone()) {
            return Err(Error::SessionLocked {
                session: path,
                holder_pid: std::process::id() as i32,
            });
        }
        Ok(Self(path))
    }
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        session_leases().lock().unwrap().remove(&self.0);
    }
}

fn session_leases() -> &'static Mutex<HashSet<PathBuf>> {
    static LEASES: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    LEASES.get_or_init(|| Mutex::new(HashSet::new()))
}

struct Worker {
    address: String,
    cancel: watch::Sender<bool>,
    done: watch::Receiver<CleanupOutcome>,
    join: JoinHandle<Result<()>>,
}

/// Open runtime resources for one persistent group of independent agents.
#[derive(Clone)]
pub struct Session {
    pub(crate) inner: Arc<SessionInner>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.inner.id)
            .field("events_path", &self.inner.events_path)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// Acquire the session writer and reconstruct committed facts without automatically running agents.
    pub async fn open(mut options: SessionOptions) -> Result<Self> {
        validate_limits(&options.limits)?;
        options.workspace = std::fs::canonicalize(&options.workspace)
            .map_err(|_| Error::InvalidWorkspacePath(options.workspace.display().to_string()))?;
        crate::util::fs_sync::create_dir_all(&options.data_dir)?;
        options.data_dir = std::fs::canonicalize(&options.data_dir)?;
        let id = options
            .session_id
            .clone()
            .unwrap_or_else(super::new_session_id);
        super::validate_session_id(&id)?;
        let events_path = super::session_events_path(&options.data_dir, &options.workspace, &id)?;
        let lease = SessionLease::acquire(events_path.clone())?;
        let mut writer = EventStore::open(&events_path)?;
        let mut state = State::default();
        let mut cursor = 0;
        loop {
            let events = writer.read_after(cursor, 128)?;
            if events.is_empty() {
                break;
            }
            for event in events {
                cursor = event.id;
                if let Some(record) = Record::from_event(&event)? {
                    state.reduce(event.id, record)?;
                }
            }
        }
        if cursor == 0 {
            let ts = timestamp()?;
            writer.append(EventPayload::SessionCreated {
                ts: ts.clone(),
                schema_version: 3,
                session_id: id.clone(),
                created_at: ts,
                kuku_version: env!("CARGO_PKG_VERSION").into(),
            })?;
        }
        for agent in state.agents.values_mut() {
            if agent.snapshot.status == AgentStatus::Running {
                agent.snapshot.status = AgentStatus::Interrupted;
                agent.snapshot.last_error = Some("execution interrupted before a durable settlement; explicitly advance after reconciling side effects".into());
            }
        }
        let (live, _) = broadcast::channel(options.limits.observation_capacity);
        let model_capacity = Arc::new(Semaphore::new(options.limits.max_model_requests));
        Ok(Self {
            inner: Arc::new(SessionInner {
                id,
                options,
                events_path,
                writer: Mutex::new(Some(writer)),
                state: Mutex::new(state),
                workers: Mutex::new(HashMap::new()),
                live,
                model_capacity,
                lease: Mutex::new(Some(lease)),
                close_result: Mutex::new(None),
            }),
        })
    }

    /// Return the stable identity used when reopening this session.
    pub fn id(&self) -> &str {
        &self.inner.id
    }

    /// Return the authoritative append-only session journal.
    pub fn events_path(&self) -> &Path {
        &self.inner.events_path
    }

    /// Create one stable address with independent context and an idempotent receipt.
    pub fn create_agent(
        &self,
        command_id: &str,
        address: &str,
        config: AgentConfig,
    ) -> Result<Receipt> {
        validate_key(command_id)?;
        validate_address(address)?;
        let digest = fingerprint(&("create_agent", address, &config))?;
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        if let Some(receipt) = state.retry(command_id, &digest)? {
            return Ok(receipt);
        }
        if state.agents.contains_key(address) {
            return Err(Error::InvalidArgument(format!(
                "agent address '{address}' is already retained by this session"
            )));
        }
        if state.agents.len() >= self.inner.options.limits.max_agents {
            return Err(Error::InvalidArgument("session agent limit reached".into()));
        }
        self.validate_config(&config)?;
        self.commit(
            &mut state,
            Record::AgentCreated {
                command_id: command_id.into(),
                fingerprint: digest,
                address: address.into(),
                config,
            },
        )?;
        Ok(state.commands[command_id].1.clone())
    }

    /// Change only the selected agent's defaults at its next safe boundary.
    pub fn configure_agent(
        &self,
        command_id: &str,
        address: &str,
        config: AgentConfig,
    ) -> Result<Receipt> {
        validate_key(command_id)?;
        let digest = fingerprint(&("configure_agent", address, &config))?;
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        if let Some(receipt) = state.retry(command_id, &digest)? {
            return Ok(receipt);
        }
        state.agent_mut(address)?;
        self.validate_config(&config)?;
        self.commit(
            &mut state,
            Record::AgentConfigured {
                command_id: command_id.into(),
                fingerprint: digest,
                address: address.into(),
                config,
            },
        )?;
        Ok(state.commands[command_id].1.clone())
    }

    /// Persist delivery before acknowledging it and optionally wake an idle recipient.
    pub fn send(&self, input: Input) -> Result<Receipt> {
        validate_key(&input.command_id)?;
        validate_address(&input.target)?;
        if input.text.len() > self.inner.options.limits.max_input_bytes {
            return Err(Error::InvalidArgument(
                "input exceeds the configured byte limit".into(),
            ));
        }
        let identity = match &input.source {
            InputSource::Host(identity)
            | InputSource::Agent(identity)
            | InputSource::External(identity) => identity,
        };
        if identity.len() > 256
            || input
                .correlation
                .as_ref()
                .is_some_and(|value| value.len() > 256)
        {
            return Err(Error::InvalidArgument(
                "input source or correlation exceeds 256 bytes".into(),
            ));
        }
        let digest = receipts::input_fingerprint(&input)?;
        let target = input.target.clone();
        let wake = input.activation == Activation::Wake;
        let receipt = {
            let mut state = self.inner.state.lock().unwrap();
            ensure_open(&state)?;
            if let Some(receipt) = state.retry(&input.command_id, &digest)? {
                drop(state);
                if wake {
                    self.schedule(&target)?;
                }
                return Ok(receipt);
            }
            if state.agent_mut(&target)?.inbox.len() >= self.inner.options.limits.max_pending_inputs
            {
                return Err(Error::InvalidArgument(format!(
                    "agent '{target}' inbox limit reached"
                )));
            }
            let key = input.command_id.clone();
            self.commit(
                &mut state,
                Record::InputDelivered {
                    fingerprint: digest,
                    input,
                },
            )?;
            state.commands[&key].1.clone()
        };
        if wake {
            self.schedule(&target)?;
        }
        Ok(receipt)
    }

    /// Explicitly authorize advancement of pending or interrupted work.
    pub fn advance(&self, address: &str) -> Result<()> {
        {
            let mut state = self.inner.state.lock().unwrap();
            ensure_open(&state)?;
            let status = state.agent_mut(address)?.snapshot.status;
            if status == AgentStatus::Stopped && state.agent_mut(address)?.active_epoch.is_some() {
                return Err(Error::InvalidArgument(format!(
                    "agent '{address}' cleanup is still in progress"
                )));
            }
            if matches!(status, AgentStatus::Stopped | AgentStatus::Interrupted) {
                self.commit(
                    &mut state,
                    Record::AgentResumed {
                        address: address.into(),
                    },
                )?;
            }
        }
        self.schedule(address)
    }

    /// Return an agent's derived state without affecting execution or observations.
    pub fn agent(&self, address: &str) -> Result<AgentSnapshot> {
        let state = self.inner.state.lock().unwrap();
        state
            .agents
            .get(address)
            .map(|agent| agent.snapshot.clone())
            .ok_or_else(|| {
                Error::InvalidArgument(format!("agent '{address}' does not exist in this session"))
            })
    }

    /// Return all retained agents in stable address order.
    pub fn agents(&self) -> Vec<AgentSnapshot> {
        self.inner
            .state
            .lock()
            .unwrap()
            .agents
            .values()
            .map(|agent| agent.snapshot.clone())
            .collect()
    }

    /// Reconstruct retained agent snapshots without opening model or runtime resources.
    pub fn read_agents(events_path: &Path) -> Result<Vec<AgentSnapshot>> {
        let mut state = State::default();
        let mut cursor = 0;
        loop {
            let page = EventStore::read_page(events_path, cursor, 128)?;
            if page.is_empty() {
                break;
            }
            for event in page {
                cursor = event.id;
                if let Some(record) = Record::from_event(&event)? {
                    state.reduce(event.id, record)?;
                }
            }
        }
        Ok(state
            .agents
            .into_values()
            .map(|mut agent| {
                if agent.snapshot.status == AgentStatus::Running {
                    agent.snapshot.status = AgentStatus::Interrupted;
                }
                agent.snapshot
            })
            .collect())
    }

    /// Read retained command output by bounded range, search, or incremental cursor.
    pub fn read_output(&self, output_id: &str, request: &OutputRead) -> Result<OutputChunk> {
        Ok(crate::tool::builtin::run_command::output::read_output(
            &self
                .inner
                .events_path
                .parent()
                .expect("events path has session parent")
                .join("outputs"),
            output_id,
            request.clone(),
        )?)
    }

    /// Read a bounded page of durable events after a reconnectable cursor.
    pub fn events(&self, after: u64, limit: usize) -> Result<Vec<StoredEvent>> {
        let writer = self.inner.writer.lock().unwrap();
        match writer.as_ref() {
            Some(writer) => writer.read_after(after, limit),
            None => EventStore::read_page(&self.inner.events_path, after, limit),
        }
    }

    /// Subscribe with a durable cursor and a bounded transient streaming channel.
    pub fn observe(&self, after: u64) -> SessionObserver {
        SessionObserver::new(self.clone(), after)
    }

    /// Stop the selected agent, clean up its active work, and retain pending inputs.
    pub async fn cancel(&self, address: &str) -> Result<()> {
        let (done, mut failure) = {
            let mut state = self.inner.state.lock().unwrap();
            ensure_open(&state)?;
            let agent = state.agent_mut(address)?;
            let permission = if agent.active_epoch.is_none() {
                agent.snapshot.permission.clone()
            } else {
                None
            };
            let mut failure = permission
                .as_ref()
                .and_then(|request| self.cancel_waiting_action(request).err());
            if let Err(error) = self.commit(
                &mut state,
                Record::AgentStopped {
                    address: address.into(),
                },
            ) {
                failure.get_or_insert(error);
                state.agent_mut(address)?.snapshot.status = AgentStatus::Stopped;
            }
            let epoch = state.agent_mut(address)?.active_epoch;
            let workers = self.inner.workers.lock().unwrap();
            let done = epoch.and_then(|epoch| workers.get(&epoch)).map(|worker| {
                let _ = worker.cancel.send(true);
                worker.done.clone()
            });
            (done, failure)
        };
        if let Some(mut done) = done {
            loop {
                if let Some(result) = done.borrow().clone() {
                    if let Err(error) = result {
                        failure.get_or_insert(Error::InvalidEventStream(error));
                    }
                    break;
                }
                done.changed().await.map_err(|_| {
                    Error::InvalidEventStream(format!(
                        "agent '{address}' cleanup task ended without a result"
                    ))
                })?;
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        if let Some(message) = self
            .inner
            .state
            .lock()
            .unwrap()
            .cleanup_failures
            .get(address)
        {
            return Err(Error::InvalidEventStream(message.clone()));
        }
        Ok(())
    }

    fn cancel_waiting_action(&self, request: &crate::query::PermissionRequest) -> Result<()> {
        let mut writer = self.inner.writer.lock().unwrap();
        let writer = writer
            .as_mut()
            .ok_or_else(|| Error::InvalidArgument("session resources are closed".into()))?;
        writer.append(EventPayload::ToolResult {
            turn: request.turn,
            ts: timestamp()?,
            conversation: Some(request.conversation.as_str().into()),
            tool_call_id: request.tool_call_id.clone(),
            status: "cancelled".into(),
            summary: "pending action cancelled before dispatch".into(),
            model_content: "pending action cancelled before dispatch".into(),
            truncated: false,
            files_read: Vec::new(),
            files_changed: Vec::new(),
            commands_run: Vec::new(),
            memory_changed: None,
            structured: Some(serde_json::json!({"kind":"cancelled"})),
        })?;
        Ok(())
    }

    /// Reply once to the exact pending authorization and resume its bound action.
    pub fn reply_permission(
        &self,
        command_id: &str,
        address: &str,
        request_id: &str,
        allow: bool,
    ) -> Result<Receipt> {
        validate_key(command_id)?;
        let digest = receipts::permission_fingerprint(command_id, address, request_id, allow)?;
        let receipt = {
            let mut state = self.inner.state.lock().unwrap();
            ensure_open(&state)?;
            if let Some(receipt) = state.retry(command_id, &digest)? {
                drop(state);
                self.schedule(address)?;
                return Ok(receipt);
            }
            let agent = state.agent_mut(address)?;
            if agent.snapshot.status != AgentStatus::Waiting
                || agent
                    .snapshot
                    .permission
                    .as_ref()
                    .is_none_or(|pending| pending.id != request_id)
            {
                return Err(Error::PermissionRequestNotPending(request_id.into()));
            }
            self.commit(
                &mut state,
                Record::PermissionReplied {
                    command_id: command_id.into(),
                    fingerprint: digest,
                    address: address.into(),
                    request_id: request_id.into(),
                    allow,
                },
            )?;
            state.commands[command_id].1.clone()
        };
        self.schedule(address)?;
        Ok(receipt)
    }

    /// Deliver a reply only to the matching still-pending question.
    pub fn reply_question(
        &self,
        command_id: &str,
        address: &str,
        request_id: &str,
        text: impl Into<String>,
    ) -> Result<Receipt> {
        validate_key(command_id)?;
        let text = text.into();
        if text.len() > self.inner.options.limits.max_input_bytes {
            return Err(Error::InvalidArgument(
                "question reply exceeds the configured input byte limit".into(),
            ));
        }
        let digest = receipts::question_fingerprint(command_id, address, request_id, &text)?;
        let receipt = {
            let mut state = self.inner.state.lock().unwrap();
            ensure_open(&state)?;
            if let Some(receipt) = state.retry(command_id, &digest)? {
                drop(state);
                self.schedule(address)?;
                return Ok(receipt);
            }
            let agent = state.agent_mut(address)?;
            if agent
                .snapshot
                .question
                .as_ref()
                .is_none_or(|question| question.id != request_id)
            {
                return Err(Error::InvalidArgument(format!(
                    "question '{request_id}' is no longer pending for agent '{address}'"
                )));
            }
            if agent.inbox.len() >= self.inner.options.limits.max_pending_inputs {
                return Err(Error::InvalidArgument(format!(
                    "agent '{address}' inbox limit reached"
                )));
            }
            let mut input = Input::new(command_id, address, text);
            input.correlation = Some(request_id.into());
            self.commit(
                &mut state,
                Record::QuestionReplied {
                    fingerprint: digest,
                    request_id: request_id.into(),
                    input,
                },
            )?;
            state.commands[command_id].1.clone()
        };
        self.schedule(address)?;
        Ok(receipt)
    }

    pub(crate) fn request_question(
        &self,
        address: &str,
        turn: u64,
        tool_call_id: &str,
        text: &str,
    ) -> Result<QuestionRequest> {
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        if text.is_empty() || text.len() > self.inner.options.limits.max_input_bytes {
            return Err(Error::InvalidArgument(
                "question must be nonempty and within the input byte limit".into(),
            ));
        }
        if state.agent_mut(address)?.snapshot.question.is_some() {
            return Err(Error::InvalidArgument(format!(
                "agent '{address}' already has a pending question"
            )));
        }
        let request = QuestionRequest {
            id: format!("question:{address}:{turn}:{tool_call_id}"),
            agent: address.into(),
            turn,
            tool_call_id: tool_call_id.into(),
            text: text.into(),
        };
        self.commit(
            &mut state,
            Record::QuestionRequested {
                address: address.into(),
                request: request.clone(),
            },
        )?;
        Ok(request)
    }

    /// Await all cleanup and durable writes before releasing the session writer.
    pub async fn close(&self) -> Result<()> {
        let mut result = {
            let mut close = self.inner.close_result.lock().unwrap();
            match &*close {
                Some(result) => result.clone(),
                None => {
                    self.inner.state.lock().unwrap().closing = true;
                    let (complete, result) = watch::channel(None);
                    *close = Some(result.clone());
                    let session = self.clone();
                    tokio::spawn(async move {
                        let outcome = session
                            .finish_close()
                            .await
                            .map_err(|error| error.to_string());
                        let _ = complete.send(Some(outcome));
                    });
                    result
                }
            }
        };
        loop {
            if let Some(outcome) = result.borrow().clone() {
                return outcome.map_err(Error::InvalidEventStream);
            }
            result.changed().await.map_err(|_| {
                Error::InvalidEventStream("session cleanup task ended without a result".into())
            })?;
        }
    }

    async fn finish_close(&self) -> Result<()> {
        let workers = std::mem::take(&mut *self.inner.workers.lock().unwrap());
        for worker in workers.values() {
            let _ = worker.cancel.send(true);
        }
        let mut failure = None;
        for (_, worker) in workers {
            let address = worker.address;
            match worker.join.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                }
                Err(error) => {
                    failure.get_or_insert(Error::InvalidEventStream(format!(
                        "agent '{address}' close failed: {error}"
                    )));
                }
            }
        }
        let mut state = self.inner.state.lock().unwrap();
        if let Some(message) = state.cleanup_failures.values().next() {
            failure.get_or_insert_with(|| Error::InvalidEventStream(message.clone()));
        }
        if let Err(error) = self.commit(&mut state, Record::SessionClosed) {
            failure.get_or_insert(error);
        }
        state.closed = true;
        if let Some(writer) = self.inner.writer.lock().unwrap().take() {
            writer.changed().notify_waiters();
        }
        self.inner.lease.lock().unwrap().take();
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn commit(&self, state: &mut State, record: Record) -> Result<u64> {
        let mut writer = self.inner.writer.lock().unwrap();
        let writer = writer
            .as_mut()
            .ok_or_else(|| Error::InvalidArgument("session resources are closed".into()))?;
        let event = writer.append(record.payload()?)?;
        state.reduce(event.id, record)?;
        Ok(event.id)
    }

    fn validate_config(&self, config: &AgentConfig) -> Result<()> {
        if config
            .instructions
            .as_ref()
            .is_some_and(|value| value.len() > 65_536)
            || [
                &config.preset,
                &config.connection,
                &config.tier,
                &config.model,
                &config.reasoning_effort,
                &config.service_tier,
                &config.bootstrap_skill,
            ]
            .iter()
            .any(|value| value.as_ref().is_some_and(|value| value.len() > 512))
        {
            return Err(Error::InvalidArgument(
                "agent instructions or model selection exceeds its byte limit".into(),
            ));
        }
        if config.max_output_tokens == Some(0) {
            return Err(Error::InvalidArgument(
                "max_output_tokens must be positive".into(),
            ));
        }
        if let Some(workspace) = &config.workspace {
            std::fs::canonicalize(workspace)
                .map_err(|_| Error::InvalidWorkspacePath(workspace.display().to_string()))?;
        }
        crate::provider::config::resolve_config(crate::provider::config::ResolveConfigInput {
            connection: config.connection.clone(),
            model: config.model.clone(),
            tier: config.tier.clone(),
            reasoning_effort: config.reasoning_effort.clone(),
            service_tier: config.service_tier.clone(),
            max_output_tokens: config.max_output_tokens,
            config: Some(self.inner.options.config.clone()),
            ..Default::default()
        })?;
        Ok(())
    }
}

fn ensure_open(state: &State) -> Result<()> {
    if state.closing || state.closed {
        return Err(Error::InvalidArgument(
            "session resources are closing or closed".into(),
        ));
    }
    Ok(())
}

fn validate_limits(limits: &RuntimeLimits) -> Result<()> {
    if limits.max_request_bytes < 4096
        || limits.max_request_bytes > 16 * 1024 * 1024
        || limits.max_preview_bytes < 1024
    {
        return Err(Error::InvalidArgument("model request byte limit must be 4096..=16777216 and output preview must retain at least 1024 bytes".into()));
    }
    if limits.max_agents == 0
        || limits.max_agents > 4096
        || limits.max_pending_inputs == 0
        || limits.max_pending_inputs > 4096
        || limits.max_input_bytes == 0
        || limits.max_input_bytes > 262_144
        || limits.max_model_requests == 0
        || limits.max_model_requests > 256
        || limits.max_steps_per_turn == 0
        || limits.max_steps_per_turn > 1024
        || limits.max_tools_per_agent == 0
        || limits.max_tools_per_agent > 128
        || limits.max_output_bytes < 4096
        || limits.max_output_bytes > 1024 * 1024 * 1024
        || limits.max_preview_bytes == 0
        || limits.max_preview_bytes > 65_536
        || limits.observation_capacity == 0
        || limits.observation_capacity > 4096
    {
        return Err(Error::InvalidArgument(
            "runtime limits are zero or outside the supported bounds".into(),
        ));
    }
    Ok(())
}
