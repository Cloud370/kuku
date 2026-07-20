use crate::api::{AgentThread, ApiVersion, TaskId};

use super::read_model::{AgentThreadFacts, ContextReadError};

pub(crate) const MAX_AGENT_THREAD_MESSAGES: usize = 500;
pub(crate) const MAX_AGENT_THREAD_BYTES: usize = 10 * 1024 * 1024;

pub(crate) fn reduce_agent_thread(
    task_id: &TaskId,
    facts: &AgentThreadFacts,
) -> Result<AgentThread, ContextReadError> {
    let base_bytes = serde_json::to_vec(&thread(task_id, facts, Vec::new(), false))
        .map_err(|_| ContextReadError::LedgerCorrupt)?
        .len();
    let mut bytes = 0_usize;
    let mut retained = Vec::new();
    for message in facts.messages.iter().rev() {
        let message_bytes = serde_json::to_vec(message)
            .map_err(|_| ContextReadError::LedgerCorrupt)?
            .len();
        let separators = retained.len();
        let total_bytes = base_bytes
            .checked_add(bytes)
            .and_then(|total| total.checked_add(message_bytes))
            .and_then(|total| total.checked_add(separators));
        if retained.len() == MAX_AGENT_THREAD_MESSAGES
            || total_bytes.is_none_or(|total| total > MAX_AGENT_THREAD_BYTES)
        {
            break;
        }
        bytes += message_bytes;
        retained.push(message.clone());
    }
    retained.reverse();
    let messages_truncated_before = retained.len() < facts.messages.len();
    Ok(thread(task_id, facts, retained, messages_truncated_before))
}

fn thread(
    task_id: &TaskId,
    facts: &AgentThreadFacts,
    messages: Vec<crate::api::MessageProjection>,
    messages_truncated_before: bool,
) -> AgentThread {
    AgentThread {
        api_version: ApiVersion,
        task_id: task_id.clone(),
        conversation_id: facts.conversation_id.clone(),
        agent: facts.agent.clone(),
        tier: facts.tier.clone(),
        status: facts.status,
        result_in_main: facts.result_in_main,
        messages,
        messages_truncated_before,
    }
}
