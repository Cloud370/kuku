//! Agent-addressed event routing.

pub mod address;
pub mod reducer;

pub use reducer::{TurnState, active_turn, conversation_events};
