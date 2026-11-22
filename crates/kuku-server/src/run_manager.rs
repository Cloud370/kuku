//! Own open SDK Sessions independently of HTTP requests and observers.

mod commands;
mod values;

pub(crate) use commands::{input_status, INTERNAL_CREATE_PREFIX};
pub(crate) use values::SaveValue;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;
use kuku::session::{
    HistoryPoint, Input, Session, SessionHistory, SessionOptions, TurnIndex, TurnSummary,
};
use tokio::sync::{watch, Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore};

use crate::error_mapping::{ApiError, Result};
use crate::wire::{Command, CommandResult};

const RELEASE_INTERVAL: Duration = Duration::from_millis(100);
const CLOSED_HISTORY_LIMIT: usize = 4;

/// Own the Sessions and concurrent execution allowance of one HTTP service.
#[derive(Clone)]
pub struct RunManager {
    inner: Arc<Registry>,
}

struct Registry {
    slots: Mutex<HashMap<PathBuf, Arc<Slot>>>,
    closed_history: Mutex<VecDeque<Arc<CachedHistory>>>,
    changed: watch::Sender<()>,
    permits: Arc<Semaphore>,
}

struct CachedHistory {
    path: PathBuf,
    cursor: u64,
    index: Arc<TurnIndex>,
    history: Arc<SessionHistory>,
}

pub(crate) enum TurnLookup {
    Live(Session),
    Closed(Arc<TurnIndex>),
}

impl TurnLookup {
    pub(crate) fn history_points(
        &self,
        agent: &str,
        before: u64,
        limit: usize,
    ) -> Vec<HistoryPoint> {
        match self {
            Self::Live(session) => session.history_points(agent, before, limit),
            Self::Closed(index) => index.history_points(agent, before, limit),
        }
    }

    pub(crate) fn number(&self, agent: &str, turn: u64) -> Option<u64> {
        match self {
            Self::Live(session) => session.turn_number(agent, turn),
            Self::Closed(index) => index.number(agent, turn),
        }
    }

    pub(crate) fn started_at_ms(&self, agent: &str, turn: u64) -> Option<u64> {
        match self {
            Self::Live(session) => session.turn_started_at_ms(agent, turn),
            Self::Closed(index) => index.started_at_ms(agent, turn),
        }
    }

    pub(crate) fn summary_at(&self, event_id: u64) -> Option<(u64, TurnSummary)> {
        match self {
            Self::Live(session) => session.turn_summary_at(event_id),
            Self::Closed(index) => index.summary_at(event_id),
        }
    }

    pub(crate) fn explicitly_stopped(&self, agent: &str, turn: u64) -> bool {
        match self {
            Self::Live(session) => session.turn_explicitly_stopped(agent, turn),
            Self::Closed(index) => index.explicitly_stopped(agent, turn),
        }
    }
}

struct Slot {
    session_id: String,
    workspace: PathBuf,
    open: AsyncMutex<Option<OpenSession>>,
    session: Mutex<Option<Session>>,
    active_operation: AtomicBool,
}

struct OpenSession {
    session: Session,
    permits: HashMap<String, OwnedSemaphorePermit>,
}

struct SessionOperation<'a>(&'a AtomicBool);

impl<'a> SessionOperation<'a> {
    fn begin(active: &'a AtomicBool) -> Self {
        active.store(true, Ordering::Relaxed);
        Self(active)
    }
}

impl Drop for SessionOperation<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

