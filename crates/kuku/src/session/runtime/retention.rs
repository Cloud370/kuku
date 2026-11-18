use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Weak;
use std::time::SystemTime;

use crate::event::EventPayload;
use crate::tool::builtin::run_command::output::{framed_bytes, output_path};
use crate::util::fs_sync;
use crate::{Error, Result};

use super::record::Record;
use super::{ensure_open, AgentStatus, Session, SessionInner};

#[derive(Debug, Default)]
pub(super) struct OutputStore {
    entries: HashMap<String, OutputEntry>,
    charged_bytes: u64,
}

#[derive(Debug)]
struct OutputEntry {
    owner: Option<String>,
    bytes: u64,
    retained_since: SystemTime,
    writing: bool,
}

pub(crate) struct OutputReservation {
    inner: Weak<SessionInner>,
    dir: PathBuf,
    id: String,
    max_bytes: u64,
    keep: bool,
}

impl OutputReservation {
    pub(crate) fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    pub(crate) fn keep(&mut self) {
        self.keep = true;
    }

    pub(crate) fn discard_on_drop(&mut self) {
        self.keep = false;
    }
}

impl Drop for OutputReservation {
    fn drop(&mut self) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let path = output_path(&self.dir, &self.id).expect("reserved output ID was validated");
        inner
            .state
            .lock()
            .unwrap()
            .outputs
            .finish(&self.id, &path, self.keep);
    }
}

impl Session {
    pub(crate) fn reserve_output(&self, agent: &str, id: &str) -> Result<OutputReservation> {
        self.reserve_retained_output(agent, id, None)
    }

    pub(super) fn reserve_retained_output(
        &self,
        agent: &str,
        id: &str,
        exact_bytes: Option<u64>,
    ) -> Result<OutputReservation> {
        let dir = output_dir(self.events_path())?;
        let path = output_path(&dir, id)?;
        let limits = &self.inner.options.limits;
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        let snapshot = &state.agent(agent)?.snapshot;
        let turn = snapshot
            .turn
            .filter(|_| snapshot.status == AgentStatus::Running);
        let pinned = state.values.pinned_outputs();
        state
            .outputs
            .collect(&dir, limits.output_retention_secs, &pinned)?;
        let available = limits
            .max_retained_output_bytes
            .saturating_sub(state.outputs.charged_bytes)
            .min(limits.max_output_bytes);
        let max_bytes = exact_bytes.unwrap_or(available);
        if max_bytes < framed_bytes(0) || max_bytes > available {
            return Err(Error::OutputQuotaExceeded { available });
        }
        if state.outputs.entries.contains_key(id) || path.try_exists()? {
            return Err(Error::InvalidArgument(format!(
                "output '{id}' already exists"
            )));
        }
        if exact_bytes.is_none() {
            self.commit(
                &mut state,
                Record::OutputOwned {
                    agent: agent.into(),
                    id: id.into(),
                    turn,
                },
            )?;
        }
        state.outputs.begin(agent, id, max_bytes);
        Ok(OutputReservation {
            inner: std::sync::Arc::downgrade(&self.inner),
            dir,
            id: id.into(),
            max_bytes,
            keep: exact_bytes.is_none(),
        })
    }
}

impl OutputStore {
    pub(super) fn one(id: &str, metadata: fs::Metadata) -> Self {
        Self {
            charged_bytes: metadata.len(),
            entries: HashMap::from([(id.into(), file_entry(metadata, None))]),
        }
    }

