use super::EventPayload;

pub(crate) fn event_turn(payload: &EventPayload) -> Option<u64> {
    match payload {
        EventPayload::ContextSources { turn, .. }
        | EventPayload::ContextSkills { turn, .. }
        | EventPayload::ModelResponse { turn, .. }
        | EventPayload::ModelError { turn, .. }
        | EventPayload::ModelRecovery { turn, .. }
        | EventPayload::ToolCall { turn, .. }
        | EventPayload::PermissionRequested { turn, .. }
        | EventPayload::PermissionAllow { turn, .. }
        | EventPayload::PermissionDeny { turn, .. }
        | EventPayload::ToolResult { turn, .. }
        | EventPayload::Handoff { turn, .. }
        | EventPayload::PromptSnapshot { turn, .. }
        | EventPayload::MessageUser { turn, .. }
        | EventPayload::MessageAssistant { turn, .. }
        | EventPayload::TurnStarted { turn, .. }
        | EventPayload::TurnCompleted { turn, .. }
        | EventPayload::TurnCancelled { turn, .. }
        | EventPayload::TurnInterrupted { turn, .. } => Some(*turn),
        EventPayload::SessionCreated { .. }
        | EventPayload::ConversationOpened { .. }
        | EventPayload::ConversationBound { .. }
        | EventPayload::ConversationRollback { .. }
        | EventPayload::ConversationRollbackUndone { .. }
        | EventPayload::Unknown(_) => None,
    }
}

pub(crate) fn event_conversation(payload: &EventPayload) -> Option<&str> {
    match payload {
        EventPayload::ToolCall { conversation, .. }
        | EventPayload::ToolResult { conversation, .. } => conversation.as_deref(),
        EventPayload::ConversationOpened { conversation, .. }
        | EventPayload::ConversationBound { conversation, .. }
        | EventPayload::PromptSnapshot { conversation, .. }
        | EventPayload::MessageUser { conversation, .. }
        | EventPayload::MessageAssistant { conversation, .. }
        | EventPayload::TurnStarted { conversation, .. }
        | EventPayload::TurnCompleted { conversation, .. }
        | EventPayload::TurnCancelled { conversation, .. }
        | EventPayload::TurnInterrupted { conversation, .. }
        | EventPayload::ConversationRollback { conversation, .. }
        | EventPayload::ConversationRollbackUndone { conversation, .. }
        | EventPayload::ContextSkills { conversation, .. } => Some(conversation.as_str()),
        EventPayload::ModelRecovery { conversation, .. } => Some(conversation.as_str()),
        _ => None,
    }
}
