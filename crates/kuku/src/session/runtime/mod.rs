//! Session-owned execution, durable input delivery, and resumable observations.

mod boundary;
mod controls;
mod creation;
mod driver;
mod observe;
mod receipts;
mod record;
mod selection;
mod types;

pub(crate) use selection::resolve_agent_model;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::{broadcast, watch, Semaphore};
use tokio::task::JoinHandle;

use crate::event::{EventStore, StoredEvent};
use crate::{Error, Result};

pub use crate::tool::builtin::run_command::output::{
    OutputChunk, OutputRead, OutputSegment, OutputStream,
};
pub use observe::{Observation, SessionObserver};
pub use types::{
    Activation, AgentConfig, AgentSnapshot, AgentStatus, AgentView, Input, InputSource,
    InputStatus, InputTiming, PendingInput, QuestionRequest, Receipt, RuntimeLimits,
    SessionOptions, UsageTotals, Withdrawal,
};

use record::{fingerprint, validate_address, validate_key, Command, Record, State};

type CleanupOutcome = Option<std::result::Result<(), String>>;

pub(crate) struct SessionInner {
    id: String,
    options: SessionOptions,
    events_path: PathBuf,
    writer: Mutex<Option<EventStore>>,
    state: Mutex<State>,
    workers: Mutex<HashMap<u64, Worker>>,
    settlement_failure: OnceLock<String>,
    live: broadcast::Sender<observe::LiveObservation>,
    model_capacity: Arc<Semaphore>,
    tool_adapters: Arc<std::collections::BTreeMap<String, crate::adapter::RegisteredTool>>,
    model_request_budget: Option<Arc<std::sync::atomic::AtomicU64>>,
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
    pub async fn open(options: SessionOptions) -> Result<Self> {
        Self::open_runtime(options, None)
    }

