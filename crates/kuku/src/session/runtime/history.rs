use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::context::revert::{event_owner_with, RewindVisibility};
use crate::event::store::{HistoryMarks, TurnOwners};
use crate::event::{EventStore, StoredEvent};
use crate::Result;

use super::TurnIndex;

pub use crate::event::store::MessageRewind;

/// Agent ownership and rewind visibility for a committed Session history.
pub struct SessionHistory {
    path: PathBuf,
    cursor: u64,
    rewinds: Vec<MessageRewind>,
    owners: HashMap<u64, TurnOwners>,
    visibility: RewindVisibility,
}

impl SessionHistory {
    /// Read history visibility without opening a runtime writer.
    pub fn read(events_path: &Path) -> Result<Self> {
        Ok(Self::from_marks(
            events_path,
            EventStore::history_marks(events_path, true)?,
        ))
    }

    /// Read history visibility and turn numbering from the same committed journal prefix.
    pub fn read_with_turns(events_path: &Path) -> Result<(Self, TurnIndex)> {
        let mut turns = TurnIndex::default();
        let marks = EventStore::visit_history_path(events_path, |item| turns.replay(item))?;
        Ok((Self::from_marks(events_path, marks), turns))
    }

    fn from_marks(path: &Path, marks: HistoryMarks) -> Self {
        Self {
            path: path.to_owned(),
            cursor: marks.cursor,
            visibility: RewindVisibility::new(&marks.controls),
            owners: marks.turn_owners,
            rewinds: marks.message_rewinds,
        }
    }

    /// Determine whether a committed event remains in the active history.
    pub fn includes(&self, event: &StoredEvent) -> bool {
        self.visibility.includes(event, self.owner(event))
    }

    /// List retained message rewind groups in commit order.
    pub fn rewinds(&self) -> &[MessageRewind] {
        &self.rewinds
    }

    /// Last committed event represented by this history view.
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Read a bounded chronological page through this history's committed cursor.
    /// Use `includes` to test the active branch and `rewinds` to locate removed branches.
    pub fn page_before(&self, before: Option<u64>, limit: usize) -> Result<Vec<StoredEvent>> {
        let before = self
            .cursor
            .checked_add(1)
            .map_or(before, |end| Some(before.map_or(end, |id| id.min(end))));
        EventStore::read_page_before(&self.path, before, limit)
    }

    /// Read a bounded page of a rewind's events and the cursor for the next older scan.
    /// Nested rewind ranges belong to the earliest rewind that removed them.
    pub fn rewind_page(
        &self,
        rewind: &MessageRewind,
        before: Option<u64>,
        limit: usize,
    ) -> Result<(Vec<StoredEvent>, Option<u64>)> {
        let before = before.unwrap_or(rewind.event_id).min(rewind.event_id);
        if before <= rewind.target_event_id {
            return Ok((Vec::new(), None));
        }
        let previous = &self.rewinds[..self
            .rewinds
            .partition_point(|item| item.event_id < rewind.event_id)];
        let mut events = self.page_before(Some(before), limit)?;
        let next = events
            .first()
            .map(|event| event.id)
            .filter(|id| *id > rewind.target_event_id);
        events.retain(|event| {
            event.id >= rewind.target_event_id
                && self.owner(event) == Some(rewind.agent.as_str())
                && !previous.iter().any(|earlier| {
                    earlier.agent == rewind.agent
                        && (earlier.target_event_id..earlier.event_id).contains(&event.id)
                })
        });
        Ok((events, next))
    }

    fn owner<'a>(&'a self, event: &'a StoredEvent) -> Option<&'a str> {
        event_owner_with(event, |turn| {
            self.owners.get(&turn).map(|owner| owner.last.as_str())
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::event::{EventPayload, EventStore};
    use serde_json::json;

    use super::SessionHistory;

    #[test]
    fn history_pages_stay_at_the_projected_cursor() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.jsonl");
        let mut store = EventStore::open(&path).unwrap();
        store
            .append(EventPayload::Unknown(json!({"kind":"test.first"})))
            .unwrap();
        let history = SessionHistory::read(&path).unwrap();
        store
            .append(EventPayload::Unknown(json!({"kind":"test.second"})))
            .unwrap();
        let page = history.page_before(None, 10).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, history.cursor());
        assert!(history
            .page_before(Some(history.cursor()), 10)
            .unwrap()
            .is_empty());
    }
}
