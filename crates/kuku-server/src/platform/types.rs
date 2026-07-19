use std::path::PathBuf;
use std::sync::Arc;

use kuku::config::{Config, ConfigFile};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformState {
    Missing,
    Invalid { diagnostics: Vec<String> },
    Ready,
}

#[derive(Debug, Clone)]
pub struct ConfigSnapshot {
    pub path: PathBuf,
    pub state: PlatformState,
    pub raw: Option<ConfigFile>,
    pub resolved: Option<Arc<Config>>,
}

#[derive(Debug, Clone)]
pub struct ConfigPatch {
    pub file: ConfigFile,
}

impl ConfigPatch {
    pub fn replace(file: ConfigFile) -> Self {
        Self { file }
    }
}
