//! Thin HTTP run ownership and bounded observation over the public Session SDK.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use kuku::session::{
    AgentConfig, AgentStatus, Input, InputSource, Observation, Session, SessionOptions,
};
use tokio::sync::{broadcast, watch, Mutex as TokioMutex, OwnedSemaphorePermit, Semaphore};

const RECENT_EVENT_LIMIT: usize = 200;
const RECENT_BYTE_LIMIT: usize = 1024 * 1024;

#[derive(Default)]
struct RecentEvents {
    lines: VecDeque<String>,
    bytes: usize,
}

pub(crate) enum HostReply {
    Permission(bool),
    Question(String),
}

/// Host controls and bounded observations for one admitted Agent run.
pub struct RunHandle {
    session: Session,
    agent: String,
    /// Canonical working directory of the Session.
    pub workspace: PathBuf,
    done: watch::Receiver<Option<Result<(), String>>>,
    permit: Arc<Mutex<Option<OwnedSemaphorePermit>>>,
    recent_events: Arc<Mutex<RecentEvents>>,
}

struct OpenSession {
    session: Session,
    runs: HashSet<String>,
}

/// HTTP admission and resource ownership over the shared Session runtime.
pub struct RunManager {
    runs: Arc<Mutex<HashMap<String, RunHandle>>>,
    sessions: Arc<TokioMutex<HashMap<PathBuf, OpenSession>>>,
    semaphore: Arc<Semaphore>,
}

