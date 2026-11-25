use std::collections::{HashMap, HashSet};

use super::{ContextCommit, MessageOrigin, OwnedRequestBase, model_history};
use crate::Result;
use crate::context::runtime::AdmittedMessage;
use crate::context::{CanonicalMessage, MessageBlock};
use crate::event::{EventPayload, EventStore};
use crate::query::request::estimated_message_tokens;
use crate::query::types::{FailedCompaction, PendingRun};

impl ContextCommit {
    pub(in crate::query) fn failed_compaction(&self, material_tokens: u32) -> FailedCompaction {
        FailedCompaction {
            segment: self.segment.id,
            signature: self.segment.signature.clone(),
            material_tokens,
        }
    }

    pub(in crate::query) fn matches_failed_compaction(&self, failure: &FailedCompaction) -> bool {
        failure.segment == self.segment.id && failure.signature == self.segment.signature
    }

    pub(in crate::query) fn checkpoint_source_head(&self) -> u64 {
        self.source_head
    }

    pub(in crate::query) fn checkpointed_messages(&self) -> usize {
        self.checkpointed_messages
    }

    pub(in crate::query) fn record_checkpoint(
        &mut self,
        pending: &mut PendingRun,
        completion: serde_json::Value,
    ) -> Result<bool> {
        let Some(segment) = super::commit_context(pending, self.clone(), completion, true)? else {
            return Ok(false);
        };
        self.segment.id = segment;
        self.checkpoint_file_delta = Default::default();
        self.new_segment = false;
        self.prefix_context_head = self.messages.len();
        Ok(true)
    }

    pub(in crate::query) fn checkpoint_cut(
        &self,
        turn: u64,
        recent_tokens: u32,
        compact_only: bool,
    ) -> Option<usize> {
        let newest_work = if compact_only {
            self.messages.len()
        } else {
            self.messages
                .iter()
                .rposition(|entry| entry.turn == turn && entry.origin == MessageOrigin::Response)
                .unwrap_or(self.current_input)
        };
        let mut boundaries = Vec::new();
        let mut open_calls = HashSet::new();
        let mut settled_response = false;
        let mut preceding_turn = None;
        for (index, entry) in self.messages.iter().enumerate() {
            let starts_turn = entry.origin == MessageOrigin::Input
                && preceding_turn.is_some_and(|prior| prior != entry.turn);
            let within_current_turn =
                !compact_only && entry.turn == turn && entry.origin == MessageOrigin::Response;
            if index <= newest_work
                && open_calls.is_empty()
                && settled_response
                && (starts_turn || within_current_turn)
            {
                boundaries.push(index);
            }
            for block in &entry.message.blocks {
                match block {
                    MessageBlock::ToolUse(call) => {
                        if !open_calls.insert(call.id.as_str()) {
                            return None;
                        }
                    }
                    MessageBlock::ToolResult(result)
                        if !open_calls.remove(result.tool_call_id.as_str()) =>
                    {
                        return None;
                    }
                    _ => {}
                }
            }
            settled_response |= entry.origin == MessageOrigin::Response;
            if matches!(
                entry.origin,
                MessageOrigin::Input | MessageOrigin::Response | MessageOrigin::ToolResults
            ) {
                preceding_turn = Some(entry.turn);
            }
        }
        if !open_calls.is_empty() {
            return None;
        }
        if boundaries.is_empty() {
            return (compact_only && settled_response).then_some(self.messages.len());
        }
        let mut tail_tokens = 0u32;
        let mut cut = *boundaries.last().expect("at least one balanced cut");
        for (index, entry) in self.messages.iter().enumerate().rev() {
            tail_tokens = tail_tokens
                .saturating_add(estimated_message_tokens(std::iter::once(&entry.message)));
            if boundaries.last() == Some(&index) {
                cut = index;
                boundaries.pop();
            }
            if tail_tokens >= recent_tokens || boundaries.is_empty() {
                break;
            }
        }
        Some(cut)
    }

    pub(in crate::query) fn checkpoint_source(
        &self,
        cut: usize,
        turn: u64,
    ) -> Vec<CanonicalMessage> {
        model_history(&self.messages[..cut], turn)
    }

    pub(in crate::query) fn new_checkpoint_material_tokens(&self, cut: usize, turn: u64) -> u32 {
        estimated_message_tokens(
            self.messages[..cut]
                .iter()
                .enumerate()
                .filter(|(index, entry)| {
                    entry.origin != MessageOrigin::Checkpoint
                        && !self.preserved_by_checkpoint(*index, entry, turn, false)
                })
                .map(|(_, entry)| &entry.message),
        )
    }

