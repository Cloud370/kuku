use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, Mutex, Semaphore};

pub struct RunHandle {
    cancel_token: Arc<tokio::sync::Notify>,
    pub workspace: PathBuf,
    #[allow(dead_code)]
    join_handle: tokio::task::JoinHandle<()>,
}

type PermissionMap = Arc<Mutex<HashMap<String, oneshot::Sender<kuku::PermissionChoice>>>>;

pub struct RunManager {
    runs: HashMap<String, RunHandle>,
    permissions: PermissionMap,
    semaphore: Arc<Semaphore>,
}

impl RunManager {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            runs: HashMap::new(),
            permissions: Arc::new(Mutex::new(HashMap::new())),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
        }
    }

    pub async fn spawn_run(
        &mut self,
        query: kuku::Query,
    ) -> Result<(String, mpsc::Receiver<String>), kuku::Error> {
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| kuku::Error::ConfigLoad("semaphore closed".to_string()))?;

        if let Some(sid) = query.session_id() {
            if self.runs.contains_key(sid) {
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

        let (event_tx, event_rx) = mpsc::channel(256);
        let cancel_token = run.cancel_token();
        let workspace = run.workspace().to_path_buf();

        let permissions = self.permissions.clone();
        let event_tx_clone = event_tx.clone();
        let join_handle = tokio::spawn(Self::run_loop(run, event_tx_clone, permissions, permit));

        let handle = RunHandle {
            cancel_token: cancel_token.clone(),
            workspace,
            join_handle,
        };
        self.runs.insert(run_id.clone(), handle);

        Ok((run_id, event_rx))
    }

    async fn run_loop(
        mut run: kuku::Run,
        event_tx: mpsc::Sender<String>,
        permissions: PermissionMap,
        _permit: tokio::sync::OwnedSemaphorePermit,
    ) {
        loop {
            match run.next().await {
                Ok(Some(event)) => {
                    if let kuku::UiEvent::PermissionRequested { ref request } = event {
                        if let Some(line) = crate::wire::serialize_event(&event) {
                            if event_tx.send(line).await.is_err() {
                                break;
                            }
                        }
                        let (tx, rx) = oneshot::channel();
                        permissions.lock().await.insert(request.id.clone(), tx);
                        let choice = rx.await.unwrap_or(kuku::PermissionChoice::Deny);
                        if let Ok(Some(result_event)) = run.decide(&request.id, choice).await {
                            if let Some(line) = crate::wire::serialize_event(&result_event) {
                                if event_tx.send(line).await.is_err() {
                                    break;
                                }
                            }
                        }
                        continue;
                    }
                    let is_done = matches!(event, kuku::UiEvent::Done { .. });
                    if let Some(line) = crate::wire::serialize_event(&event) {
                        if event_tx.send(line).await.is_err() {
                            break;
                        }
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
                        let _ = event_tx.send(line).await;
                    }
                    break;
                }
            }
        }
    }

    pub fn cancel(&mut self, run_id: &str) -> bool {
        if let Some(handle) = self.runs.remove(run_id) {
            handle.cancel_token.notify_waiters();
            true
        } else {
            false
        }
    }

    pub async fn respond(
        &self,
        interaction_id: &str,
        choice: kuku::PermissionChoice,
    ) -> Result<(), kuku::Error> {
        let tx = self
            .permissions
            .lock()
            .await
            .remove(interaction_id)
            .ok_or_else(|| kuku::Error::PermissionRequestNotPending(interaction_id.to_string()))?;
        let _ = tx.send(choice);
        Ok(())
    }

    pub fn active_run_ids(&self) -> Vec<String> {
        self.runs.keys().cloned().collect()
    }
}