impl RunManager {
    /// Limit concurrently advancing HTTP submissions independently of observers.
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            runs: Arc::new(Mutex::new(HashMap::new())),
            sessions: Arc::new(TokioMutex::new(HashMap::new())),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
        }
    }

    /// Persist one input and return a bounded subscription to its Session execution.
    pub async fn spawn_run(
        &mut self,
        mut options: SessionOptions,
        address: String,
        config: AgentConfig,
        prompt: String,
        command_id: Option<String>,
    ) -> Result<(String, broadcast::Receiver<String>), kuku::Error> {
        let permit = self.semaphore.clone().try_acquire_owned().map_err(|_| {
            kuku::Error::InvalidArgument("server concurrent execution limit reached".into())
        })?;
        let session_id = options
            .session_id
            .clone()
            .unwrap_or_else(kuku::session::new_session_id);
        options.session_id = Some(session_id.clone());
        let workspace = std::fs::canonicalize(&options.workspace)?;
        let path = kuku::session::session_events_path(&options.data_dir, &workspace, &session_id)?;
        let mut sessions = self.sessions.lock().await;
        if !sessions.contains_key(&path) {
            let session = Session::open(options).await?;
            sessions.insert(
                path.clone(),
                OpenSession {
                    session,
                    runs: HashSet::new(),
                },
            );
        }
        let entry = sessions.get_mut(&path).expect("session was inserted");
        let session = entry.session.clone();
        let command_id = command_id.unwrap_or_else(kuku::session::new_session_id);
        let run_id = kuku::session::new_session_id();
        let setup = self.prepare_run(&session, &address, config, &prompt, &command_id);
        let receipt = match setup {
            Ok(receipt) => receipt,
            Err(error) => {
                if entry.runs.is_empty() {
                    let close = session.close().await;
                    sessions.remove(&path);
                    close?;
                }
                return Err(error);
            }
        };
        let mut observer = session.observe(receipt.event_id.saturating_sub(1));
        let (event_tx, event_rx) = broadcast::channel(256);
        let recent_events = Arc::new(Mutex::new(RecentEvents::default()));
        let start = crate::wire::run_start(&run_id, session.id(), &address, &receipt);
        publish(&event_tx, &recent_events, start);
        let (complete, done) = watch::channel(None);
        let permit = Arc::new(Mutex::new(Some(permit)));
        self.runs.lock().unwrap().insert(
            run_id.clone(),
            RunHandle {
                session: session.clone(),
                agent: address.clone(),
                workspace,
                done,
                permit: permit.clone(),
                recent_events: recent_events.clone(),
            },
        );
        entry.runs.insert(run_id.clone());
        drop(sessions);
        let runs = self.runs.clone();
        let sessions = self.sessions.clone();
        let task_run_id = run_id.clone();
        tokio::spawn(async move {
            let result = observe_run(
                &session,
                &address,
                &mut observer,
                event_tx.clone(),
                recent_events.clone(),
                permit,
            )
            .await;
            let result = match result {
                Ok(()) => wait_for_running_agents(&session, &mut observer, &sessions, &path).await,
                Err(error) => Err(error),
            };
            let mut sessions = sessions.lock().await;
            let close = if let Some(entry) = sessions.get_mut(&path) {
                entry.runs.remove(&task_run_id);
                if entry.runs.is_empty() {
                    session.close().await
                } else {
                    Ok(())
                }
            } else {
                Ok(())
            };
            if sessions
                .get(&path)
                .is_some_and(|entry| entry.runs.is_empty())
            {
                sessions.remove(&path);
            }
            let result = match (result, close) {
                (Ok(()), close) => close,
                (Err(error), Ok(())) => Err(error),
                (Err(error), Err(cleanup)) => Err(kuku::Error::InvalidEventStream(format!(
                    "{error}; session cleanup failed: {cleanup}"
                ))),
            };
            if let Err(error) = &result {
                publish(
                    &event_tx,
                    &recent_events,
                    json_line(
                        serde_json::json!({"type":"error","code":error.code(),"message":error.to_string()}),
                    ),
                );
            }
            runs.lock().unwrap().remove(&task_run_id);
            let _ = complete.send(Some(result.map_err(|error| error.to_string())));
        });
        Ok((run_id, event_rx))
    }

    /// Cancel the selected Agent and await its owned cleanup.
    pub fn cancel(&self, run_id: &str) -> Option<tokio::task::JoinHandle<Result<(), String>>> {
        let (session, agent, mut done) = {
            let runs = self.runs.lock().unwrap();
            let run = runs.get(run_id)?;
            (run.session.clone(), run.agent.clone(), run.done.clone())
        };
        Some(tokio::spawn(async move {
            session
                .cancel(&agent)
                .await
                .map_err(|error| error.to_string())?;
            if session
                .agents()
                .iter()
                .any(|other| other.address != agent && other.status == AgentStatus::Running)
            {
                return Ok(());
            }
            loop {
                if let Some(result) = done.borrow().clone() {
                    return result;
                }
                done.changed()
                    .await
                    .map_err(|_| "run cleanup ended without a result".to_string())?;
            }
        }))
    }

    /// Return the currently retained HTTP run identities.
    pub fn active_run_ids(&self) -> Vec<String> {
        self.runs.lock().unwrap().keys().cloned().collect()
    }

    /// Close all live Sessions while preserving durable waiting requests.
    pub async fn close_all(&self) -> kuku::Result<()> {
        let mut completions = self
            .runs
            .lock()
            .unwrap()
            .values()
            .map(|run| run.done.clone())
            .collect::<Vec<_>>();
        let mut failure = None;
        {
            let sessions = self.sessions.lock().await;
            for entry in sessions.values() {
                if let Err(error) = entry.session.close().await {
                    failure.get_or_insert(error);
                }
            }
        }
        for done in &mut completions {
            loop {
                if let Some(result) = done.borrow().clone() {
                    if let Err(error) = result {
                        failure.get_or_insert(kuku::Error::InvalidEventStream(error));
                    }
                    break;
                }
                if done.changed().await.is_err() {
                    failure.get_or_insert(kuku::Error::InvalidEventStream(
                        "host observation ended without cleanup completion".into(),
                    ));
                    break;
                }
            }
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Return bounded recent progress for open runs within the selected Session.
    pub fn recent_events(&self, session_id: &str) -> Vec<String> {
        self.runs
            .lock()
            .unwrap()
            .values()
            .filter(|run| run.session.id() == session_id)
            .flat_map(|run| {
                run.recent_events
                    .lock()
                    .unwrap()
                    .lines
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    pub(crate) fn respond(
        &self,
        run_id: &str,
        interaction_id: &str,
        agent: Option<&str>,
        command_id: &str,
        reply: HostReply,
    ) -> Result<kuku::session::Receipt, kuku::Error> {
        let runs = self.runs.lock().unwrap();
        let run = runs
            .get(run_id)
            .ok_or_else(|| kuku::Error::PermissionRequestNotPending(interaction_id.into()))?;
        if agent.is_some_and(|address| address != run.agent) {
            return Err(kuku::Error::PermissionRequestNotPending(
                interaction_id.into(),
            ));
        }
        let mut permit = run.permit.lock().unwrap();
        let newly_acquired = permit.is_none();
        if newly_acquired {
            *permit = Some(self.semaphore.clone().try_acquire_owned().map_err(|_| {
                kuku::Error::InvalidArgument("server concurrent execution limit reached".into())
            })?);
        }
        let result = match reply {
            HostReply::Permission(allow) => {
                run.session
                    .reply_permission(command_id, &run.agent, interaction_id, allow)
            }
            HostReply::Question(text) => {
                run.session
                    .reply_question(command_id, &run.agent, interaction_id, text)
            }
        };
        if newly_acquired
            && (result.is_err() || run.session.agent(&run.agent)?.status != AgentStatus::Running)
        {
            permit.take();
        }
        result
    }

    fn prepare_run(
        &self,
        session: &Session,
        address: &str,
        config: AgentConfig,
        prompt: &str,
        run_id: &str,
    ) -> kuku::Result<kuku::session::Receipt> {
        let snapshots = session.agents();
        if let Some(current) = snapshots.into_iter().find(|agent| agent.address == address) {
            if current.status == AgentStatus::Waiting {
                return Err(kuku::Error::InvalidArgument(format!(
                    "agent '{address}' is waiting for a correlated reply"
                )));
            }
            if current.status == AgentStatus::Running {
                return Err(kuku::Error::InvalidArgument(format!(
                    "agent '{address}' already has an active run"
                )));
            }
            if config != AgentConfig::default() {
                session.configure_agent(
                    &format!("config:{run_id}"),
                    address,
                    merge_config(current.config, config),
                )?;
            }
        } else {
            session.create_agent(&format!("create:{run_id}"), address, config)?;
        }
        let mut input = Input::new(run_id, address, prompt);
        input.source = InputSource::Host("http".into());
        let receipt = session.send(input)?;
        if matches!(
            session.agent(address)?.status,
            AgentStatus::Stopped | AgentStatus::Interrupted
        ) {
            session.advance(address)?;
        }
        Ok(receipt)
    }
}

async fn observe_run(
    session: &Session,
    address: &str,
    observer: &mut kuku::session::SessionObserver,
    event_tx: broadcast::Sender<String>,
    recent_events: Arc<Mutex<RecentEvents>>,
    permit: Arc<Mutex<Option<OwnedSemaphorePermit>>>,
) -> kuku::Result<()> {
    let mut last_permission = None;
    loop {
        match observer.next().await? {
            Some(Observation::Live { agent, event }) if agent == address => {
                if !matches!(
                    *event,
                    kuku::UiEvent::Done { .. }
                        | kuku::UiEvent::Cancelled { .. }
                        | kuku::UiEvent::PermissionRequested { .. }
                ) {
                    if let Some(line) = crate::wire::serialize_agent_event(&event, address) {
                        publish(&event_tx, &recent_events, line);
                    }
                }
            }
            Some(Observation::Live { .. }) => {}
            Some(Observation::Lagged { skipped }) => publish(
                &event_tx,
                &recent_events,
                json_line(serde_json::json!({"type":"lagged","skipped":skipped})),
            ),
            Some(Observation::Committed(event)) => {
                publish(&event_tx, &recent_events, committed_line(&event));
                let snapshot = session.agent(address)?;
                match snapshot.status {
                    AgentStatus::Waiting => {
                        permit.lock().unwrap().take();
                        if let Some(request) = snapshot.question {
                            if last_permission.as_deref() == Some(request.id.as_str()) {
                                continue;
                            }
                            last_permission = Some(request.id.clone());
                            publish(
                                &event_tx,
                                &recent_events,
                                json_line(
                                    serde_json::json!({"type":"question","session_id":session.id(),"agent":address,"id":request.id,"text":request.text,"turn":request.turn}),
                                ),
                            );
                        }
                        if let Some(request) = snapshot.permission {
                            if last_permission.as_deref() == Some(request.id.as_str()) {
                                continue;
                            }
                            last_permission = Some(request.id.clone());
                            if let Some(line) =
                                crate::wire::serialize_event(&kuku::UiEvent::PermissionRequested {
                                    request,
                                })
                            {
                                publish(&event_tx, &recent_events, line);
                            }
                        }
                    }
                    AgentStatus::Idle if snapshot.pending_inputs == 0 => {
                        drain_progress(address, observer, &event_tx, &recent_events).await?;
                        publish(
                            &event_tx,
                            &recent_events,
                            json_line(
                                serde_json::json!({"type":"done","session_id":session.id(),"agent":address,"conversation":address,"text":snapshot.last_output,"turn":snapshot.turn,"usage":snapshot.usage,"config":snapshot.config}),
                            ),
                        );
                        return Ok(());
                    }
                    AgentStatus::Stopped | AgentStatus::Interrupted => {
                        let terminal = if snapshot.last_error.is_some() {
                            "error"
                        } else {
                            "cancelled"
                        };
                        publish(
                            &event_tx,
                            &recent_events,
                            json_line(
                                serde_json::json!({"type":terminal,"session_id":session.id(),"agent":address,"conversation":address,"turn":snapshot.turn,"message":snapshot.last_error}),
                            ),
                        );
                        return Ok(());
                    }
                    AgentStatus::Running | AgentStatus::Idle => {
                        last_permission = None;
                    }
                }
            }
            None => return Ok(()),
        }
    }
}

async fn wait_for_running_agents(
    session: &Session,
    observer: &mut kuku::session::SessionObserver,
    sessions: &TokioMutex<HashMap<PathBuf, OpenSession>>,
    path: &Path,
) -> kuku::Result<()> {
    loop {
        if sessions
            .lock()
            .await
            .get(path)
            .is_none_or(|entry| entry.runs.len() > 1)
        {
            return Ok(());
        }
        if !session
            .agents()
            .iter()
            .any(|agent| agent.status == AgentStatus::Running)
        {
            return Ok(());
        }
        if observer.next().await?.is_none() {
            return Ok(());
        }
    }
}

async fn drain_progress(
    address: &str,
    observer: &mut kuku::session::SessionObserver,
    tx: &broadcast::Sender<String>,
    recent: &Mutex<RecentEvents>,
) -> kuku::Result<()> {
    for _ in 0..256 {
        match tokio::time::timeout(std::time::Duration::from_millis(1), observer.next()).await {
            Ok(Ok(Some(Observation::Live { agent, event }))) if agent == address => {
                if !matches!(
                    *event,
                    kuku::UiEvent::Done { .. }
                        | kuku::UiEvent::Cancelled { .. }
                        | kuku::UiEvent::PermissionRequested { .. }
                ) {
                    if let Some(line) = crate::wire::serialize_agent_event(&event, address) {
                        publish(tx, recent, line);
                    }
                }
            }
            Ok(Ok(Some(Observation::Committed(event)))) => {
                publish(tx, recent, committed_line(&event))
            }
            Ok(Ok(Some(Observation::Live { .. }))) => {}
            Ok(Ok(Some(Observation::Lagged { skipped }))) => publish(
                tx,
                recent,
                json_line(serde_json::json!({"type":"lagged","skipped":skipped})),
            ),
            Ok(Ok(None)) | Err(_) => break,
            Ok(Err(error)) => return Err(error),
        }
    }
    Ok(())
}

fn merge_config(mut current: AgentConfig, update: AgentConfig) -> AgentConfig {
    if update.connection.is_some() || update.model.is_some() || update.tier.is_some() {
        current.reasoning_effort = None;
        current.service_tier = None;
    }
    if update.connection.is_some() {
        current.connection = update.connection;
    }
    if update.tier.is_some() {
        current.tier = update.tier;
        current.model = None;
    }
    if update.model.is_some() {
        current.model = update.model;
    }
    if update.reasoning_effort.is_some() {
        current.reasoning_effort = update.reasoning_effort;
    }
    if update.service_tier.is_some() {
        current.service_tier = update.service_tier;
    }
    if update.max_output_tokens.is_some() {
        current.max_output_tokens = update.max_output_tokens;
    }
    current.no_agents |= update.no_agents;
    current.no_skills |= update.no_skills;
    current
}

fn publish(tx: &broadcast::Sender<String>, recent: &Mutex<RecentEvents>, line: String) {
    let mut recent = recent.lock().unwrap();
    while recent.lines.len() >= RECENT_EVENT_LIMIT || recent.bytes + line.len() > RECENT_BYTE_LIMIT
    {
        let Some(removed) = recent.lines.pop_front() else {
            break;
        };
        recent.bytes -= removed.len();
    }
    if line.len() <= RECENT_BYTE_LIMIT {
        recent.bytes += line.len();
        recent.lines.push_back(line.clone());
    }
    let _ = tx.send(line);
}

fn committed_line(event: &kuku::event::StoredEvent) -> String {
    let agent = crate::wire::event_agent(&event.payload);
    json_line(
        serde_json::json!({"type":"committed","id":event.id,"kind":event.payload.kind_name(),"agent":agent}),
    )
}

fn json_line(value: serde_json::Value) -> String {
    format!("{value}\n")
}
