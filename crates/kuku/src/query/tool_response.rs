use std::collections::HashSet;

use crate::provider::types::ProviderToolCall;

pub(super) fn validate_tool_calls(
    mut tool_calls: Vec<ProviderToolCall>,
    argument_buffers: Vec<(u64, String)>,
    completions: Vec<u64>,
    stream_invalid: bool,
) -> std::result::Result<Vec<ProviderToolCall>, ToolResponseError> {
    if stream_invalid {
        return Err(ToolResponseError::InvalidLifecycle);
    }
    if tool_calls.is_empty() {
        return Err(ToolResponseError::MissingCalls);
    }

    let mut ids = HashSet::new();
    let mut indices = HashSet::new();
    for tool_call in &tool_calls {
        if tool_call.id.trim().is_empty() {
            return Err(ToolResponseError::EmptyId);
        }
        if tool_call.name.trim().is_empty() {
            return Err(ToolResponseError::EmptyName);
        }
        if !ids.insert(&tool_call.id) {
            return Err(ToolResponseError::DuplicateId);
        }
        if !indices.insert(tool_call.index) {
            return Err(ToolResponseError::DuplicateIndex);
        }
    }

    if argument_buffers.len() != tool_calls.len() {
        return Err(ToolResponseError::InvalidBufferMapping);
    }
    if completions.len() != tool_calls.len() {
        return Err(ToolResponseError::InvalidCompletionMapping);
    }

    let mut completed_indices = HashSet::new();
    for index in completions {
        if !indices.contains(&index) || !completed_indices.insert(index) {
            return Err(ToolResponseError::InvalidCompletionMapping);
        }
    }

    let mut buffered_indices = HashSet::new();
    for (index, buffer) in argument_buffers {
        if !buffered_indices.insert(index) {
            return Err(ToolResponseError::InvalidBufferMapping);
        }
        let Some(tool_call) = tool_calls
            .iter_mut()
            .find(|tool_call| tool_call.index == index)
        else {
            return Err(ToolResponseError::InvalidBufferMapping);
        };
        let args = if buffer.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&buffer).map_err(|_| ToolResponseError::InvalidArguments)?
        };
        if !args.is_object() {
            return Err(ToolResponseError::InvalidArguments);
        }
        tool_call.args = args;
    }
    if buffered_indices != indices {
        return Err(ToolResponseError::InvalidBufferMapping);
    }

    Ok(tool_calls)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolResponseError {
    InvalidLifecycle,
    MissingCalls,
    EmptyId,
    EmptyName,
    DuplicateId,
    DuplicateIndex,
    InvalidBufferMapping,
    InvalidCompletionMapping,
    InvalidArguments,
}
