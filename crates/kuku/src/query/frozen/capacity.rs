use super::{model_history, ContextCommit, MessageOrigin};
use crate::context::MessageBlock;
use crate::query::request::OwnedRequestBase;
use crate::query::runtime_tools::output::archived_page_reference;
use crate::Result;

const MIN_PAGE_TEXT_BYTES: usize = 1024;

impl ContextCommit {
    pub(in crate::query) fn reference_output_pages(
        &mut self,
        request: &mut OwnedRequestBase,
        turn: u64,
        reclaim_bytes: usize,
    ) -> Result<bool> {
        let Some(latest_results) = self
            .messages
            .iter()
            .rposition(|message| message.origin == MessageOrigin::ToolResults)
        else {
            return Ok(false);
        };
        let mut read_calls = Vec::new();
        let mut reclaimed = 0usize;
        let candidates = latest_results.min(self.prefix_context_head);
        for admitted in self.messages[..candidates]
            .iter_mut()
            .filter(|message| message.turn == turn)
        {
            if admitted.origin == MessageOrigin::Response {
                read_calls = admitted
                    .message
                    .blocks
                    .iter()
                    .filter_map(|block| match block {
                        MessageBlock::ToolUse(call) if call.name == "read_output" => {
                            Some(call.id.clone())
                        }
                        _ => None,
                    })
                    .collect();
            }
            if admitted.origin != MessageOrigin::ToolResults || read_calls.is_empty() {
                continue;
            }
            for block in &mut admitted.message.blocks {
                let MessageBlock::ToolResult(result) = block else {
                    continue;
                };
                if result.status != "ok"
                    || result.truncated
                    || result.model_content.len() < MIN_PAGE_TEXT_BYTES
                    || !read_calls.contains(&result.tool_call_id)
                {
                    continue;
                }
                let Some(reference) = archived_page_reference(&result.model_content) else {
                    continue;
                };
                let saved = serde_json::to_vec(&result.model_content)?
                    .len()
                    .saturating_sub(serde_json::to_vec(&reference)?.len());
                if saved == 0 {
                    continue;
                }
                result.model_content = reference;
                self.output_pages_referenced += 1;
                reclaimed = reclaimed.saturating_add(saved);
                if reclaimed >= reclaim_bytes {
                    break;
                }
            }
            read_calls.clear();
            if reclaimed >= reclaim_bytes {
                break;
            }
        }
        if reclaimed == 0 {
            return Ok(false);
        }
        self.new_segment = true;
        self.prefix_context_head = 0;
        self.reason = "request_byte_budget";
        request.replace_history(model_history(&self.messages, turn));
        Ok(true)
    }
}
