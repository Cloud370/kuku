use super::resumed_current_input;
use crate::context::CanonicalMessage;
use crate::context::runtime::{AdmittedMessage, MessageOrigin};

#[test]
fn compacted_resume_recreates_the_source_input_and_reuses_it_on_retry() {
    let mut messages = vec![
        AdmittedMessage {
            turn: 7,
            origin: MessageOrigin::Input,
            message: CanonicalMessage::user_text("older unrelated input"),
            admitted_at: 7,
        },
        AdmittedMessage {
            turn: 8,
            origin: MessageOrigin::Runtime,
            message: CanonicalMessage::user_text("compacted context summary"),
            admitted_at: 8,
        },
    ];

    let first = resumed_current_input(&mut messages, 10, Some(9), "accepted work", 9).unwrap();
    assert_eq!(9, messages[first].turn);
    assert_eq!(
        CanonicalMessage::user_text("accepted work"),
        messages[first].message
    );
    assert_eq!(9, messages[first].admitted_at);
    let message_count = messages.len();

    let retry = resumed_current_input(&mut messages, 11, Some(9), "accepted work", 10).unwrap();
    assert_eq!(first, retry);
    assert_eq!(message_count, messages.len());
}
