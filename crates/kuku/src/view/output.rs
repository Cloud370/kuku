use crate::event::{EventPayload, StoredEvent};

pub fn derive_final_output(events: &[StoredEvent]) -> Option<String> {
    events.iter().rev().find_map(|event| match &event.payload {
        EventPayload::ModelResponse {
            stop_reason, text, ..
        } if stop_reason == "end_turn" => Some(text.clone()),
        _ => None,
    })
}
