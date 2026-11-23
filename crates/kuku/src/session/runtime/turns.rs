use std::collections::HashMap;
use std::path::Path;

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::event::store::Replay;
use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::Result;

use super::record::Record;
use super::{AgentSnapshot, AgentStatus, CacheWarning, TurnSummary, UsageTotals};

mod cache;
use cache::{CacheHistory, TurnCache};

/// Per-Agent Turn numbering and terminal statistics derived from committed facts.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TurnIndex {
    by_agent: HashMap<String, Vec<IndexedTurn>>,
    summaries: HashMap<u64, (u64, TurnSummary)>,
    cache: HashMap<String, CacheHistory>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct IndexedTurn {
    id: u64,
    start: Option<u64>,
    tool_calls: u64,
    end: Option<TurnEnd>,
    cache: TurnCache,
    preview: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TurnEnd {
    event_id: u64,
    explicit_stop: bool,
    can_continue: bool,
}

/// A Turn that can be selected when browsing conversation history.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HistoryPoint {
    /// Journal cursor at the end of the Turn, or its admission when still open.
    pub event_id: u64,
    /// Whether the Agent can continue after this cursor.
    pub can_continue: bool,
    /// Durable Turn identity.
    pub turn: u64,
    /// One-based display number within the Agent.
    pub number: u64,
    /// Short preview of the input, when available.
    pub preview: Option<String>,
}

impl TurnIndex {
    /// Rebuild the Turn index and its source cursor without acquiring a runtime writer.
    pub fn read(events_path: &Path) -> Result<(Self, u64)> {
        let mut index = Self::default();
        let cursor = EventStore::visit_recovery_path(events_path, |item| index.replay(item))?;
        Ok((index, cursor))
    }

    pub(super) fn replay(&mut self, item: Replay<'_>) -> Result<bool> {
        match item {
            Replay::Checkpoint(raw) => {
                #[derive(serde::Deserialize)]
                struct Checkpoint {
                    state: SavedState,
                }
                #[derive(serde::Deserialize)]
                struct SavedState {
                    turns: TurnIndex,
                }
                let Ok(saved) = serde_json::from_str::<Checkpoint>(raw.get()) else {
                    return Ok(false);
                };
                *self = saved.state.turns;
            }
            Replay::Event(event) => {
                let record = if matches!(&event.payload, EventPayload::Unknown(value)
                    if value["kind"] == "runtime.operation" && matches!(value["record"]["operation"].as_str(),
                        Some("inputs_accepted" | "turn_settled" | "agent_stopped")))
                {
                    Record::from_event(&event)?
                } else {
                    None
                };
                self.observe(&event, record.as_ref());
            }
        }
        Ok(true)
    }

    /// Return the one-based display number for a known Turn belonging to an Agent.
    pub fn number(&self, address: &str, turn: u64) -> Option<u64> {
        let (position, _) = self.indexed(address, turn)?;
        Some(position as u64 + 1)
    }

    pub(super) fn can_continue_after(&self, address: &str, event_id: u64) -> Option<u64> {
        let (turn, _) = self.summaries.get(&event_id)?;
        let end = self.indexed(address, *turn)?.1.end.as_ref()?;
        (end.event_id == event_id && end.can_continue).then_some(*turn)
    }

    /// List conversation points, newest first, from any branch of this Agent.
    pub fn history_points(&self, address: &str, before: u64, limit: usize) -> Vec<HistoryPoint> {
        self.by_agent
            .get(address)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .enumerate()
            .rev()
            .filter_map(|(index, turn)| {
                let event_id = turn.end.as_ref().map_or(turn.id, |end| end.event_id);
                let can_continue = turn.end.as_ref().is_some_and(|end| end.can_continue);
                (event_id < before).then(|| HistoryPoint {
                    event_id,
                    can_continue,
                    turn: turn.id,
                    number: index as u64 + 1,
                    preview: turn.preview.clone(),
                })
            })
            .take(limit)
            .collect()
    }

