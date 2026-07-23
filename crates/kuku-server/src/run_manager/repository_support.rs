use std::collections::HashMap;
#[cfg(unix)]
use std::fs::File;
use std::hash::Hash;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use kuku::event::TaskId;
use tokio::sync::Mutex as TokioMutex;

use super::domain::DomainError;
use super::repository::RepositoryState;

pub(super) fn updated_at(path: &Path) -> Result<String, DomainError> {
    let modified = std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|_| DomainError::LedgerCorrupt)?;
    super::domain::system_time_rfc3339(modified)
}

pub(super) fn valid_task_component(task_id: &TaskId) -> bool {
    let mut components = Path::new(task_id.as_str()).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

#[cfg(unix)]
pub(super) fn sync_directory(path: &Path) -> Result<(), DomainError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| DomainError::LedgerCorrupt)
}

#[cfg(not(unix))]
pub(super) fn sync_directory(_path: &Path) -> Result<(), DomainError> {
    Ok(())
}

pub(super) fn shared_state(root: &Path) -> Arc<RepositoryState> {
    static STATES: OnceLock<Mutex<HashMap<PathBuf, Weak<RepositoryState>>>> = OnceLock::new();
    let states = STATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut states = states
        .lock()
        .expect("repository state lock is not poisoned");
    if let Some(state) = states.get(root).and_then(Weak::upgrade) {
        return state;
    }
    let state = Arc::new(RepositoryState::default());
    states.insert(root.to_owned(), Arc::downgrade(&state));
    state
}

pub(super) fn shared_gate<K>(
    gates: &Mutex<HashMap<K, Weak<TokioMutex<()>>>>,
    key: K,
) -> Arc<TokioMutex<()>>
where
    K: Hash + Eq,
{
    let mut gates = gates.lock().expect("repository gate map is not poisoned");
    if let Some(gate) = gates.get(&key).and_then(Weak::upgrade) {
        return gate;
    }
    let gate = Arc::new(TokioMutex::new(()));
    gates.insert(key, Arc::downgrade(&gate));
    gate
}
