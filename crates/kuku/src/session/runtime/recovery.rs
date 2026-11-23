use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::context::runtime::StartupCache;
use crate::event::store::{JournalSummary, Replay};
use crate::event::EventStore;
use crate::Result;

use super::jobs::JobReplay;
use super::record::{AgentState, State};
use super::{reduce_event, retention, InputStatus};

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

    fn observe(
        &mut self,
        replay: Replay<'_>,
        required_startup_limit: Option<usize>,
    ) -> Result<bool> {
        match replay {
            Replay::Checkpoint(bytes) => {
                let Ok(mut projected) = serde_json::from_str::<Self>(bytes.get()) else {
                    return Ok(false);
                };
                if projected.id != self.id
                    || required_startup_limit
                        .is_some_and(|limit| !projected.startup.accepts_limit(limit))
                {
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
    let writer = EventStore::open_replaying(events, |replay| checkpoint.observe(replay, None))?;
    checkpoint.startup = checkpoint.startup.at_limit(max_bytes);
    Ok((writer, checkpoint))
}

pub(super) fn pack(writer: EventStore, events: &Path, id: &str, max_bytes: usize) -> Result<()> {
    let output_dir = retention::output_dir(events)?;
    let mut checkpoint = Projection::new(&output_dir, id, max_bytes)?;
    let head = writer.visit_recovery(|replay| checkpoint.observe(replay, Some(max_bytes)))?;
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

pub(super) fn read_state(events: &Path, state: State) -> Result<(State, u64)> {
    recover_state(events, state, false).map(|(state, cursor, _)| (state, cursor))
}

pub(super) fn read_listing(events: &Path) -> Result<(State, JournalSummary)> {
    if let Some(listing) = EventStore::read_sealed_summary(events, decode_listing)? {
        return Ok(listing);
    }
    recover_state(events, State::default(), true).map(|(state, _, summary)| (state, summary))
}

fn decode_listing(summary: JournalSummary, runtime: &RawValue) -> Option<(State, JournalSummary)> {
    #[derive(Deserialize)]
    struct ListingCheckpoint {
        state: ListingState,
    }

    #[derive(Deserialize)]
    struct ListingState {
        agents: BTreeMap<String, AgentState>,
        deliveries: HashMap<String, InputStatus>,
    }

    let saved = serde_json::from_str::<ListingCheckpoint>(runtime.get()).ok()?;
    let mut state = State {
        agents: saved.state.agents,
        deliveries: saved.state.deliveries,
        ..State::default()
    };
    state.mark_interrupted();
    Some((state, summary))
}

fn recover_state(
    events: &Path,
    mut state: State,
    with_summary: bool,
) -> Result<(State, u64, JournalSummary)> {
    let mut job_replay = JobReplay::default();
    let visit = |item: Replay<'_>| {
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
    };
    let (cursor, summary) = if with_summary {
        EventStore::visit_summary_path(events, visit)?
    } else {
        (
            EventStore::visit_recovery_path(events, visit)?,
            JournalSummary::default(),
        )
    };
    state.mark_interrupted();
    Ok((state, cursor, summary))
}

#[cfg(test)]
mod tests {
    use super::{EventStore, Projection, Replay, StartupCache};
    use crate::event::EventPayload;
    use crate::session::runtime::record::Record;
    use crate::AgentConfig;

    #[test]
    fn sealed_listing_reads_only_the_state_it_projects() {
        let runtime = serde_json::value::to_raw_value(&serde_json::json!({
            "state": {"agents": {}, "deliveries": {}, "turns": "unreadable"},
            "jobs": "unreadable",
        }))
        .unwrap();
        let summary = super::JournalSummary {
            title: Some("Saved title".into()),
            ..Default::default()
        };
        let (state, summary) = super::decode_listing(summary, &runtime).unwrap();
        assert!(state.snapshots().is_empty());
        assert_eq!(Some("Saved title"), summary.title.as_deref());
    }

    #[test]
    fn changing_request_limit_keeps_committed_state_and_rebuilds_startup_facts() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let outputs = dir.path().join("outputs");
        let mut writer = EventStore::open(&events).unwrap();
        let original_limit = 4096;
        let new_limit = 8192;
        let created = writer
            .append(
                Record::AgentCreated {
                    command_id: "create".into(),
                    fingerprint: "fingerprint".into(),
                    address: "main".into(),
                    config: AgentConfig::default(),
                }
                .payload()
                .unwrap(),
            )
            .unwrap();
        let started = writer
            .append(EventPayload::TurnStarted {
                ts: String::new(),
                conversation: "main".into(),
                turn: 1,
            })
            .unwrap();
        let prompt = writer
            .append(EventPayload::MessageUser {
                ts: String::new(),
                conversation: "main".into(),
                turn: 1,
                text: "a".repeat(20_000),
                from: None,
                job_delivery: false,
            })
            .unwrap();
        let mut saved = Projection::new(&outputs, "session", original_limit).unwrap();
        for event in [created, started, prompt.clone()] {
            saved.observe(Replay::Event(event), None).unwrap();
        }
        assert!(saved.startup.read(&events, "main").is_err());
        let checkpoint = serde_json::value::to_raw_value(&saved).unwrap();
        let mut live = Projection::new(&outputs, "session", new_limit).unwrap();
        assert!(live.observe(Replay::Checkpoint(&checkpoint), None).unwrap());
        assert_eq!(
            live.state
                .retry("create", "fingerprint")
                .unwrap()
                .unwrap()
                .event_id,
            1
        );
        let actual = live
            .startup
            .at_limit(new_limit)
            .read(&events, "main")
            .unwrap();
        let expected = StartupCache::new(new_limit).read(&events, "main").unwrap();
        assert!(actual.contains(&prompt));
        assert_eq!(actual, expected);

        let mut packing = Projection::new(&outputs, "session", new_limit).unwrap();
        assert!(!packing
            .observe(Replay::Checkpoint(&checkpoint), Some(new_limit))
            .unwrap());
    }
}
