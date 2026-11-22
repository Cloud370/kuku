use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::context::runtime::StartupCache;
use crate::event::store::Replay;
use crate::event::EventStore;
use crate::Result;

use super::jobs::JobReplay;
use super::record::State;
use super::{reduce_event, retention};

#[derive(Serialize, Deserialize)]
pub(super) struct Projection {
    id: String,
    pub(super) state: State,
    pub(super) startup: StartupCache,
    jobs: JobReplay,
}

impl Projection {
    fn new(output_dir: &Path, id: &str, max_bytes: usize) -> Result<Self> {
        Ok(Self {
            id: id.into(),
            state: State {
                outputs: retention::OutputStore::recover(output_dir)?,
                ..State::default()
            },
            startup: StartupCache::new(max_bytes),
            jobs: JobReplay::default(),
        })
    }

    fn observe(&mut self, replay: Replay<'_>, max_bytes: usize) -> Result<bool> {
        match replay {
            Replay::Checkpoint(bytes) => {
                let Ok(mut projected) = serde_json::from_str::<Self>(bytes.get()) else {
                    return Ok(false);
                };
                if projected.id != self.id || !projected.startup.accepts_limit(max_bytes) {
                    return Ok(false);
                }
                let mut outputs = std::mem::take(&mut self.state.outputs);
                outputs.restore_ownership(std::mem::take(&mut projected.state.outputs));
                projected.state.outputs = outputs;
                *self = projected;
            }
            Replay::Event(event) => {
                self.startup.observe(&event);
                reduce_event(&mut self.state, &mut self.jobs, event)?;
            }
        }
        Ok(true)
    }
}

pub(super) fn open(
    events: &Path,
    output_dir: &Path,
    id: &str,
    max_bytes: usize,
) -> Result<(EventStore, Projection)> {
    let mut checkpoint = Projection::new(output_dir, id, max_bytes)?;
    let writer =
        EventStore::open_replaying(events, |replay| checkpoint.observe(replay, max_bytes))?;
    Ok((writer, checkpoint))
}

pub(super) fn pack(writer: EventStore, events: &Path, id: &str, max_bytes: usize) -> Result<()> {
    let output_dir = retention::output_dir(events)?;
    let mut checkpoint = Projection::new(&output_dir, id, max_bytes)?;
    let head = writer.visit_recovery(|replay| checkpoint.observe(replay, max_bytes))?;
    writer.pack_closed(head, &checkpoint)?;
    Ok(())
}

pub(super) fn report_pack_failure(data_dir: &Path, id: &str, error: impl std::fmt::Display) {
    use crate::log::{runtime_log_path, BufferedLogWriter, LogLevel, LogRecord, LogScope};

    let now = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    if let Some(day) = now.get(..10) {
        if let Ok(path) = runtime_log_path(data_dir, day) {
            let mut record = LogRecord::new(now, LogLevel::Warn, LogScope::Runtime);
            record.kind = "runtime.journal_pack_failed".into();
            record.message = error.to_string();
            record.session_id = Some(id.into());
            let _ = BufferedLogWriter::new(path).push(record);
        }
    }
}

#[derive(Deserialize)]
struct StateCheckpoint {
    state: State,
    jobs: JobReplay,
}

pub(super) fn read_state(events: &Path, mut state: State) -> Result<(State, u64)> {
    let mut job_replay = JobReplay::default();
    let cursor = EventStore::visit_recovery_path(events, |item| {
        match item {
            Replay::Checkpoint(raw) => {
                let Ok(mut saved) = serde_json::from_str::<StateCheckpoint>(raw.get()) else {
                    return Ok(false);
                };
                let mut outputs = std::mem::take(&mut state.outputs);
                outputs.restore_ownership(std::mem::take(&mut saved.state.outputs));
                saved.state.outputs = outputs;
                state = saved.state;
                job_replay = saved.jobs;
            }
            Replay::Event(event) => reduce_event(&mut state, &mut job_replay, event)?,
        }
        Ok(true)
    })?;
    state.mark_interrupted();
    Ok((state, cursor))
}
