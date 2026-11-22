use std::collections::HashSet;

use super::{model_history, ContextCommit, MessageOrigin, OwnedRequestBase};
use crate::context::runtime::AdmittedMessage;
use crate::context::{CanonicalMessage, MessageBlock};
use crate::query::request::estimated_message_tokens;
use crate::query::types::PendingRun;
use crate::Result;

impl ContextCommit {
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
    ) -> Result<()> {
        self.segment.id = super::commit_context(pending, self.clone(), completion)?;
        self.new_segment = false;
        self.prefix_context_head = self.messages.len();
        Ok(())
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
                        return None
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
            || (!closed_turn
                && (index == self.current_input
                    || (entry.turn == turn && entry.origin == MessageOrigin::Input)))
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
