use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::context::revert::{
    apply_verified_file_rewind, event_owner_with, plan_file_rewind, RewindVisibility,
};
use crate::event::store::{HistoryMarks, MessagePreviews, TurnOwnerIndex};
use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::{Error, Result};

use super::controls::{command, commit_with_file_revert, rewind_blocked};
use super::record::Record;
use super::{Receipt, Session, TurnIndex};

pub use crate::context::revert::{FileRewindChange, FileRewindPreview};
pub use crate::event::store::{MessageRewind, UserMessagePreview};

const USER_MESSAGE_SCAN_BYTES: usize = 16 * 1024 * 1024;
const USER_MESSAGE_PAGE_LIMIT: usize = 256;

impl Session {
    /// Continue one Agent from a settled Turn on any retained branch of this Session.
    pub fn jump_history(
        &self,
        command_id: &str,
        address: &str,
        target_event_id: u64,
    ) -> Result<Receipt> {
        self.jump_history_with_file_restore(command_id, address, target_event_id, None)
    }

    /// Read the effects of restoring recorded files after a settled point on the current branch.
    pub fn preview_file_rewind(
        events_path: &Path,
        workspace: &Path,
        address: &str,
        target_event_id: u64,
    ) -> Result<FileRewindPreview> {
        let events = EventStore::read_page_before(events_path, target_event_id.checked_add(1), 1)?;
        if !events.first().is_some_and(|event| {
            event.id == target_event_id
                && matches!(
                    &event.payload, EventPayload::Unknown(value)
                    if value["kind"] == "runtime.operation"
                        && value["record"]["operation"] == "turn_settled"
                        && value["record"]["status"] == "idle"
                        && value["record"]["address"] == address
                        && value["record"]["turn"].is_u64()
                )
        }) {
            return Err(Error::InvalidArgument(
                "Select a finished Turn belonging to this Agent".into(),
            ));
        }
        Ok(plan_file_rewind(events_path, workspace, address, target_event_id)?.preview)
    }

    /// Continue from a settled Turn, optionally restoring the exact file plan previously previewed.
    pub fn jump_history_with_file_restore(
        &self,
        command_id: &str,
        address: &str,
        target_event_id: u64,
        file_restore: Option<&str>,
    ) -> Result<Receipt> {
        let command = command(
            command_id,
            address,
            &("history_jump", address, target_event_id, file_restore),
        )?;
        let mut state = self.inner.state.lock().unwrap();
        super::ensure_open(&state)?;
        if let Some(receipt) = state.retry(command_id, &command.fingerprint)? {
            return Ok(receipt);
        }
        if rewind_blocked(state.agent(address)?) {
            return Err(Error::InvalidArgument(
                "cannot switch history while this Agent is executing or has pending input".into(),
            ));
        }
        let target_turn = state
            .turns
            .can_continue_after(address, target_event_id)
            .ok_or_else(|| {
                Error::InvalidArgument("select a finished Turn belonging to this Agent".into())
            })?;
        let transaction = file_restore
            .map(|approved| {
                if state.jobs.has_active() || state.agents.values().any(rewind_blocked) {
                    return Err(Error::InvalidArgument(
                        "Wait for Session Agents and jobs to settle before restoring files".into(),
                    ));
                }
                let workspace = &self.inner.options.workspace;
                let path = self.events_path();
                let files = plan_file_rewind(path, workspace, address, target_event_id)?;
                if files.preview.fingerprint != approved || files.preview.changes.is_empty() {
                    return Err(Error::InvalidArgument(
                        "The file plan changed. Open the history and preview it again.".into(),
                    ));
                }
                let store = EventStore::open(path)?;
                let directory = path.parent().ok_or_else(|| {
                    Error::InvalidArgument("events path has no session directory".into())
                })?;
                Ok(apply_verified_file_rewind(
                    &files.plan,
                    workspace,
                    directory,
                    store.next_id(),
                    &files.expected,
                )?)
            })
            .transpose()?;
        commit_with_file_revert(transaction, || {
            self.commit(
                &mut state,
                Record::HistoryJump {
                    command,
                    address: address.into(),
                    target_event_id,
                    target_turn,
                    restore_files: file_restore.is_some(),
                },
            )
        })?;
        Ok(state.commands[command_id].1.clone())
    }

    /// Capture the live history projection and its Turn index without replaying the journal.
    pub fn history_snapshot(&self) -> Result<(SessionHistory, TurnIndex)> {
        let state = self.inner.state.lock().unwrap();
        super::ensure_open(&state)?;
        Ok((
            SessionHistory::read(self.events_path())?,
            state.turns.clone(),
        ))
    }
}

