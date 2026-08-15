use super::*;

fn message_text(message: &CanonicalMessage) -> &str {
    match &message.blocks[0] {
        MessageBlock::Text(text) => text,
        _ => panic!("expected text block"),
    }
}

#[test]
fn handoff_trigger_requires_known_token_headroom() {
    let headroom = compute_context_headroom(200_000, Some(64_000), None);

    assert!(!should_trigger_handoff(&headroom, 0.7));
}

#[test]
fn handoff_trigger_uses_known_token_headroom() {
    let headroom = compute_context_headroom(200_000, Some(64_000), Some(125_000));

    assert!(should_trigger_handoff(&headroom, 0.7));
}

#[test]
fn delegated_body_replacement_prefers_current_wrapped_message() {
    let raw = "same text";
    let wrapped = "<kuku_delegated_prompt>\nsame text\n</kuku_delegated_prompt>";
    let replacement = CanonicalMessage::user_text("provider body");
    let mut history = vec![
        CanonicalMessage::user_text(raw),
        CanonicalMessage::assistant(vec![MessageBlock::Text("answer".to_string())]),
        CanonicalMessage::user_text(wrapped),
    ];

    assert!(replace_current_user_message(&mut history, raw, wrapped, replacement).is_some());

    assert_eq!(message_text(&history[0]), raw);
    assert_eq!(message_text(&history[2]), "provider body");
}

#[test]
fn current_turn_prefix_is_appended_once_to_restored_prelude() {
    let prefix = "You are a code and document reviewer";
    let mut missing = vec![CanonicalMessage::user_text("old snapshot")];
    append_current_turn_prefix_once(&mut missing, prefix);
    assert_eq!(missing.len(), 2);
    assert_eq!(message_text(&missing[1]), prefix);

    append_current_turn_prefix_once(&mut missing, prefix);
    assert_eq!(missing.len(), 2);

    let mut existing = vec![CanonicalMessage::user_text(format!(
        "before {prefix} after"
    ))];
    append_current_turn_prefix_once(&mut existing, prefix);
    assert_eq!(existing.len(), 1);
}