    pub(super) fn previous(&self, address: &str, turn: u64) -> Option<u64> {
        let (position, _) = self.indexed(address, turn)?;
        Some(
            self.by_agent
                .get(address)?
                .get(position.checked_sub(1)?)?
                .id,
        )
    }

    /// Return the durable first admission time in Unix milliseconds.
    pub fn started_at_ms(&self, address: &str, turn: u64) -> Option<u64> {
        self.indexed(address, turn)?.1.start
    }

    /// Return the Turn identity and summary carried by the terminal event, if any.
    pub fn summary_at(&self, event_id: u64) -> Option<(u64, TurnSummary)> {
        self.summaries.get(&event_id).cloned()
    }

    pub(super) fn settlement(&self, address: &str, turn: u64) -> Option<(u64, TurnSummary)> {
        let end = self.indexed(address, turn)?.1.end.as_ref()?;
        Some((end.event_id, self.summaries.get(&end.event_id)?.1.clone()))
    }

    /// Return whether the Turn ended with a committed explicit stop.
    pub fn explicitly_stopped(&self, address: &str, turn: u64) -> bool {
        self.indexed(address, turn)
            .and_then(|(_, turn)| turn.end.as_ref())
            .is_some_and(|end| end.explicit_stop)
    }

    pub(super) fn time_snapshot(&self, snapshot: &mut AgentSnapshot) {
        snapshot.cache_warning = self
            .cache
            .get(&snapshot.address)
            .and_then(CacheHistory::warning);
        let Some((_, turn)) = snapshot
            .turn
            .and_then(|id| self.indexed(&snapshot.address, id))
        else {
            return;
        };
        snapshot.turn_started_at_ms = turn.start;
        snapshot.turn_elapsed_ms = turn
            .end
            .as_ref()
            .and_then(|end| self.summaries[&end.event_id].1.elapsed_ms);
    }

    pub(super) fn observe(&mut self, event: &StoredEvent, record: Option<&Record>) {
        match record {
            Some(Record::InputsAccepted {
                address,
                turn: None,
                ..
            }) => {
                self.by_agent
                    .entry(address.clone())
                    .or_default()
                    .push(IndexedTurn {
                        id: event.id,
                        start: event_ms(event),
                        tool_calls: 0,
                        end: None,
                        cache: TurnCache::default(),
                        preview: None,
                    });
            }
            Some(Record::TurnSettled {
                address,
                turn: Some(turn),
                usage,
                status,
                ..
            }) => {
                self.finish(address, *turn, event, usage, Some(*status));
            }
            Some(Record::AgentStopped {
                address,
                interruption: Some(stopped),
                ..
            }) => {
                if let Some(turn) = stopped.turn {
                    self.finish(address, turn, event, &stopped.usage, None);
                }
            }
            None => {
                if let EventPayload::MessageUser {
                    conversation,
                    turn,
                    text,
                    job_delivery: false,
                    ..
                } = &event.payload
                {
                    self.preview(conversation, *turn, text);
                }
                if let EventPayload::Unknown(value) = &event.payload {
                    self.observe_cache(value);
                }
                if let EventPayload::ToolCall {
                    conversation, turn, ..
                } = &event.payload
                {
                    let pending = self
                        .by_agent
                        .get_mut(conversation.as_deref().unwrap_or("main"));
                    if let Some(index) = pending
                        .and_then(|turns| turns.last_mut())
                        .filter(|index| index.id == *turn && index.end.is_none())
                    {
                        index.tool_calls += 1;
                    }
                }
            }
            _ => {}
        }
    }

    fn observe_cache(&mut self, value: &serde_json::Value) {
        if value["kind"] != "runtime.step_prepared" {
            return;
        }
        let (Some(address), Some(id)) = (value["agent"].as_str(), value["turn"].as_u64()) else {
            return;
        };
        let Some(turn) = self
            .by_agent
            .get_mut(address)
            .and_then(|turns| turns.last_mut())
            .filter(|turn| turn.id == id && turn.end.is_none())
        else {
            return;
        };
        turn.cache.prepare(value);
        if let Some(history) = self.cache.get_mut(address) {
            history.observe_prefix(&turn.cache);
        }
    }