    fn open_runtime(
        mut options: SessionOptions,
        creation: Option<(Command, &AgentConfig)>,
    ) -> Result<Self> {
        validate_limits(&options.limits)?;
        let tool_adapters = Arc::new(selection::register_tools(&options)?);
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
        if let Some((_, config)) = &creation {
            if EventStore::read_head(&events_path)? == 0 {
                selection::validate_initial_model(&options, config)?;
            }
        }
        let lease = SessionLease::acquire(events_path.clone())?;
        let mut writer = EventStore::open(&events_path)?;
        let mut state = State::default();
        writer.visit(|event| reduce_event(&mut state, event))?;
        creation::prepare_journal(
            &mut writer,
            &mut state,
            &id,
            creation.map(|(command, _)| command),
            options.prompts_dir.as_deref(),
        )?;
        if let Some(claim) = &state.creation {
            options.prompts_dir = claim.prompts_dir.clone();
        }
        state.mark_interrupted();
        let (live, _) = broadcast::channel(options.limits.observation_capacity);
        let model_capacity = Arc::new(Semaphore::new(options.limits.max_model_requests));
        let model_request_budget = options
            .limits
            .max_requests_per_open
            .map(|limit| Arc::new(std::sync::atomic::AtomicU64::new(limit)));
        Ok(Self {
            inner: Arc::new(SessionInner {
                id,
                options,
                events_path,
                writer: Mutex::new(Some(writer)),
                state: Mutex::new(state),
                workers: Mutex::new(HashMap::new()),
                settlement_failure: OnceLock::new(),
                live,
                model_capacity,
                tool_adapters,
                model_request_budget,
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
        let config = self.resolve_defaults(address, config)?;
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
        let config = self.resolve_defaults(address, config)?;
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
        self.send_input(input, None)
    }

    /// Commit a configuration and input together, using the input correlation to identify the configuration intent.
    pub fn send_configured(&self, input: Input, config: AgentConfig) -> Result<Receipt> {
        if input.correlation.as_deref().is_none_or(str::is_empty) {
            return Err(Error::InvalidArgument(
                "configured delivery requires an input correlation for its configuration intent"
                    .into(),
            ));
        }
        self.send_input(input, Some(config))
    }

    fn send_input(&self, input: Input, config: Option<AgentConfig>) -> Result<Receipt> {
        let digest = receipts::input_fingerprint(&input)?;
        if input.text.len() > self.inner.options.limits.max_input_bytes {
            return Err(Error::InvalidArgument(
                "input exceeds the configured byte limit".into(),
            ));
        }
        let target = input.target.clone();
        let wake = input.activation == Activation::Wake;
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        if let Some(receipt) = state.retry(&input.command_id, &digest)? {
            if wake {
                self.activate_delivery(&mut state, &target, &receipt)?;
            }
            return Ok(receipt);
        }
        if state.agent(&target)?.inbox.len() >= self.inner.options.limits.max_pending_inputs {
            return Err(Error::InvalidArgument(format!(
                "agent '{target}' inbox limit reached"
            )));
        }
        let config = config
            .map(|config| {
                let config = self.resolve_defaults(&target, config)?;
                self.validate_config(&config)?;
                Ok::<_, Error>(config)
            })
            .transpose()?;
        if input.bootstrap_skill.is_some()
            && config
                .as_ref()
                .unwrap_or(&state.agent(&target)?.snapshot.config)
                .no_skills
        {
            return Err(Error::InvalidArgument(format!(
                "skills are disabled for agent '{target}'"
            )));
        }
        let key = input.command_id.clone();
        self.commit(
            &mut state,
            Record::InputDelivered {
                fingerprint: digest,
                input,
                config,
            },
        )?;
        let receipt = state.commands[&key].1.clone();
        if wake {
            self.activate_delivery(&mut state, &target, &receipt)?;
        }
        Ok(receipt)
    }

    fn activate_delivery(&self, state: &mut State, address: &str, receipt: &Receipt) -> Result<()> {
        if state.input_status(address, &receipt.command_id)? != InputStatus::Queued {
            return Ok(());
        }
        let agent = state.agent(address)?;
        if agent.snapshot.status == AgentStatus::Stopped
            && agent.active_epoch.is_none()
            && agent.stopped_at.is_none_or(|stop| receipt.event_id > stop)
            && agent.inbox.iter().any(|entry| {
                entry.receipt.event_id == receipt.event_id
                    && matches!(entry.input.source, InputSource::Host(_))
            })
        {
            self.resume(state, address, None)?;
        }
        self.schedule_locked(state, address)
    }

    /// Return an agent's derived state without affecting execution or observations.
    pub fn agent(&self, address: &str) -> Result<AgentSnapshot> {
        Ok(self
            .inner
            .state
            .lock()
            .unwrap()
            .agent(address)?
            .snapshot
            .clone())
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
        let (state, _) = load_state(events_path)?;
        Ok(state
            .agents
            .into_values()
            .map(|agent| agent.snapshot)
            .collect())
    }

    /// Read retained command output by bounded range, search, or incremental cursor.
    pub fn read_output(&self, output_id: &str, request: &OutputRead) -> Result<OutputChunk> {
        Self::read_saved_output(&self.inner.events_path, output_id, request)
    }

    /// Read saved output without opening model or runtime resources.
    pub fn read_saved_output(
        events_path: &Path,
        output_id: &str,
        request: &OutputRead,
    ) -> Result<OutputChunk> {
        Ok(crate::tool::builtin::run_command::output::read_output(
            &events_path
                .parent()
                .ok_or_else(|| {
                    Error::InvalidArgument("events path has no session directory".into())
                })?
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

    /// Read a bounded chronological page strictly before a cursor, or the latest page.
    pub fn events_before(&self, before: Option<u64>, limit: usize) -> Result<Vec<StoredEvent>> {
        EventStore::read_page_before(&self.inner.events_path, before, limit)
    }

    /// Subscribe with a durable cursor and a bounded transient streaming channel.
    pub fn observe(&self, after: u64) -> SessionObserver {
        SessionObserver::new(self.clone(), after)
    }

    /// Reply once to the exact pending authorization and resume its bound action.
    pub fn reply_permission(
        &self,
        command_id: &str,
        address: &str,
        request_id: &str,
        allow: bool,
    ) -> Result<Receipt> {
        let digest = receipts::permission_fingerprint(command_id, address, request_id, allow)?;
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        if let Some(receipt) = state.retry(command_id, &digest)? {
            if state
                .agent(address)?
                .permission_reply
                .as_ref()
                .is_some_and(|(pending, _)| pending == request_id)
            {
                self.schedule_locked(&mut state, address)?;
            }
            return Ok(receipt);
        }
        let agent = state.agent(address)?;
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
        self.schedule_locked(&mut state, address)?;
        Ok(state.commands[command_id].1.clone())
    }

    /// Deliver a reply only to the matching still-pending question.
    pub fn reply_question(
        &self,
        command_id: &str,
        address: &str,
        request_id: &str,
        text: impl Into<String>,
    ) -> Result<Receipt> {
        let text = text.into();
        if text.len() > self.inner.options.limits.max_input_bytes {
            return Err(Error::InvalidArgument(
                "question reply exceeds the configured input byte limit".into(),
            ));
        }
        let digest = receipts::question_fingerprint(command_id, address, request_id, &text)?;
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        if let Some(receipt) = state.retry(command_id, &digest)? {
            self.activate_delivery(&mut state, address, &receipt)?;
            return Ok(receipt);
        }
        let agent = state.agent(address)?;
        if !matches!(
            agent.snapshot.status,
            AgentStatus::Running | AgentStatus::Waiting
        ) || agent
            .snapshot
            .question
            .as_ref()
            .is_none_or(|question| question.id != request_id)
        {
            return Err(Error::QuestionRequestNotPending(request_id.into()));
        }
        if agent.inbox.len() >= self.inner.options.limits.max_pending_inputs {
            return Err(Error::InvalidArgument(format!(
                "agent '{address}' inbox limit reached"
            )));
        }
        let mut input = Input::new(command_id, address, text);
        input.correlation = Some(request_id.into());
        input.timing = InputTiming::NextTurn;
        self.commit(
            &mut state,
            Record::QuestionReplied {
                fingerprint: digest,
                request_id: request_id.into(),
                input,
            },
        )?;
        self.schedule_locked(&mut state, address)?;
        Ok(state.commands[command_id].1.clone())
    }

    pub(crate) fn request_question(
        &self,
        address: &str,
        execution: u64,
        turn: u64,
        tool_call_id: &str,
        text: &str,
    ) -> Result<QuestionRequest> {
        let mut state = self.inner.state.lock().unwrap();
        let agent = self.executing_agent(&state, address, execution)?;
        if text.is_empty() || text.len() > self.inner.options.limits.max_input_bytes {
            return Err(Error::InvalidArgument(
                "question must be nonempty and within the input byte limit".into(),
            ));
        }
        if agent.snapshot.question.is_some() {
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
                        let outcome = session.finish_close().await;
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

    async fn finish_close(&self) -> std::result::Result<(), String> {
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
            state.closed_cursor = writer.last_id();
            writer.changed().notify_waiters();
        }
        self.inner.lease.lock().unwrap().take();
        match (
            self.inner.settlement_failure.get(),
            failure.map(|error| error.to_string()),
        ) {
            (Some(settlement), Some(other)) if settlement != &other => {
                Err(format!("{settlement}\n{other}"))
            }
            (Some(settlement), _) => Err(settlement.clone()),
            (None, Some(error)) => Err(error),
            (None, None) => Ok(()),
        }
    }

    fn remember_settlement_failure(&self, error: Error) -> Error {
        self.inner
            .settlement_failure
            .get_or_init(|| error.to_string());
        error
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
}

fn ensure_open(state: &State) -> Result<()> {
    if state.closing || state.closed {
        return Err(Error::InvalidArgument(
            "session resources are closing or closed".into(),
        ));
    }
    Ok(())
}

fn load_state(events_path: &Path) -> Result<(State, u64)> {
    let mut state = State::default();
    let cursor = EventStore::visit_path(events_path, |event| reduce_event(&mut state, event))?;
    state.mark_interrupted();
    Ok((state, cursor))
}

fn reduce_event(state: &mut State, event: StoredEvent) -> Result<()> {
    if let Some(record) = Record::from_event(&event)? {
        state.reduce(event.id, record)?;
    }
    Ok(())
}

fn validate_limits(limits: &RuntimeLimits) -> Result<()> {
    if limits.max_requests_per_open == Some(0) || limits.max_output_tokens_per_request == Some(0) {
        return Err(Error::InvalidArgument(
            "optional model request and output-token limits must be positive".into(),
        ));
    }
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