impl RunManager {
    /// Limit independently advancing host submissions while keeping waiting Sessions recoverable.
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            inner: Arc::new(Registry {
                slots: Mutex::new(HashMap::new()),
                closed_history: Mutex::new(VecDeque::new()),
                changed: watch::channel(()).0,
                permits: Arc::new(Semaphore::new(max_concurrent)),
            }),
        }
    }

    /// Return identities of active Session commands, executions, Jobs, and tool cleanup.
    pub fn active_run_ids(&self) -> Vec<String> {
        let slots = self.inner.slots.lock().unwrap();
        let mut ids = Vec::new();
        for slot in slots.values() {
            let seen = ids.len();
            if let Some(session) = slot.session.lock().unwrap().as_ref() {
                ids.extend(session.active_executions().into_iter().map(|(agent, id)| {
                    format!(
                        "{}/{agent}/{id} ({})",
                        slot.session_id,
                        slot.workspace.display()
                    )
                }));
                ids.extend(session.running_jobs().into_iter().map(|(agent, id)| {
                    format!(
                        "{}/{agent}/{id} ({})",
                        slot.session_id,
                        slot.workspace.display()
                    )
                }));
            }
            if slot.active_operation.load(Ordering::Relaxed) && ids.len() == seen {
                ids.push(format!("{} (Session operation)", slot.session_id));
            }
        }
        ids.sort();
        ids
    }

    /// Close all opened Sessions and wait for their writer and tool cleanup.
    pub async fn close_all(&self) -> kuku::Result<()> {
        let slots = self
            .inner
            .slots
            .lock()
            .unwrap()
            .iter()
            .map(|(path, slot)| (path.clone(), slot.clone()))
            .collect::<Vec<_>>();
        let mut failure = None;
        for (path, slot) in slots {
            let mut open = slot.open.lock().await;
            if let Some(entry) = open.as_ref() {
                if let Err(error) = entry.session.close().await {
                    failure.get_or_insert(error);
                }
            }
            *open = None;
            *slot.session.lock().unwrap() = None;
            self.release_slot(&path, &slot);
        }
        self.inner.changed.send_replace(());
        failure.map_or(Ok(()), Err)
    }

    pub(crate) fn changes(&self) -> watch::Receiver<()> {
        self.inner.changed.subscribe()
    }

    pub(crate) fn open_session(&self, path: &Path) -> Option<Session> {
        let slots = self.inner.slots.lock().unwrap();
        let session = slots.get(path)?.session.lock().unwrap().clone();
        session
    }

    pub(crate) fn turn_lookup(&self, path: &Path, minimum_cursor: u64) -> Result<TurnLookup> {
        if let Some(session) = self.open_session(path) {
            return Ok(TurnLookup::Live(session));
        }
        let saved = self.closed_history(path, minimum_cursor)?;
        Ok(TurnLookup::Closed(saved.index.clone()))
    }

    pub(crate) fn history(
        &self,
        path: &Path,
        minimum_cursor: u64,
    ) -> Result<(Arc<SessionHistory>, TurnLookup)> {
        if let Some(session) = self.open_session(path) {
            return Ok((
                Arc::new(SessionHistory::read(path)?),
                TurnLookup::Live(session),
            ));
        }
        let saved = self.closed_history(path, minimum_cursor)?;
        Ok((
            saved.history.clone(),
            TurnLookup::Closed(saved.index.clone()),
        ))
    }

    fn closed_history(&self, path: &Path, minimum_cursor: u64) -> Result<Arc<CachedHistory>> {
        {
            let mut cache = self.inner.closed_history.lock().unwrap();
            if let Some(position) = cache
                .iter()
                .position(|item| item.path == path && item.cursor >= minimum_cursor)
            {
                let saved = cache.remove(position).unwrap();
                cache.push_front(saved.clone());
                return Ok(saved);
            }
        }

        let (history, index) = SessionHistory::read_with_turns(path)?;
        let cursor = history.cursor();
        let saved = Arc::new(CachedHistory {
            path: path.to_owned(),
            cursor,
            index: Arc::new(index),
            history: Arc::new(history),
        });
        Ok(self.cache_history(saved))
    }

    fn cache_history(&self, saved: Arc<CachedHistory>) -> Arc<CachedHistory> {
        let mut cache = self.inner.closed_history.lock().unwrap();
        if let Some(newer) = cache
            .iter()
            .find(|item| item.path == saved.path && item.cursor >= saved.cursor)
        {
            return newer.clone();
        }
        if saved.path.is_file() {
            cache.retain(|item| item.path != saved.path);
            cache.push_front(saved.clone());
            cache.truncate(CLOSED_HISTORY_LIMIT);
        }
        saved
    }

    pub(crate) async fn command(
        &self,
        mut options: SessionOptions,
        address: &str,
        command_id: &str,
        command: Command,
        replay_only: bool,
    ) -> Result<CommandResult> {
        let session_id = options.session_id.as_deref().expect("explicit Session ID");
        let creation = command.creation();
        if replay_only && !options.data_dir.is_dir() {
            return Err(ApiError::receipt_not_found());
        }
        if creation.is_some() && !replay_only {
            std::fs::create_dir_all(&options.data_dir)?;
        }
        if options.data_dir.is_dir() {
            options.data_dir = options.data_dir.canonicalize()?;
        }
        let path =
            kuku::session::session_events_path(&options.data_dir, &options.workspace, session_id)?;
        let input = command.input(command_id, address)?;
        let slot = self.slot(&path, &options.workspace, session_id);
        let mut open = slot.open.lock().await;
        let _operation = SessionOperation::begin(&slot.active_operation);
        let result = self
            .execute(
                &mut open,
                options,
                CommandExecution {
                    address,
                    command_id,
                    command,
                    input,
                    replay_only,
                },
                &path,
                &slot,
            )
            .await;
        if open.is_none() {
            self.release_slot(&path, &slot);
        }
        result
    }

    pub(crate) async fn delete_session(
        &self,
        home: &Path,
        workspace: &Path,
        session_id: &str,
    ) -> Result<()> {
        let path = kuku::session::session_events_path(home, workspace, session_id)?;
        let slot = self.slot(&path, workspace, session_id);
        let mut open = slot.open.lock().await;
        let _operation = SessionOperation::begin(&slot.active_operation);
        let result = if let Some(entry) = open.as_ref() {
            if !entry.session.active_executions().is_empty() || entry.session.has_active_jobs() {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "session_busy",
                    "Session has active execution, Jobs, or cleanup",
                ));
            }
            let closed = entry.session.close().await;
            *open = None;
            *slot.session.lock().unwrap() = None;
            closed.map_err(ApiError::from)
        } else {
            Ok(())
        };
        let result = result.and_then(|()| {
            let mut cache = self.inner.closed_history.lock().unwrap();
            if !path.is_file() {
                return Err(ApiError::session_not_found());
            }
            kuku::delete_session(home, Some(workspace), session_id)?;
            cache.retain(|item| item.path != path);
            Ok(())
        });
        self.release_slot(&path, &slot);
        self.inner.changed.send_replace(());
        result
    }

    async fn execute(
        &self,
        open: &mut Option<OpenSession>,
        options: SessionOptions,
        request: CommandExecution<'_>,
        path: &Path,
        slot: &Arc<Slot>,
    ) -> Result<CommandResult> {
        let initial_prompts = options.prompts_dir.clone();
        let creation = request.command.creation().zip(request.input.as_ref());
        if let Some(entry) = open.as_mut() {
            entry.release_idle_permits();
        } else {
            let exists = path.is_file();
            if request.replay_only
                && (!exists || Session::read_receipt(path, request.command_id)?.is_none())
            {
                return Err(ApiError::receipt_not_found());
            }
            let permit = if !exists && creation.is_some() {
                Some(self.acquire_permit()?)
            } else {
                None
            };
            let session = match creation {
                Some((config, input)) => Session::open_for_creation(options, input, config).await?,
                None if exists => Session::open(options).await?,
                None => return Err(ApiError::session_not_found()),
            };
            *slot.session.lock().unwrap() = Some(session.clone());
            *open = Some(OpenSession {
                session,
                permits: permit
                    .into_iter()
                    .map(|permit| (request.address.into(), permit))
                    .collect(),
            });
            self.monitor(path.to_owned(), slot.clone());
            self.inner.changed.send_replace(());
        }
        let open = open.as_mut().expect("Session is open");
        if let Some((config, input)) = creation {
            open.session
                .verify_creation(input, config, initial_prompts.as_deref())?;
        }
        self.execute_command(open, request).await
    }

    fn monitor(&self, path: PathBuf, slot: Arc<Slot>) {
        let manager = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(RELEASE_INTERVAL).await;
                let mut open = slot.open.lock().await;
                let Some(entry) = open.as_mut() else {
                    manager.release_slot(&path, &slot);
                    break;
                };
                if entry.release_idle_permits() || entry.session.has_active_jobs() {
                    continue;
                }
                let history = entry.session.history_snapshot().ok();
                if let Err(error) = entry.session.close().await {
                    tracing::warn!(%error, session = %slot.session_id, "Session closed with a cleanup error");
                } else if let Some((history, index)) = history {
                    // SessionClosed advances the cursor without changing the history projection.
                    let closed = kuku::event::EventStore::read_page_before(&path, None, 1);
                    if let Ok(events) = closed {
                        if let [event] = events.as_slice() {
                            if event.id == history.cursor().saturating_add(1)
                                && matches!(&event.payload, kuku::event::EventPayload::Unknown(value) if value["record"]["operation"] == "session_closed")
                            {
                                manager.cache_history(Arc::new(CachedHistory {
                                    path: path.clone(),
                                    cursor: event.id,
                                    history: Arc::new(history),
                                    index: Arc::new(index),
                                }));
                            }
                        }
                    }
                }
                *open = None;
                *slot.session.lock().unwrap() = None;
                manager.release_slot(&path, &slot);
                manager.inner.changed.send_replace(());
                break;
            }
        });
    }

    fn slot(&self, path: &Path, workspace: &Path, session_id: &str) -> Arc<Slot> {
        self.inner
            .slots
            .lock()
            .unwrap()
            .entry(path.to_owned())
            .or_insert_with(|| {
                Arc::new(Slot {
                    session_id: session_id.into(),
                    workspace: workspace.to_owned(),
                    open: AsyncMutex::new(None),
                    session: Mutex::new(None),
                    active_operation: AtomicBool::new(false),
                })
            })
            .clone()
    }

    fn release_slot(&self, path: &Path, slot: &Arc<Slot>) {
        let mut slots = self.inner.slots.lock().unwrap();
        if Arc::strong_count(slot) == 2
            && slots
                .get(path)
                .is_some_and(|current| Arc::ptr_eq(current, slot))
        {
            slots.remove(path);
        }
    }

    fn acquire_permit(&self) -> Result<OwnedSemaphorePermit> {
        let semaphore = &self.inner.permits;
        if let Ok(permit) = semaphore.clone().try_acquire_owned() {
            return Ok(permit);
        }
        for slot in self.inner.slots.lock().unwrap().values() {
            if let Ok(mut open) = slot.open.try_lock() {
                if let Some(entry) = open.as_mut() {
                    entry.release_idle_permits();
                }
            }
        }
        semaphore.clone().try_acquire_owned().map_err(|_| {
            ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "execution_limit",
                "Server concurrent execution limit reached",
            )
        })
    }
}