    fn indexed(&self, address: &str, turn: u64) -> Option<(usize, &IndexedTurn)> {
        let turns = self.by_agent.get(address)?;
        let index = turns.binary_search_by_key(&turn, |known| known.id).ok()?;
        Some((index, &turns[index]))
    }

    pub(super) fn preview(&mut self, address: &str, turn: u64, text: &str) {
        if let Some(indexed) = self
            .by_agent
            .get_mut(address)
            .and_then(|turns| turns.last_mut())
            .filter(|known| known.id == turn)
        {
            indexed
                .preview
                .get_or_insert_with(|| text.chars().take(160).collect());
        }
    }

    fn finish(
        &mut self,
        address: &str,
        turn: u64,
        event: &StoredEvent,
        usage: &UsageTotals,
        settled: Option<AgentStatus>,
    ) {
        let explicit_stop = settled.is_none();
        let Some(turns) = self.by_agent.get_mut(address) else {
            return;
        };
        let Ok(index) = turns.binary_search_by_key(&turn, |known| known.id) else {
            return;
        };
        let known = &mut turns[index];
        if known.end.is_some() {
            return;
        }
        let cache = std::mem::take(&mut known.cache);
        let summary = TurnSummary {
            elapsed_ms: known
                .start
                .and_then(|start| event_ms(event)?.checked_sub(start)),
            input_tokens: usage.input_tokens.filter(|_| usage.input_tokens_complete()),
            output_tokens: usage
                .output_tokens
                .filter(|_| usage.output_tokens_complete()),
            cache_hit_percent: usage.cache_hit_percent(),
            low_cache_hit: !explicit_stop
                && self
                    .cache
                    .entry(address.to_owned())
                    .or_default()
                    .finish(turn, cache, usage),
            tool_calls: known.tool_calls,
        };
        known.end = Some(TurnEnd {
            event_id: event.id,
            explicit_stop,
            can_continue: settled == Some(AgentStatus::Idle),
        });
        self.summaries.insert(event.id, (turn, summary));
    }
}

fn event_ms(event: &StoredEvent) -> Option<u64> {
    let EventPayload::Unknown(value) = &event.payload else {
        return None;
    };
    epoch_ms(OffsetDateTime::parse(value["ts"].as_str()?, &Rfc3339).ok()?)
}

