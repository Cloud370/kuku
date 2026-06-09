//! Conversation domain types.

pub mod address;
pub mod binding;
pub mod reducer;

pub use reducer::{
    active_turn, completed_turns, conversation_events, reduce_conversations, ConversationState,
    TurnState, TurnTerminal,
};
