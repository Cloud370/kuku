use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use kuku::config::{Config, config_sha256, parse_config_text};

pub(crate) struct ConfigSource {
    pub(crate) path: PathBuf,
    pub(crate) loaded_sha256: ArcSwap<String>,
}

impl ConfigSource {
    pub(crate) fn open(path: PathBuf) -> kuku::Result<(Arc<Self>, Config)> {
        let bytes = std::fs::read(&path).map_err(|error| {
            kuku::Error::ConfigLoad(format!(
                "cannot read config file {}: {error}",
                path.display()
            ))
        })?;
        let config = resolve(&bytes)?;
        let source = Arc::new(Self {
            path,
            loaded_sha256: ArcSwap::from_pointee(config_sha256(&bytes)),
        });
        Ok((source, config))
    }
}

pub(crate) struct ConfigWatcher {
    cancel: Arc<tokio::sync::Notify>,
}

impl ConfigWatcher {
    pub(crate) fn start(source: Arc<ConfigSource>, config_store: Arc<ArcSwap<Config>>) -> Self {
        let cancel = Arc::new(tokio::sync::Notify::new());
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            let mut previous_hash = (**source.loaded_sha256.load()).clone();
            let mut interval = tokio::time::interval(Duration::from_secs(3));
            interval.tick().await;
            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    _ = cancel_clone.notified() => break,
                }
                let Ok(bytes) = tokio::fs::read(&source.path).await else {
                    continue;
                };
                let hash = config_sha256(&bytes);
                if hash == previous_hash {
                    continue;
                }
                match resolve(&bytes) {
                    Ok(config) => {
                        config_store.store(Arc::new(config));
                        source.loaded_sha256.store(Arc::new(hash.clone()));
                        tracing::info!("config reloaded");
                    }
                    Err(error) => tracing::warn!("config reload failed: {error}"),
                }
                previous_hash = hash;
            }
        });

        Self { cancel }
    }
}

impl Drop for ConfigWatcher {
    fn drop(&mut self) {
        self.cancel.notify_one();
    }
}

fn resolve(bytes: &[u8]) -> kuku::Result<Config> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| kuku::Error::ConfigLoad("config file must be valid UTF-8".into()))?;
    parse_config_text(text)?.resolve()
}
