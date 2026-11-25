use super::{admission::input_admissions, resumed_current_input};
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

    assert!(input_admissions(&messages, 20, Some(9)).next().is_none());
    let first = resumed_current_input(&mut messages, 20, Some(9), "accepted work", 12).unwrap();
    assert_eq!(9, messages[first].turn);
    assert_eq!(
        CanonicalMessage::user_text("accepted work"),
        messages[first].message
    );
    assert_eq!(12, messages[first].admitted_at);
    let message_count = messages.len();

    let retry = resumed_current_input(&mut messages, 21, Some(9), "accepted work", 13).unwrap();
    assert_eq!(first, retry);
    assert_eq!(message_count, messages.len());
    assert_eq!(
        Some(12),
        input_admissions(&messages, 21, Some(9))
            .next()
            .map(|entry| entry.admitted_at)
    );
    assert_eq!(
        Some(12),
        input_admissions(&messages, 21, Some(9))
            .next_back()
            .map(|entry| entry.admitted_at)
    );
}