    pub(super) fn discover(dir: &Path) -> Result<Self> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error.into()),
        };
        let mut store = Self::default();
        for entry in entries {
            let entry = entry?;
            let filename = entry.file_name();
            if let Some(id) = filename
                .to_str()
                .and_then(|name| name.strip_suffix(".output"))
            {
                if output_path(dir, id).is_ok() && entry.file_type()?.is_file() {
                    let metadata = entry.metadata()?;
                    store.charged_bytes = store.charged_bytes.saturating_add(metadata.len());
                    store.entries.insert(id.into(), file_entry(metadata, None));
                }
            }
        }
        Ok(store)
    }

    pub(super) fn claim(&mut self, agent: &str, id: &str) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.owner.get_or_insert_with(|| agent.into());
        }
    }

    pub(super) fn observe_dispatch(&mut self, payload: &EventPayload) {
        if self.entries.is_empty() {
            return;
        }
        let EventPayload::Unknown(value) = payload else {
            return;
        };
        let Some(agent) = value["agent"].as_str() else {
            return;
        };
        match value["kind"].as_str() {
            Some("runtime.tool_dispatched") => {
                if let (Some(turn), Some(id)) =
                    (value["turn"].as_u64(), value["tool_call_id"].as_str())
                {
                    self.claim(agent, &command_output_id(agent, turn, id));
                }
            }
            Some("workspace.action") => {
                if let Some(id) = value["facts"]["output_ref"]["id"].as_str() {
                    self.claim(agent, id);
                }
            }
            _ => {}
        };
    }

    pub(super) fn owned_by(&self, agent: &str, id: &str) -> bool {
        self.entries
            .get(id)
            .is_some_and(|entry| entry.owner.as_deref() == Some(agent))
    }

    pub(super) fn writing(&self, id: &str) -> bool {
        self.entries.get(id).is_some_and(|entry| entry.writing)
    }

    pub(super) fn collect(
        &mut self,
        dir: &Path,
        retention_secs: u64,
        pinned: &HashSet<String>,
    ) -> Result<()> {
        let now = SystemTime::now();
        let mut failure = None;
        self.entries.retain(|id, entry| {
            let expired = retention_secs == 0
                || now
                    .duration_since(entry.retained_since)
                    .is_ok_and(|age| age.as_secs() >= retention_secs);
            if entry.writing || pinned.contains(id) || (entry.owner.is_some() && !expired) {
                return true;
            }
            let path = output_path(dir, id).expect("catalogued output ID was validated");
            match reclaim_file(&path) {
                Ok(true) => {
                    self.charged_bytes -= entry.bytes;
                    false
                }
                Ok(false) => true,
                Err(error) => {
                    failure.get_or_insert(error);
                    true
                }
            }
        });
        failure.map_or(Ok(()), |error| Err(error.into()))
    }

    fn begin(&mut self, agent: &str, id: &str, bytes: u64) {
        self.entries.insert(
            id.into(),
            OutputEntry {
                owner: Some(agent.into()),
                bytes,
                retained_since: SystemTime::now(),
                writing: true,
            },
        );
        self.charged_bytes += bytes;
    }

    fn finish(&mut self, id: &str, path: &Path, keep: bool) {
        if !keep {
            let _ = reclaim_file(path);
        }
        let Some(entry) = self.entries.get_mut(id) else {
            return;
        };
        entry.writing = false;
        match fs::metadata(path) {
            Ok(metadata) => {
                self.charged_bytes = self.charged_bytes - entry.bytes + metadata.len();
                *entry = file_entry(metadata, entry.owner.take());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.charged_bytes -= entry.bytes;
                self.entries.remove(id);
            }
            Err(_) => {}
        }
    }
}

pub(super) fn output_dir(events_path: &Path) -> Result<PathBuf> {
    Ok(events_path
        .parent()
        .ok_or_else(|| Error::InvalidArgument("events path has no session directory".into()))?
        .join("outputs"))
}

pub(super) fn open_retained_output(dir: &Path, id: &str) -> Result<fs::File> {
    let path = output_path(dir, id).map_err(|error| Error::InvalidArgument(error.to_string()))?;
    let file = fs::File::open(&path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => Error::OutputUnavailable(id.into()),
        _ => error.into(),
    })?;
    fs_sync::lock_reader(&file, &path).map_err(|error| match error.kind() {
        io::ErrorKind::WouldBlock => Error::OutputUnavailable(id.into()),
        _ => error.into(),
    })?;
    Ok(file)
}

pub(crate) fn command_output_id(agent: &str, turn: u64, id: &str) -> String {
    use sha2::{Digest, Sha256};

    format!(
        "{:x}",
        Sha256::digest(format!("{agent}:{turn}:{id}").as_bytes())
    )
}

fn file_entry(metadata: fs::Metadata, owner: Option<String>) -> OutputEntry {
    OutputEntry {
        owner,
        bytes: metadata.len(),
        retained_since: metadata.modified().unwrap_or_else(|_| SystemTime::now()),
        writing: false,
    }
}

fn reclaim_file(path: &Path) -> io::Result<bool> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    if let Err(error) = fs_sync::lock_writer(&file, path) {
        return if error.kind() == io::ErrorKind::WouldBlock {
            Ok(false)
        } else {
            Err(error)
        };
    }
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        outcome => outcome.map(|()| true),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::{open_retained_output, output_path, OutputStore};

    #[test]
    fn expired_output_with_an_active_reader_is_still_charged() {
        let dir = tempfile::tempdir().unwrap();
        let path = output_path(dir.path(), "capture").unwrap();
        std::fs::write(&path, b"captured bytes").unwrap();
        let mut store = OutputStore::discover(dir.path()).unwrap();
        store.claim("main", "capture");
        store.entries.get_mut("capture").unwrap().retained_since =
            SystemTime::now() + Duration::from_secs(60);
        let reader = open_retained_output(dir.path(), "capture").unwrap();
        store.collect(dir.path(), 0, &Default::default()).unwrap();
        assert!(path.exists());
        assert_eq!(14, store.charged_bytes);
        drop(reader);
        store.collect(dir.path(), 0, &Default::default()).unwrap();
        assert!(!path.exists());
        assert_eq!(0, store.charged_bytes);
    }
}
