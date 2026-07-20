use std::path::PathBuf;
use std::sync::Arc;

use crate::platform::ConfigService;

pub struct ConfigWatcherHandle {
    cancel: tokio::sync::watch::Sender<bool>,
    join: tokio::task::JoinHandle<()>,
}

impl ConfigWatcherHandle {
    pub fn start(_config_path: PathBuf, service: Arc<ConfigService>) -> Self {
        let (cancel, mut cancelled) = tokio::sync::watch::channel(false);
        let join = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let _ = service.reload_from_disk().await;
                    }
                    changed = cancelled.changed() => {
                        if changed.is_err() || *cancelled.borrow() {
                            break;
                        }
                    }
                }
            }
        });
        Self { cancel, join }
    }

    pub async fn shutdown(self) {
        let _ = self.cancel.send(true);
        let _ = self.join.await;
    }
}