fn epoch_ms(time: OffsetDateTime) -> Option<u64> {
    u64::try_from(time.unix_timestamp_nanos().div_euclid(1_000_000)).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::AgentStatus;
    use super::{EventPayload, EventStore, Record, TurnIndex, UsageTotals};

    fn append(store: &mut EventStore, second: u8, record: Record) -> u64 {
        store
            .append(EventPayload::Unknown(json!({
                "kind":"runtime.operation",
                "ts":format!("2026-10-01T01:30:{second:02}Z"),
                "record":record,
            })))
            .unwrap()
            .id
    }

    fn accept(store: &mut EventStore, second: u8, address: &str, turn: Option<u64>) -> u64 {
        append(
            store,
            second,
            Record::InputsAccepted {
                address: address.into(),
                input_ids: vec!["test".into()],
                message_input_ids: Vec::new(),
                turn,
            },
        )
    }

    fn finish(
        store: &mut EventStore,
        second: u8,
        address: &str,
        turn: u64,
        usage: UsageTotals,
    ) -> u64 {
        append(
            store,
            second,
            Record::TurnSettled {
                address: address.into(),
                status: AgentStatus::Idle,
                turn: Some(turn),
                output: Some("complete".into()),
                error: None,
                usage,
            },
        )
    }

    #[test]
    fn replay_preserves_each_agents_first_admission_and_available_usage() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("events.jsonl");
        let mut store = EventStore::open(&path).unwrap();
        let main = accept(&mut store, 1, "main", None);
        let peer = accept(&mut store, 2, "reviewer", None);
        accept(&mut store, 3, "main", Some(main));
        let peer_usage = UsageTotals {
            model_requests: 2,
            unreported_requests: 1,
            input_tokens: Some(50),
            input_reported_requests: 1,
            cache_measured_input_tokens: 50,
            cache_measured_read_tokens: 10,
            ..UsageTotals::default()
        };
        let peer_end = finish(&mut store, 5, "reviewer", peer, peer_usage);
        let main_usage = UsageTotals {
            model_requests: 2,
            input_tokens: Some(80),
            output_tokens: Some(9),
            input_reported_requests: 2,
            output_reported_requests: 2,
            cache_measured_input_tokens: 80,
            cache_measured_read_tokens: 30,
            ..UsageTotals::default()
        };
        let main_end = finish(&mut store, 7, "main", main, main_usage);
        drop(store);

        let (index, cursor) = TurnIndex::read(&path).unwrap();
        assert_eq!(main_end, cursor);
        assert_eq!(Some(1), index.number("main", main));
        assert_eq!(Some(1), index.number("reviewer", peer));
        assert_eq!(None, index.number("reviewer", main));
        let (turn, summary) = index.summary_at(main_end).unwrap();
        assert_eq!(main, turn);
        assert_eq!(Some(6_000), summary.elapsed_ms);
        assert_eq!(Some(80), summary.input_tokens);
        assert_eq!(Some(9), summary.output_tokens);
        assert_eq!(Some(37.5), summary.cache_hit_percent);
        let (turn, summary) = index.summary_at(peer_end).unwrap();
        assert_eq!(peer, turn);
        assert_eq!(Some(3_000), summary.elapsed_ms);
        assert_eq!(None, summary.input_tokens);
        assert_eq!(None, summary.output_tokens);
        assert_eq!(Some(20.0), summary.cache_hit_percent);
    }

    #[test]
    fn cache_warnings_replay_per_agent_and_reset_when_the_prefix_changes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("events.jsonl");
        let mut store = EventStore::open(&path).unwrap();
        let mut settled = Vec::new();
        let mut last_warning = None;
        for (index, (segment, read)) in [
            (1, Some(0)),
            (1, Some(820)),
            (1, Some(370)),
            (1, Some(0)),
            (1, Some(900)),
            (1, None),
            (1, Some(400)),
            (1, Some(200)),
            (2, Some(0)),
        ]
        .into_iter()
        .enumerate()
        {
            let second = index as u8 + 1;
            let turn = accept(&mut store, second, "main", None);
            store
                .append(EventPayload::Unknown(json!({
                    "kind":"runtime.step_prepared", "agent":"main", "turn":turn, "segment":segment,
                    "prefix_hash":"fixed system", "request_id": format!("request-{turn}"),
                })))
                .unwrap();
            let usage = UsageTotals {
                cache_measured_input_tokens: u64::from(read.is_some()) * 1000,
                cache_measured_read_tokens: read.unwrap_or_default(),
                ..UsageTotals::default()
            };
            settled.push(finish(&mut store, second, "main", turn, usage));
            let (replayed, _) = TurnIndex::read(&path).unwrap();
            let warning = replayed.cache["main"].warning();
            if index == 3 {
                let witness = warning.as_ref().unwrap();
                assert_eq!(
                    (0, 370, 2000),
                    (
                        witness.latest_percent,
                        witness.read_tokens,
                        witness.input_tokens
                    )
                );
                last_warning = Some(witness.since_turn);
            } else if index == 7 {
                assert_ne!(last_warning, Some(warning.unwrap().since_turn));
            } else {
                assert!(warning.is_none());
            }
            assert!(!replayed.cache.contains_key("reviewer"));
        }
        let (index, _) = TurnIndex::read(&path).unwrap();
        let flags: Vec<_> = settled
            .iter()
            .map(|id| index.summary_at(*id).unwrap().1.low_cache_hit)
            .collect();
        assert_eq!(
            flags,
            [false, false, true, true, false, false, true, true, false]
        );
    }
}
