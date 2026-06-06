use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{broadcast, oneshot, Mutex as TokioMutex, Semaphore};

pub struct RunHandle {
    cancel_token: Arc<tokio::sync::Notify>,
    pub workspace: PathBuf,
    join_handle: tokio::task::JoinHandle<()>,
    recent_events: Arc<Mutex<VecDeque<String>>>,
}

type PermissionKey = (String, String);
type PermissionMap =
    Arc<TokioMutex<HashMap<PermissionKey, oneshot::Sender<kuku::PermissionChoice>>>>;

pub struct RunManager {
    runs: Arc<Mutex<HashMap<String, RunHandle>>>,
    permissions: PermissionMap,
    semaphore: Arc<Semaphore>,
}

impl RunManager {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            runs: Arc::new(Mutex::new(HashMap::new())),
            permissions: Arc::new(TokioMutex::new(HashMap::new())),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
        }
    }

    pub async fn spawn_run(
        &mut self,
        query: kuku::Query,
    ) -> Result<(String, broadcast::Receiver<String>), kuku::Error> {
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| kuku::Error::ConfigLoad("semaphore closed".to_string()))?;

        if let Some(sid) = query.session_id() {
            if self.runs.lock().unwrap().contains_key(sid) {
                drop(permit);
                return Err(kuku::Error::SessionLocked {
                    session: PathBuf::from(sid),
                    holder_pid: 0,
                });
            }
        }

        let run = match query.start().await {
            Ok(run) => run,
            Err(e) => {
                drop(permit);
                return Err(e);
            }
        };

        let run_id = run.session_id().to_string();

        let (event_tx, event_rx) = broadcast::channel(256);
        let cancel_token = run.cancel_token();
        let workspace = run.workspace().to_path_buf();
        let recent_events = Arc::new(Mutex::new(VecDeque::new()));

        let run_start_line = crate::wire::run_start(&run_id);
        let _ = event_tx.send(run_start_line.clone());
        push_event(&recent_events, &run_start_line);

        let permissions = self.permissions.clone();
        let event_tx_clone = event_tx.clone();
        let recent_clone = recent_events.clone();
        let cancel_for_perm = cancel_token.clone();
        let run_id_for_loop = run_id.clone();
        let (done_tx, done_rx) = oneshot::channel::<()>();
        let join_handle = tokio::spawn(async move {
            Self::run_loop(
                run,
                run_id_for_loop,
                event_tx_clone,
                permissions,
                permit,
                recent_clone,
                cancel_for_perm,
            )
            .await;
            let _ = done_tx.send(());
        });

        let handle = RunHandle {
            cancel_token: cancel_token.clone(),
            workspace,
            join_handle,
            recent_events,
        };
        self.runs.lock().unwrap().insert(run_id.clone(), handle);

        let runs = self.runs.clone();
        let rid = run_id.clone();
        tokio::spawn(async move {
            let _ = done_rx.await;
            runs.lock().unwrap().remove(&rid);
        });

        Ok((run_id, event_rx))
    }

    async fn run_loop(
        mut run: kuku::Run,
        run_id: String,
        event_tx: broadcast::Sender<String>,
        permissions: PermissionMap,
        _permit: tokio::sync::OwnedSemaphorePermit,
        recent_events: Arc<Mutex<VecDeque<String>>>,
        cancel_for_perm: Arc<tokio::sync::Notify>,
    ) {
        loop {
            match run.next().await {
                Ok(Some(event)) => {
                    if let kuku::UiEvent::PermissionRequested { ref request } = event {
                        if let Some(line) = crate::wire::serialize_event(&event) {
                            push_event(&recent_events, &line);
                            let _ = event_tx.send(line);
                        }
                        let (tx, rx) = oneshot::channel();
                        let permission_key = (run_id.clone(), request.id.clone());
                        permissions.lock().await.insert(permission_key.clone(), tx);
                        let choice = tokio::select! {
                            result = rx => result.unwrap_or(kuku::PermissionChoice::Deny),
                            _ = tokio::time::sleep(Duration::from_secs(60)) => kuku::PermissionChoice::Deny,
                            _ = cancel_for_perm.notified() => kuku::PermissionChoice::Deny,
                        };
                        permissions.lock().await.remove(&permission_key);
                        if let Ok(Some(result_event)) = run.decide(&request.id, choice, None).await
                        {
                            if let Some(line) = crate::wire::serialize_event(&result_event) {
                                push_event(&recent_events, &line);
                                let _ = event_tx.send(line);
                            }
                        }
                        continue;
                    }
                    let is_done = matches!(event, kuku::UiEvent::Done { .. });
                    if let Some(line) = crate::wire::serialize_event(&event) {
                        push_event(&recent_events, &line);
                        let _ = event_tx.send(line);
                    }
                    if is_done {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let err_event = kuku::UiEvent::Error {
                        code: e.code().to_string(),
                        message: e.to_string(),
                    };
                    if let Some(line) = crate::wire::serialize_event(&err_event) {
                        push_event(&recent_events, &line);
                        let _ = event_tx.send(line);
                    }
                    break;
                }
            }
        }
    }

    pub fn cancel(&self, run_id: &str) -> Option<tokio::task::JoinHandle<()>> {
        if let Some(handle) = self.runs.lock().unwrap().remove(run_id) {
            handle.cancel_token.notify_waiters();
            Some(handle.join_handle)
        } else {
            None
        }
    }

    pub async fn respond(
        &self,
        run_id: &str,
        interaction_id: &str,
        choice: kuku::PermissionChoice,
    ) -> Result<(), kuku::Error> {
        let tx = self
            .permissions
            .lock()
            .await
            .remove(&(run_id.to_string(), interaction_id.to_string()))
            .ok_or_else(|| kuku::Error::PermissionRequestNotPending(interaction_id.to_string()))?;
        let _ = tx.send(choice);
        Ok(())
    }

    pub fn active_run_ids(&self) -> Vec<String> {
        self.runs.lock().unwrap().keys().cloned().collect()
    }

    pub fn recent_events(&self, session_id: &str) -> Vec<String> {
        self.runs
            .lock()
            .unwrap()
            .get(session_id)
            .map(|h| h.recent_events.lock().unwrap().iter().cloned().collect())
            .unwrap_or_default()
    }
}

fn push_event(buf: &Mutex<VecDeque<String>>, line: &str) {
    let mut buf = buf.lock().unwrap();
    if buf.len() >= 200 {
        buf.pop_front();
    }
    buf.push_back(line.to_string());
}
