use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::platform::ConfigService;

pub struct ConfigWatcher {
    cancel: Arc<tokio::sync::Notify>,
}

impl ConfigWatcher {
    pub fn start(config_path: PathBuf, config_store: Arc<ArcSwap<kuku::config::Config>>) -> Self {
        let cancel = Arc::new(tokio::sync::Notify::new());
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            let mut last_mtime = std::fs::metadata(&config_path)
                .and_then(|m| m.modified())
                .ok();
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3));
            interval.tick().await;

            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    _ = cancel_clone.notified() => break,
                }

                let Ok(meta) = std::fs::metadata(&config_path) else {
                    continue;
                };
                let Ok(mtime) = meta.modified() else {
                    continue;
                };

                if last_mtime.as_ref() == Some(&mtime) {
                    continue;
                }

                match kuku::config::load_and_patch_config(&config_path).and_then(|f| f.resolve()) {
                    Ok(new_config) => {
                        config_store.store(Arc::new(new_config));
                        last_mtime = Some(mtime);
                        tracing::info!("config reloaded");
                    }
                    Err(e) => {
                        tracing::warn!("config reload failed: {e}");
                    }
                }
            }
        });

        Self { cancel }
    }
}

impl Drop for ConfigWatcher {
    fn drop(&mut self) {
        self.cancel.notify_waiters();
    }
}

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
