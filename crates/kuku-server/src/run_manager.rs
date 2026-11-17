//! Own open SDK Sessions independently of HTTP requests and observers.

mod commands;

pub(crate) use commands::{input_status, INTERNAL_CREATE_PREFIX};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;
use kuku::session::{Input, Session, SessionOptions};
use tokio::sync::{watch, Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore};

use crate::error_mapping::{ApiError, Result};
use crate::wire::{Command, CommandResult};

const RELEASE_INTERVAL: Duration = Duration::from_millis(100);

/// Own the Sessions and concurrent execution allowance of one HTTP service.
#[derive(Clone)]
pub struct RunManager {
    inner: Arc<Registry>,
}

struct Registry {
    slots: Mutex<HashMap<PathBuf, Arc<Slot>>>,
    changed: watch::Sender<()>,
    permits: Arc<Semaphore>,
}

struct Slot {
    session_id: String,
    workspace: PathBuf,
    open: AsyncMutex<Option<OpenSession>>,
    session: Mutex<Option<Session>>,
}

struct OpenSession {
    session: Session,
    permits: HashMap<String, OwnedSemaphorePermit>,
}

impl RunManager {
    /// Limit independently advancing host submissions while keeping waiting Sessions recoverable.
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            inner: Arc::new(Registry {
                slots: Mutex::new(HashMap::new()),
                changed: watch::channel(()).0,
                permits: Arc::new(Semaphore::new(max_concurrent)),
            }),
        }
    }

    /// Return identities of executions that still own work or cleanup.
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
            }
            if slot.open.try_lock().is_err() && ids.len() == seen {
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

    pub(crate) async fn busy(&self) -> Vec<String> {
        self.active_run_ids()
    }

    pub(crate) fn changes(&self) -> watch::Receiver<()> {
        self.inner.changed.subscribe()
    }

    pub(crate) fn open_session(&self, path: &Path) -> Option<Session> {
        let slots = self.inner.slots.lock().unwrap();
        let session = slots.get(path)?.session.lock().unwrap().clone();
        session
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
        let result = if let Some(entry) = open.as_ref() {
            if !entry.session.active_executions().is_empty() {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "session_busy",
                    "Session has active execution or cleanup",
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
            if !path.is_file() {
                return Err(ApiError::session_not_found());
            }
            kuku::delete_session(home, Some(workspace), session_id).map_err(ApiError::from)
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
                if entry.release_idle_permits() {
                    continue;
                }
                if let Err(error) = entry.session.close().await {
                    tracing::warn!(%error, session = %slot.session_id, "Session closed with a cleanup error");
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