/// Agent ownership and rewind visibility for a committed Session history.
pub struct SessionHistory {
    path: PathBuf,
    cursor: u64,
    rewinds: Vec<MessageRewind>,
    owners: TurnOwnerIndex,
    visibility: RewindVisibility,
    editable: HashMap<String, MessagePreviews>,
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
            editable: marks.editable,
        }
    }

    /// Determine whether a committed event remains in the active history.
    pub fn includes(&self, event: &StoredEvent) -> bool {
        self.visibility.includes(event, Some(self.agent(event)))
    }

    /// Check whether a settled Turn cursor belongs to the active branch.
    pub fn includes_cursor(&self, agent: &str, id: u64) -> bool {
        id <= self.cursor && self.visibility.hidden_start(agent, id).is_none()
    }

    /// Return the active user messages that can be edited in the selected Turn.
    pub fn editable_messages(&self, agent: &str, turn: u64) -> Vec<UserMessagePreview> {
        self.editable
            .get(agent)
            .and_then(|turns| turns.get(&turn))
            .iter()
            .flat_map(|messages| messages.iter())
            .filter(|message| self.includes_cursor(agent, message.event_id))
            .cloned()
            .collect()
    }

    /// Original committed prompt at the selected Turn's historical boundary.
    pub fn turn_prompt(&self, agent: &str, turn: u64, through: u64) -> Option<&UserMessagePreview> {
        self.editable
            .get(agent)?
            .get(&turn)?
            .iter()
            .find(|message| message.event_id <= through)
    }

    /// Resolve the Agent shown in a transcript, including older events scoped through their Turn.
    pub fn agent<'a>(&'a self, event: &'a StoredEvent) -> &'a str {
        event_owner_with(event, |turn| {
            self.owners.get(&turn).map(|owner| owner.last.as_str())
        })
        .unwrap_or("main")
    }

    /// Move an exclusive transcript cursor across a rewound range to its nearest group anchor.
    pub fn transcript_before(&self, agent: &str, before: Option<u64>) -> Option<u64> {
        let before = self.bound_before(before)?;
        let start = self
            .visibility
            .hidden_start(agent, before.saturating_sub(1));
        let anchor = start.and_then(|start| {
            self.rewinds
                .iter()
                .filter(|rewind| {
                    rewind.agent == agent && (start..before).contains(&rewind.target_event_id)
                })
                .map(|rewind| rewind.target_event_id)
                .max()
        });
        Some(anchor.map_or(start.unwrap_or(before), |id| id + 1))
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
        EventStore::read_page_before(&self.path, self.bound_before(before), limit)
    }

    /// Read a bounded chronological page after an exclusive cursor in this history view.
    pub fn page_after(&self, after: u64, limit: usize) -> Result<Vec<StoredEvent>> {
        if after >= self.cursor {
            return Ok(Vec::new());
        }
        let mut events = EventStore::read_page(&self.path, after, limit)?;
        events.truncate(events.partition_point(|event| event.id <= self.cursor));
        Ok(events)
    }

    fn bound_before(&self, before: Option<u64>) -> Option<u64> {
        self.cursor
            .checked_add(1)
            .map_or(before, |end| Some(before.map_or(end, |id| id.min(end))))
    }

    /// Read active user messages for one Agent, with an exclusive cursor for the next scan.
    /// A page can be short when its journal scan reaches the byte budget.
    pub fn user_messages_before(
        &self,
        agent: &str,
        before: Option<u64>,
        limit: usize,
    ) -> Result<(Vec<StoredEvent>, Option<u64>)> {
        let mut messages = Vec::new();
        let mut next_before = None;
        let mut bytes_read = 0;
        if limit == 0 {
            return Ok((messages, next_before));
        }
        let limit = limit.min(USER_MESSAGE_PAGE_LIMIT);
        let mut before = self.bound_before(before);
        loop {
            let mut skip_to = None;
            EventStore::visit_committed_before_with_size(&self.path, before, |event, bytes| {
                let id = event.id;
                bytes_read += bytes;
                if let Some(start) = self.visibility.hidden_start(agent, id) {
                    skip_to = (start > 1).then_some(start);
                    return Ok(false);
                }
                if matches!(&event.payload, EventPayload::MessageUser { conversation, job_delivery: false, .. } if conversation == agent)
                    && self.includes(&event)
                {
                    messages.push(event);
                }
                let more = messages.len() < limit && bytes_read < USER_MESSAGE_SCAN_BYTES;
                if !more && id > 1 {
                    next_before = Some(id);
                }
                Ok(more)
            })?;
            let Some(start) = skip_to else {
                break;
            };
            if bytes_read >= USER_MESSAGE_SCAN_BYTES {
                next_before = Some(start);
                break;
            }
            before = Some(start);
        }
        messages.reverse();
        Ok((messages, next_before))
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
                && self.agent(event) == rewind.agent
                && !previous.iter().any(|earlier| {
                    earlier.agent == rewind.agent
                        && (earlier.target_event_id..earlier.event_id).contains(&event.id)
                })
        });
        Ok((events, next))
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
