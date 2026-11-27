use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::Result;
use crate::context::runtime::StartupCache;
use crate::event::EventStore;
use crate::event::store::{JournalSummary, Replay};

use super::jobs::JobReplay;
use super::record::{AgentState, State};
use super::{InputStatus, reduce_event, retention};

#[derive(Serialize, Deserialize)]
pub(super) struct Projection {
    id: String,
    pub(super) state: State,
    pub(super) startup: StartupCache,
    pub(super) jobs: JobReplay,
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
    use crate::log::{BufferedLogWriter, LogLevel, LogRecord, LogScope, runtime_log_path};

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
    use crate::AgentConfig;
    use crate::event::EventPayload;
    use crate::session::runtime::record::Record;

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
    fn checkpoint_without_agent_creation_identity_is_rejected_for_journal_replay() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let outputs = dir.path().join("outputs");
        let mut writer = EventStore::open(&events).unwrap();
        let mut saved = Projection::new(&outputs, "session", 4096).unwrap();
        let created = ["main", "z-review", "a-tests"]
            .into_iter()
            .map(|address| {
                writer
                    .append(
                        Record::AgentCreated {
                            command_id: format!("create-{address}"),
                            fingerprint: "fingerprint".into(),
                            address: address.into(),
                            config: AgentConfig::default(),
                        }
                        .payload()
                        .unwrap(),
                    )
                    .unwrap()
            })
            .collect::<Vec<_>>();
        for event in &created {
            saved.observe(Replay::Event(event.clone()), None).unwrap();
        }
        let mut old = serde_json::to_value(&saved).unwrap();
        for agent in old["state"]["agents"].as_object_mut().unwrap().values_mut() {
            agent["snapshot"]
                .as_object_mut()
                .unwrap()
                .remove("creation_event_id");
        }
        let checkpoint = serde_json::value::to_raw_value(&old).unwrap();
        let mut recovered = Projection::new(&outputs, "session", 4096).unwrap();
        assert!(
            !recovered
                .observe(Replay::Checkpoint(&checkpoint), None)
                .unwrap()
        );
        assert!(super::decode_listing(Default::default(), &checkpoint).is_none());
        for event in created {
            recovered.observe(Replay::Event(event), None).unwrap();
        }
        let mut agents = recovered.state.snapshots();
        agents.sort_by_key(|agent| agent.creation_event_id);
        assert_eq!(
            [("main", 1), ("z-review", 2), ("a-tests", 3)],
            agents
                .iter()
                .map(|agent| (agent.address.as_str(), agent.creation_event_id))
                .collect::<Vec<_>>()
                .as_slice()
        );
    }

    #[test]
    fn legacy_checkpoint_replays_interruption_disposition_after_a_second_stop() {
        let dir = tempfile::tempdir().unwrap();
        let outputs = dir.path().join("outputs");
        let mut writer = EventStore::open(dir.path().join("events.jsonl")).unwrap();
        let records = [
            Record::AgentCreated {
                command_id: "create-main".into(),
                fingerprint: "fingerprint".into(),
                address: "main".into(),
                config: AgentConfig::default(),
            },
            Record::ExecutionStarted {
                address: "main".into(),
            },
        ];
        let mut events = records
            .into_iter()
            .map(|record| writer.append(record.payload().unwrap()).unwrap())
            .collect::<Vec<_>>();
        for continue_queued in [true, false] {
            events.push(
                writer
                    .append(crate::event::EventPayload::Unknown(serde_json::json!({
                        "kind":"runtime.operation","ts":"2026-11-25T00:00:00Z","record":{
                            "operation":"agent_stopped","address":"main","cleanup_pending":true,
                            "continue_queued":continue_queued,
                        },
                    })))
                    .unwrap(),
            );
        }
        // Neither a checkpoint saved after the first interrupt nor one saved
        // after a subsequent Stop can reconstruct the new durable disposition.
        for prefix in [3, 4] {
            let mut saved = Projection::new(&outputs, "session", 4096).unwrap();
            for event in &events[..prefix] {
                saved.observe(Replay::Event(event.clone()), None).unwrap();
            }
            let mut old = serde_json::to_value(&saved).unwrap();
            old["state"]["agents"]["main"]
                .as_object_mut()
                .unwrap()
                .remove("discard_cancelled_turn");
            let checkpoint = serde_json::value::to_raw_value(&old).unwrap();
            let mut recovered = Projection::new(&outputs, "session", 4096).unwrap();
            assert!(
                !recovered
                    .observe(Replay::Checkpoint(&checkpoint), None)
                    .unwrap()
            );
            assert!(super::decode_listing(Default::default(), &checkpoint).is_none());
            for event in &events {
                recovered
                    .observe(Replay::Event(event.clone()), None)
                    .unwrap();
            }
            let agent = recovered.state.agent("main").unwrap();
            assert!(!agent.continue_queued);
            assert!(agent.discard_on_resume());
            recovered
                .state
                .reduce(
                    5,
                    Record::AgentResumed {
                        address: "main".into(),
                        command: None,
                        restore: true,
                    },
                )
                .unwrap();
            assert!(!recovered.state.agent("main").unwrap().discard_on_resume());
        }
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
                images: Vec::new(),
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
        assert!(
            !packing
                .observe(Replay::Checkpoint(&checkpoint), Some(new_limit))
                .unwrap()
        );
    }
}
