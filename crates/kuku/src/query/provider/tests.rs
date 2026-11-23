use super::*;
use crate::context::ContextAssembly;

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
fn handoff_trigger_requires_known_token_headroom() {
    let headroom = compute_context_headroom(200_000, Some(64_000), None);

    assert!(!should_trigger_handoff(&headroom, 0.7));
}

#[test]
fn custom_policy_date_ignores_incidental_dates_in_project_notes_and_memory() {
    let mut catalog = crate::prompt::builtin_prompt_catalog();
    catalog.blocks.get_mut("project-policy").unwrap().text =
        "Policy\n- Date: {{current_date}}\n{{project_instructions_rendered}}".into();
    let mut assembly = assembly_with_messages(
        vec![
            CanonicalMessage::user_text(
                "Policy\n- Date: 2026-05-18\nproject notes dated 2026-05-19",
            ),
            CanonicalMessage::user_text("memory recorded on 2026-05-19"),
            CanonicalMessage::user_text("- Date: 2026-05-19"),
        ],
        Vec::new(),
    );
    assert!(!custom_project_policy_has_date(
        &assembly,
        &catalog,
        "2026-05-19"
    ));
    assert!(custom_project_policy_has_date(
        &assembly,
        &catalog,
        "2026-05-18"
    ));
    assembly.prelude_messages[0] = CanonicalMessage::user_text("Policy\n- Date: 2026-05-19\nnotes");
    assert!(custom_project_policy_has_date(
        &assembly,
        &catalog,
        "2026-05-19"
    ));
}

#[test]
fn handoff_trigger_uses_known_token_headroom() {
    let headroom = compute_context_headroom(200_000, Some(64_000), Some(150_000));

    assert!(should_trigger_handoff(&headroom, 0.7));
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
fn custom_policy_date_detection_requires_the_recorded_template_placeholder() {
    let current_date = "2026-05-18";
    let assembly = assembly_with_messages(
        vec![CanonicalMessage::user_text(format!(
            "project policy\n- Current date: {current_date}"
        ))],
        Vec::new(),
    );
    let mut custom_catalog = crate::prompt::builtin_prompt_catalog();
    custom_catalog
        .blocks
        .get_mut("project-policy")
        .unwrap()
        .text =
        "project policy\n- Current date: {{current_date}}\n{{project_instructions_rendered}}"
            .into();

    assert!(custom_project_policy_has_date(
        &assembly_with_messages(
            vec![CanonicalMessage::user_text(
                "project policy\n- Current date: 2026-05-18\nproject notes"
            )],
            Vec::new(),
        ),
        &custom_catalog,
        current_date
    ));
    assert!(!custom_project_policy_has_date(
        &assembly,
        &crate::prompt::builtin_prompt_catalog(),
        current_date
    ));
}
