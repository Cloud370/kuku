use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Weak;
use std::time::SystemTime;

use crate::event::EventPayload;
use crate::tool::builtin::run_command::output::{
    framed_bytes, output_path, validate_output_id, OUTPUT_DRAFT_SUFFIX,
};
use crate::{Error, Result};

use super::jobs::JobIndex;
use super::record::Record;
use super::{ensure_open, Session, SessionInner};

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct OutputStore {
    entries: HashMap<String, OutputEntry>,
    charged_bytes: u64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct OutputEntry {
    owner: Option<String>,
    origin: Option<CaptureOrigin>,
    bytes: u64,
    retained_since: SystemTime,
    writing: bool,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct CaptureOrigin {
    pub(super) turn: u64,
    pub(super) tool_call_id: String,
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
    pub(crate) fn reserve_output(
        &self,
        agent: &str,
        turn: u64,
        tool_call_id: &str,
        id: &str,
    ) -> Result<OutputReservation> {
        self.reserve_retained_output(
            agent,
            id,
            None,
            Some(CaptureOrigin {
                turn,
                tool_call_id: tool_call_id.into(),
            }),
        )
    }

    pub(super) fn reserve_retained_output(
        &self,
        agent: &str,
        id: &str,
        exact_bytes: Option<u64>,
        origin: Option<CaptureOrigin>,
    ) -> Result<OutputReservation> {
        let dir = output_dir(self.events_path())?;
        let path = output_path(&dir, id)?;
        let limits = &self.inner.options.limits;
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        state.agent(agent)?;
        let mut pinned = state.pinned_outputs();
        if state.jobs.has_active() {
            pinned.extend(state.outputs.job_pins(&state.jobs));
        }
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
                    turn: origin.as_ref().map(|origin| origin.turn),
                    tool_call_id: origin.as_ref().map(|origin| origin.tool_call_id.clone()),
                },
            )?;
        }
        state.outputs.begin(agent, id, max_bytes, origin);
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
    pub(super) fn restore_ownership(&mut self, checkpoint: Self) {
        for (id, previous) in checkpoint.entries {
            if let Some(current) = self.entries.get_mut(&id) {
                current.owner = previous.owner;
                current.origin = previous.origin;
            }
        }
    }

    pub(super) fn one(id: &str, metadata: fs::Metadata) -> Self {
        Self {
            charged_bytes: metadata.len(),
            entries: HashMap::from([(id.into(), file_entry(metadata, None))]),
        }
    }

    pub(super) fn recover(dir: &Path) -> Result<Self> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error.into()),
        };
        let mut store = Self::default();
        for entry in entries {
            let entry = entry?;
            let filename = entry.file_name();
            let Some(filename) = filename.to_str() else {
                continue;
            };
            if let Some(id) = filename.strip_suffix(OUTPUT_DRAFT_SUFFIX) {
                if validate_output_id(id).is_ok() && entry.file_type()?.is_file() {
                    let path = entry.path();
                    match fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => {
                            return Err(io::Error::new(
                                error.kind(),
                                format!(
                                    "cannot remove interrupted output draft {}: {error}",
                                    path.display()
                                ),
                            )
                            .into());
                        }
                    }
                }
            } else if let Some(id) = filename.strip_suffix(".output") {
                if validate_output_id(id).is_ok() && entry.file_type()?.is_file() {
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

    pub(super) fn claim_capture(&mut self, agent: &str, id: &str, origin: Option<CaptureOrigin>) {
        if let Some(entry) = self.entries.get_mut(id) {
            if entry.owner.get_or_insert_with(|| agent.into()).as_str() == agent {
                entry.origin = origin;
            }
        }
    }

    fn job_pins<'a>(&'a self, jobs: &'a JobIndex) -> impl Iterator<Item = String> + 'a {
        self.entries.iter().filter_map(move |(id, entry)| {
            let owner = entry.owner.as_deref()?;
            let origin = entry.origin.as_ref()?;
            jobs.capture_active(owner, origin.turn, &origin.tool_call_id)
                .then(|| id.clone())
        })
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

    fn begin(&mut self, agent: &str, id: &str, bytes: u64, origin: Option<CaptureOrigin>) {
        self.entries.insert(
            id.into(),
            OutputEntry {
                owner: Some(agent.into()),
                origin,
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
                let origin = entry.origin.take();
                *entry = file_entry(metadata, entry.owner.take());
                entry.origin = origin;
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
    lock_output(&file, &path, OutputLock::Read).map_err(|error| match error.kind() {
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
        origin: None,
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
    if let Err(error) = lock_output(&file, path, OutputLock::Reclaim) {
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

enum OutputLock {
    Read,
    Reclaim,
}

#[cfg(not(windows))]
fn lock_output(file: &fs::File, path: &Path, mode: OutputLock) -> io::Result<()> {
    use crate::util::fs_sync;

    match mode {
        OutputLock::Read => fs_sync::lock_reader(file, path),
        OutputLock::Reclaim => fs_sync::lock_writer(file, path),
    }
}

#[cfg(windows)]
fn lock_output(file: &fs::File, path: &Path, mode: OutputLock) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows_sys::Win32::System::IO::{OVERLAPPED, OVERLAPPED_0, OVERLAPPED_0_0};

    use crate::tool::builtin::run_command::output::MAX_OUTPUT_BYTES;

    const LOCK_OFFSET: u64 = 1 << 32;
    const _: () = assert!(LOCK_OFFSET > MAX_OUTPUT_BYTES);
    let flags = LOCKFILE_FAIL_IMMEDIATELY
        | match mode {
            OutputLock::Read => 0,
            OutputLock::Reclaim => LOCKFILE_EXCLUSIVE_LOCK,
        };
    let mut range = OVERLAPPED {
        Anonymous: OVERLAPPED_0 {
            Anonymous: OVERLAPPED_0_0 {
                Offset: LOCK_OFFSET as u32,
                OffsetHigh: (LOCK_OFFSET >> 32) as u32,
            },
        },
        ..OVERLAPPED::default()
    };
    // SAFETY: The synchronous File handle and range outlive the immediate call.
    // Rust's whole-file locks would also block capture writes through other handles.
    if unsafe { LockFileEx(file.as_raw_handle(), flags, 0, 1, 0, &mut range) } != 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            format!("retained output {} is already locked", path.display()),
        ));
    }
    Err(error)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use crate::tool::builtin::run_command::output::{read_open_output, OutputLimits, OutputWriter};
    use crate::{OutputRead, OutputStream};

    use super::{open_retained_output, output_path, OutputStore};

    #[tokio::test]
    async fn capture_writes_continue_while_output_readers_hold_locks() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = OutputWriter::create(dir.path(), "capture", OutputLimits::default())
            .await
            .unwrap();
        let first = open_retained_output(dir.path(), "capture").unwrap();
        let second = open_retained_output(dir.path(), "capture").unwrap();
        writer
            .write(OutputStream::Stdout, b"capture")
            .await
            .unwrap();
        writer.finish(false).await.unwrap();
        for reader in [first, second] {
            let chunk = read_open_output(reader, "capture", OutputRead::default()).unwrap();
            assert!(chunk.complete);
            assert_eq!(b"capture", chunk.segments[0].as_bytes());
        }
    }

    #[test]
    fn expired_output_with_an_active_reader_is_still_charged() {
        let dir = tempfile::tempdir().unwrap();
        let path = output_path(dir.path(), "capture").unwrap();
        std::fs::write(&path, b"captured bytes").unwrap();
        let mut store = OutputStore::recover(dir.path()).unwrap();
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
