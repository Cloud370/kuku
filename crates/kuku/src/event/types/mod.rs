//! Event payload and persistence types.

mod codec;
mod payload;
mod stored;

pub use payload::{ContextMessage, EventPayload, RollbackScope};
pub use stored::StoredEvent;

#[cfg(test)]
mod tests;