impl OpenSession {
    fn release_idle_permits(&mut self) -> bool {
        let active = self.session.active_executions();
        self.permits
            .retain(|agent, _| active.iter().any(|(address, _)| address == agent));
        !active.is_empty()
    }
}

struct CommandExecution<'a> {
    address: &'a str,
    command_id: &'a str,
    command: Command,
    input: Option<Input>,
    replay_only: bool,
}

#[cfg(test)]
mod tests {
    use std::future::{poll_fn, Future};
    use std::path::Path;
    use std::pin::pin;
    use std::task::Poll;

    use super::{RunManager, SessionOperation};

    #[tokio::test]
    async fn an_idle_slot_release_blocks_close_but_is_not_active_work() {
        let manager = RunManager::new(1);
        let slot = manager.slot(Path::new("events.jsonl"), Path::new("workspace"), "idle");
        let held = slot.open.lock().await;

        assert!(manager.active_run_ids().is_empty());
        let operation = SessionOperation::begin(&slot.active_operation);
        assert_eq!(vec!["idle (Session operation)"], manager.active_run_ids());
        drop(operation);
        let mut close = pin!(manager.close_all());
        assert!(poll_fn(|context| Poll::Ready(close.as_mut().poll(context).is_pending())).await);

        drop(held);
        close.await.unwrap();
    }
}
