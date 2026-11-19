use super::*;
use crate::context::ContextAssembly;

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
    let headroom = compute_context_headroom(200_000, Some(64_000), Some(150_000));

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

fn assembly_with_messages(
    prelude_messages: Vec<CanonicalMessage>,
    history: Vec<CanonicalMessage>,
) -> ContextAssembly {
    ContextAssembly {
        system_prompt: String::new(),
        prelude_messages,
        history,
        tools: Vec::new(),
        prompt_asset_sources: Vec::new(),
        project_instruction_sources: Vec::new(),
        memory_sources: Vec::new(),
        runtime_context: None,
        handoff_summary: None,
    }
}

#[test]
fn custom_runtime_date_is_deduplicated_when_embedded_in_combined_text() {
    let current_date = "2026-05-18";
    let runtime_date = format!("<custom-date>今天：{current_date}</custom-date>");
    let combined_text = format!("runtime notices\n{runtime_date}\ncurrent input");
    let assembly =
        assembly_with_messages(Vec::new(), vec![CanonicalMessage::user_text(combined_text)]);

    assert!(contains_current_date(&assembly, &runtime_date));
    assert!(!contains_current_date(
        &assembly,
        &format!("Current date: {current_date}")
    ));
}

#[test]
fn legacy_policy_date_detection_requires_the_recorded_template_placeholder() {
    let current_date = "2026-05-18";
    let assembly = assembly_with_messages(
        vec![CanonicalMessage::user_text(format!(
            "Policy date: {current_date}"
        ))],
        Vec::new(),
    );
    let mut legacy_catalog = crate::prompt::builtin_prompt_catalog();
    legacy_catalog
        .blocks
        .get_mut("project-policy")
        .unwrap()
        .text
        .push_str(" {{current_date}}");

    assert!(legacy_project_policy_has_date(
        &assembly,
        &legacy_catalog,
        current_date
    ));
    assert!(!legacy_project_policy_has_date(
        &assembly,
        &crate::prompt::builtin_prompt_catalog(),
        current_date
    ));
}
