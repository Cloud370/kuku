use std::collections::{HashMap, HashSet};

use crate::context::revert::ActiveRollback;
use crate::event::{EventPayload, StoredEvent};

use super::address::ConversationAddress;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnState {
    pub turn: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnTerminal {
    Completed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationState {
    pub address: ConversationAddress,
    pub active_binding: Option<String>,
    pub active_rollback: Option<ActiveRollback>,
    pub last_terminal: Option<(u64, TurnTerminal)>,
    pub active_turn: Option<TurnState>,
}

struct ConversationAccumulator {
    state: ConversationState,
    opened_event_id: u64,
}

pub fn reduce_conversations(events: &[StoredEvent]) -> Vec<ConversationState> {
    let mut conversations = HashMap::<String, ConversationAccumulator>::new();

    for event in events {
        match &event.payload {
            EventPayload::ConversationOpened { conversation, .. } => {
                let Ok(address) = ConversationAddress::parse(conversation) else {
                    continue;
                };
                conversations
                    .entry(conversation.clone())
                    .or_insert_with(|| ConversationAccumulator {
                        state: ConversationState {
                            address,
                            active_binding: None,
                            active_rollback: None,
                            last_terminal: None,
                            active_turn: None,
                        },
                        opened_event_id: event.id,
                    });
            }
            EventPayload::ConversationBound {
                conversation,
                binding_id,
                ..
            } => {
                if let Some(state) = conversations.get_mut(conversation) {
                    state.state.active_binding = Some(binding_id.clone());
                }
            }
            EventPayload::TurnStarted {
                conversation, turn, ..
            } => {
                if let Some(state) = conversations.get_mut(conversation) {
                    state.state.active_turn = Some(TurnState { turn: *turn });
                }
            }
            EventPayload::TurnCompleted {
                conversation, turn, ..
            } => {
                if let Some(state) = conversations.get_mut(conversation) {
                    state.state.last_terminal = Some((*turn, TurnTerminal::Completed));
                    if state
                        .state
                        .active_turn
                        .as_ref()
                        .is_some_and(|active| active.turn == *turn)
                    {
                        state.state.active_turn = None;
                    }
                }
            }
            EventPayload::TurnCancelled {
                conversation, turn, ..
            } => {
                if let Some(state) = conversations.get_mut(conversation) {
                    state.state.last_terminal = Some((*turn, TurnTerminal::Cancelled));
                    if state
                        .state
                        .active_turn
                        .as_ref()
                        .is_some_and(|active| active.turn == *turn)
                    {
                        state.state.active_turn = None;
                    }
                }
            }
            EventPayload::TurnInterrupted {
                conversation, turn, ..
            } => {
                if let Some(state) = conversations.get_mut(conversation) {
                    state.state.last_terminal = Some((*turn, TurnTerminal::Interrupted));
                    if state
                        .state
                        .active_turn
                        .as_ref()
                        .is_some_and(|active| active.turn == *turn)
                    {
                        state.state.active_turn = None;
                    }
                }
            }
            EventPayload::ConversationRollback {
                conversation,
                to_turn,
                to_event_id,
                scope,
                ..
            } => {
                if let Some(state) = conversations.get_mut(conversation) {
                    state.state.active_rollback = Some(ActiveRollback {
                        conversation: conversation.clone(),
                        rollback_event_id: event.id,
                        to_turn: *to_turn,
                        to_event_id: *to_event_id,
                        scope: scope.clone(),
                    });
                }
            }
            EventPayload::ConversationRollbackUndone {
                conversation,
                rollback_event_id,
                ..
            } => {
                if let Some(state) = conversations.get_mut(conversation) {
                    if state
                        .state
                        .active_rollback
                        .as_ref()
                        .is_some_and(|active| active.rollback_event_id == *rollback_event_id)
                    {
                        state.state.active_rollback =
                            active_conversation_rollback(events, conversation);
                    }
                }
            }
            EventPayload::SessionMeta { .. }
            | EventPayload::SessionCreated { .. }
            | EventPayload::ContextPrelude { .. }
            | EventPayload::ContextSources { .. }
            | EventPayload::ContextSkills { .. }
            | EventPayload::TurnStart { .. }
            | EventPayload::UserInput { .. }
            | EventPayload::ModelResponse { .. }
            | EventPayload::ModelError { .. }
            | EventPayload::ToolCall { .. }
            | EventPayload::PermissionAllow { .. }
            | EventPayload::PermissionRequested { .. }
            | EventPayload::PermissionDeny { .. }
            | EventPayload::ToolResult { .. }
            | EventPayload::Handoff { .. }
            | EventPayload::TurnEnd { .. }
            | EventPayload::TurnRollback { .. }
            | EventPayload::TurnRollbackUndo { .. }
            | EventPayload::PromptSnapshot { .. }
            | EventPayload::MessageUser { .. }
            | EventPayload::MessageAssistant { .. }
            | EventPayload::Unknown(_) => {}
        }
    }

    let mut conversations = conversations.into_values().collect::<Vec<_>>();
    conversations.sort_by_key(|state| state.opened_event_id);
    conversations.into_iter().map(|state| state.state).collect()
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

pub fn completed_turns(events: &[StoredEvent], address: &ConversationAddress) -> Vec<u64> {
    conversation_events(events, address)
        .into_iter()
        .filter_map(|event| match &event.payload {
            EventPayload::TurnCompleted { turn, .. } => Some(*turn),
            _ => None,
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

fn active_conversation_rollback(
    events: &[StoredEvent],
    conversation: &str,
) -> Option<ActiveRollback> {
    let undone_ids = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ConversationRollbackUndone {
                conversation: event_conversation,
                rollback_event_id,
                ..
            } if event_conversation == conversation => Some(*rollback_event_id),
            _ => None,
        })
        .collect::<HashSet<_>>();

    events.iter().rev().find_map(|event| match &event.payload {
        EventPayload::ConversationRollback {
            conversation: event_conversation,
            to_turn,
            to_event_id,
            scope,
            ..
        } if event_conversation == conversation && !undone_ids.contains(&event.id) => {
            Some(ActiveRollback {
                conversation: event_conversation.clone(),
                rollback_event_id: event.id,
                to_turn: *to_turn,
                to_event_id: *to_event_id,
                scope: scope.clone(),
            })
        }
        _ => None,
    })
}

fn event_conversation(event: &StoredEvent) -> Option<&str> {
    match &event.payload {
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
        | EventPayload::ConversationRollbackUndone { conversation, .. } => {
            Some(conversation.as_str())
        }
        EventPayload::SessionMeta { .. }
        | EventPayload::SessionCreated { .. }
        | EventPayload::ContextPrelude { .. }
        | EventPayload::ContextSources { .. }
        | EventPayload::ContextSkills { .. }
        | EventPayload::TurnStart { .. }
        | EventPayload::UserInput { .. }
        | EventPayload::ModelResponse { .. }
        | EventPayload::ModelError { .. }
        | EventPayload::PermissionAllow { .. }
        | EventPayload::PermissionRequested { .. }
        | EventPayload::PermissionDeny { .. }
        | EventPayload::Handoff { .. }
        | EventPayload::TurnEnd { .. }
        | EventPayload::TurnRollback { .. }
        | EventPayload::TurnRollbackUndo { .. }
        | EventPayload::Unknown(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        active_turn, completed_turns, conversation_events, reduce_conversations, TurnState,
        TurnTerminal,
    };
    use crate::conversation::address::ConversationAddress;
    use crate::event::{EventPayload, RollbackScope, StoredEvent};

    fn event(id: u64, payload: EventPayload) -> StoredEvent {
        StoredEvent { id, payload }
    }

    fn opened(id: u64, conversation: &str) -> StoredEvent {
        event(
            id,
            EventPayload::ConversationOpened {
                ts: "t".into(),
                conversation: conversation.into(),
            },
        )
    }

    fn bound(id: u64, conversation: &str, binding_id: &str) -> StoredEvent {
        event(
            id,
            EventPayload::ConversationBound {
                ts: "t".into(),
                conversation: conversation.into(),
                binding_id: binding_id.into(),
            },
        )
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

    fn cancelled(id: u64, conversation: &str, turn: u64) -> StoredEvent {
        event(
            id,
            EventPayload::TurnCancelled {
                ts: "t".into(),
                conversation: conversation.into(),
                turn,
                reason: "cancelled".into(),
            },
        )
    }

    fn interrupted(id: u64, conversation: &str, turn: u64) -> StoredEvent {
        event(
            id,
            EventPayload::TurnInterrupted {
                ts: "t".into(),
                conversation: conversation.into(),
                turn,
                reason: "interrupted".into(),
            },
        )
    }

    fn rollback(
        id: u64,
        conversation: &str,
        target_turn: u64,
        scope: RollbackScope,
    ) -> StoredEvent {
        event(
            id,
            EventPayload::ConversationRollback {
                ts: "t".into(),
                conversation: conversation.into(),
                to_turn: target_turn,
                to_event_id: id.saturating_sub(1),
                scope,
            },
        )
    }

    #[test]
    fn reducer_keeps_open_order_with_overlapping_local_turn_numbers() {
        let states = reduce_conversations(&[
            opened(1, "review"),
            opened(2, "explore"),
            bound(3, "review", "child-review"),
            bound(4, "explore", "child-explore"),
            started(5, "explore", 1),
            completed(6, "explore", 1),
            started(7, "review", 1),
            completed(8, "review", 1),
        ]);

        assert_eq!(states.len(), 2);
        assert_eq!(states[0].address.as_str(), "review");
        assert_eq!(states[1].address.as_str(), "explore");
        assert_eq!(states[0].active_binding.as_deref(), Some("child-review"));
        assert_eq!(states[1].active_binding.as_deref(), Some("child-explore"));
        assert_eq!(states[0].last_terminal, Some((1, TurnTerminal::Completed)));
        assert_eq!(states[1].last_terminal, Some((1, TurnTerminal::Completed)));
    }

    #[test]
    fn cancelled_and_interrupted_turns_stay_in_ledger_but_not_completed_history() {
        let review = ConversationAddress::parse("review").unwrap();
        let explore = ConversationAddress::parse("explore").unwrap();
        let events = vec![
            opened(1, "review"),
            opened(2, "explore"),
            started(3, "review", 1),
            cancelled(4, "review", 1),
            started(5, "explore", 1),
            interrupted(6, "explore", 1),
            started(7, "review", 2),
            completed(8, "review", 2),
        ];

        let states = reduce_conversations(&events);

        assert_eq!(states[0].last_terminal, Some((2, TurnTerminal::Completed)));
        assert_eq!(
            states[1].last_terminal,
            Some((1, TurnTerminal::Interrupted))
        );
        assert_eq!(completed_turns(&events, &review), vec![2]);
        assert!(completed_turns(&events, &explore).is_empty());
    }

    #[test]
    fn active_turn_is_scoped_per_conversation() {
        let review = ConversationAddress::parse("review").unwrap();
        let explore = ConversationAddress::parse("explore").unwrap();
        let events = vec![
            opened(1, "review"),
            opened(2, "explore"),
            started(3, "review", 1),
            started(4, "explore", 1),
            completed(5, "explore", 1),
            started(6, "explore", 2),
        ];

        assert_eq!(active_turn(&events, &review), Some(TurnState { turn: 1 }));
        assert_eq!(active_turn(&events, &explore), Some(TurnState { turn: 2 }));
    }

    #[test]
    fn conversation_events_keep_session_facts_and_target_conversation_facts() {
        let review = ConversationAddress::parse("review").unwrap();
        let events = vec![
            event(
                1,
                EventPayload::SessionCreated {
                    ts: "t".into(),
                    schema_version: 1,
                    session_id: "s".into(),
                    created_at: "t".into(),
                    kuku_version: "0".into(),
                },
            ),
            opened(2, "review"),
            opened(3, "explore"),
            started(4, "review", 1),
            started(5, "explore", 1),
            rollback(6, "review", 1, RollbackScope::ConversationOnly),
        ];

        let ids = conversation_events(&events, &review)
            .into_iter()
            .map(|event| event.id)
            .collect::<Vec<_>>();

        assert_eq!(ids, vec![1, 2, 4, 6]);
    }
}
