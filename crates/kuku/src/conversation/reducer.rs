use std::collections::HashSet;

use crate::event::{EventPayload, StoredEvent};

use super::address::ConversationAddress;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnState {
    pub turn: u64,
}

pub fn conversation_events<'a>(
    events: &'a [StoredEvent],
    address: &ConversationAddress,
) -> Vec<&'a StoredEvent> {
    events
        .iter()
        .filter(|event| {
            event_conversation(event).is_none_or(|conversation| conversation == address.as_str())
        })
        .collect()
}

pub fn active_turn(events: &[StoredEvent], address: &ConversationAddress) -> Option<TurnState> {
    let mut terminal_turns = HashSet::new();
    let mut started_turns = Vec::new();

    for event in conversation_events(events, address) {
        match &event.payload {
            EventPayload::TurnStarted { turn, .. } => started_turns.push(*turn),
            EventPayload::TurnCompleted { turn, .. }
            | EventPayload::TurnCancelled { turn, .. }
            | EventPayload::TurnInterrupted { turn, .. } => {
                terminal_turns.insert(*turn);
            }
            _ => {}
        }
    }

    started_turns
        .into_iter()
        .rev()
        .find(|turn| !terminal_turns.contains(turn))
        .map(|turn| TurnState { turn })
}

fn event_conversation(event: &StoredEvent) -> Option<&str> {
    match &event.payload {
        EventPayload::ToolCall { conversation, .. }
        | EventPayload::ToolResult { conversation, .. } => conversation.as_deref(),
        EventPayload::PromptSnapshot { conversation, .. }
        | EventPayload::MessageUser { conversation, .. }
        | EventPayload::MessageAssistant { conversation, .. }
        | EventPayload::TurnStarted { conversation, .. }
        | EventPayload::TurnCompleted { conversation, .. }
        | EventPayload::TurnCancelled { conversation, .. }
        | EventPayload::TurnInterrupted { conversation, .. }
        | EventPayload::ModelRecovery { conversation, .. } => Some(conversation.as_str()),
        EventPayload::SessionCreated { .. }
        | EventPayload::ContextSources { .. }
        | EventPayload::ContextSkills { .. }
        | EventPayload::ModelResponse { .. }
        | EventPayload::ModelError { .. }
        | EventPayload::PermissionAllow { .. }
        | EventPayload::PermissionRequested { .. }
        | EventPayload::PermissionDeny { .. }
        | EventPayload::Handoff { .. }
        | EventPayload::Unknown(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{active_turn, conversation_events, TurnState};
    use crate::conversation::address::ConversationAddress;
    use crate::event::{EventPayload, StoredEvent};

    fn event(id: u64, payload: EventPayload) -> StoredEvent {
        StoredEvent { id, payload }
    }

    fn started(id: u64, conversation: &str, turn: u64) -> StoredEvent {
        event(
            id,
            EventPayload::TurnStarted {
                ts: "t".into(),
                conversation: conversation.into(),
                turn,
            },
        )
    }

    fn completed(id: u64, conversation: &str, turn: u64) -> StoredEvent {
        event(
            id,
            EventPayload::TurnCompleted {
                ts: "t".into(),
                conversation: conversation.into(),
                turn,
            },
        )
    }

    fn recovery(id: u64, conversation: &str) -> StoredEvent {
        event(
            id,
            EventPayload::ModelRecovery {
                conversation: conversation.into(),
                turn: 1,
                ts: "t".into(),
                from_request_id: "req_1".into(),
                to_request_id: "req_2".into(),
                reason: crate::event::ModelStopReason::Length,
                attempt: 1,
                max_attempts: 1,
                failed_max_output_tokens: Some(32),
                retry_max_output_tokens: Some(32),
                output_tokens_total: None,
                discarded_tool_calls: 0,
                notice: "notice".into(),
                prompt_path: "runtime/recovery.md".into(),
                prompt_hash: "sha256:test".into(),
            },
        )
    }

    #[test]
    fn active_turn_is_scoped_per_conversation() {
        let review = ConversationAddress::parse("review").unwrap();
        let explore = ConversationAddress::parse("explore").unwrap();
        let events = vec![
            started(3, "review", 1),
            started(4, "explore", 1),
            completed(5, "explore", 1),
            started(6, "explore", 2),
        ];

        assert_eq!(active_turn(&events, &review), Some(TurnState { turn: 1 }));
        assert_eq!(active_turn(&events, &explore), Some(TurnState { turn: 2 }));
    }

    #[test]
    fn conversation_events_scope_model_recovery_to_its_conversation() {
        let review = ConversationAddress::parse("review").unwrap();
        let events = vec![recovery(1, "review"), recovery(2, "explore")];

        let ids = conversation_events(&events, &review)
            .into_iter()
            .map(|event| event.id)
            .collect::<Vec<_>>();

        assert_eq!(ids, vec![1]);
    }
}