    fn preserved_by_checkpoint(
        &self,
        index: usize,
        entry: &AdmittedMessage,
        turn: u64,
        closed_turn: bool,
    ) -> bool {
        Some(index) == self.runtime_date_index
            || Some(index) == self.permission_mode_index
            || (!closed_turn
                && (index == self.current_input
                    || (entry.turn == turn && entry.origin == MessageOrigin::Input)))
    }

    pub(in crate::query) fn record_checkpoint_files(
        &mut self,
        cut: usize,
        pending: &PendingRun,
        closed_turn: bool,
    ) -> Result<()> {
        let mut response = 0;
        let mut first = u64::MAX;
        let mut last = 0;
        let mut sources = HashMap::<_, Vec<_>>::new();
        for (index, entry) in self.messages[..cut].iter().enumerate() {
            if entry.origin == MessageOrigin::Response {
                response = entry.admitted_at;
            }
            if self.preserved_by_checkpoint(index, entry, pending.turn, closed_turn) {
                continue;
            }
            for block in &entry.message.blocks {
                if let MessageBlock::ToolResult(result) = block {
                    sources
                        .entry((entry.turn, result.tool_call_id.as_str()))
                        .or_default()
                        .push(response..=entry.admitted_at);
                    first = first.min(response);
                    last = last.max(entry.admitted_at);
                }
            }
        }
        if sources.is_empty() {
            return Ok(());
        }

        let agent = pending.conversation.as_str();
        let files = &mut self.checkpoint_file_delta;
        EventStore::visit_committed(&pending.events_path, first.saturating_sub(1), |event| {
            if event.id > last {
                return Ok(false);
            }
            if let EventPayload::ToolResult {
                turn,
                conversation,
                tool_call_id,
                status,
                structured: Some(facts),
                ..
            } = &event.payload
            {
                if conversation.as_deref().unwrap_or("main") == agent
                    && sources
                        .get(&(*turn, tool_call_id.as_str()))
                        .is_some_and(|ranges| ranges.iter().any(|range| range.contains(&event.id)))
                {
                    files.record_result(status, facts);
                }
            }
            Ok(true)
        })?;
        self.segment.checkpoint_files_omitted |=
            self.segment.checkpoint_files.update_preview(files)?;
        Ok(())
    }

    pub(in crate::query) fn summary_with_files(&self, summary: &str) -> String {
        self.segment
            .checkpoint_files
            .append_to(summary, self.segment.checkpoint_files_omitted)
    }

    pub(in crate::query) fn apply_checkpoint(
        &mut self,
        request: &mut OwnedRequestBase,
        cut: usize,
        turn: u64,
        checkpoint: String,
        compact_only: bool,
    ) {
        let previous_input = self.current_input;
        let mut history = vec![AdmittedMessage {
            turn,
            origin: MessageOrigin::Checkpoint,
            message: CanonicalMessage::user_text(checkpoint),
            admitted_at: self.source_head,
        }];
        let mut current_input = 0;
        for (index, entry) in std::mem::take(&mut self.messages).into_iter().enumerate() {
            if index < cut && !self.preserved_by_checkpoint(index, &entry, turn, compact_only) {
                self.checkpointed_messages += 1;
                continue;
            }
            if index == previous_input {
                current_input = history.len();
            }
            history.push(entry);
        }
        self.messages = history;
        self.current_input = current_input;
        self.new_segment = true;
        self.prefix_context_head = 0;
        self.reason = "compaction";
        request.replace_checkpoint_history(model_history(&self.messages, turn), self.current_input);
    }

    pub(in crate::query) fn restore_selected_skills(
        &mut self,
        pending: &PendingRun,
        request: &mut OwnedRequestBase,
    ) -> crate::Result<()> {
        let restored =
            super::skill_catalog::append_notice(pending, &self.segment, &mut self.messages, 0)?;
        if restored.is_some_and(|index| index <= self.current_input) {
            self.current_input += 1;
        }
        if pending.query.input_skills.is_empty() && restored.is_none() {
            return Ok(());
        }
        self.input_skills
            .extend(super::admission::append_input_skills(
                pending,
                &mut self.messages,
                &self.segment.prelude,
            )?);
        self.input_skills.sort();
        self.input_skills.dedup();
        request.replace_checkpoint_history(
            model_history(&self.messages, pending.turn),
            self.current_input,
        );
        Ok(())
    }

    pub(in crate::query) fn fill_checkpoint(
        &mut self,
        request: &mut OwnedRequestBase,
        turn: u64,
        checkpoint: String,
    ) {
        self.messages[0].message = CanonicalMessage::user_text(checkpoint);
        request.replace_checkpoint_history(model_history(&self.messages, turn), self.current_input);
    }
}
