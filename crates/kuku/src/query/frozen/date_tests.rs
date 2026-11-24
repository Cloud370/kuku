use super::{FrozenSegment, custom_project_policy_has_date, insert_runtime_notice};
use crate::context::CanonicalMessage;
use crate::context::runtime::{AdmittedMessage, MessageOrigin};

#[test]
fn midnight_date_change_does_not_reorder_admitted_tool_turn_input() {
    let old_date = CanonicalMessage::user_text("Current date: 2026-05-18");
    let user_input = CanonicalMessage::user_text("continue after tool result");
    let mut messages = vec![
        AdmittedMessage {
            turn: 3,
            origin: MessageOrigin::Runtime,
            message: old_date,
            admitted_at: 1,
        },
        AdmittedMessage {
            turn: 3,
            origin: MessageOrigin::Input,
            message: user_input,
            admitted_at: 2,
        },
    ];
    let frozen_prefix: Vec<_> = messages
        .iter()
        .map(|admitted| admitted.message.clone())
        .collect();
    let next_date = CanonicalMessage::user_text("Current date: 2026-05-19");

    insert_runtime_notice(&mut messages, 3, &next_date, frozen_prefix.len(), 2);

    assert_eq!(
        frozen_prefix,
        messages
            .iter()
            .take(frozen_prefix.len())
            .map(|admitted| admitted.message.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(next_date, messages.last().unwrap().message);
    assert_eq!(MessageOrigin::Runtime, messages.last().unwrap().origin);
    assert_eq!(2, messages.last().unwrap().admitted_at);
}

#[test]
fn custom_policy_date_ignores_incidental_memory_dates_in_both_prelude_orders() {
    let mut catalog = crate::prompt::builtin_prompt_catalog();
    catalog.blocks.get_mut("project-policy").unwrap().text =
        "Policy\n- Date: {{current_date}}\n{{project_instructions_rendered}}".into();
    let policy =
        CanonicalMessage::user_text("Policy\n- Date: 2026-05-18\nproject notes dated 2026-05-19");
    let memory = CanonicalMessage::user_text("memory recorded on 2026-05-19");
    let incidental = CanonicalMessage::user_text("- Date: 2026-05-19");
    let guidance = CanonicalMessage::user_text("common guidance");
    let behavior = CanonicalMessage::user_text("shared behavior");
    for (prelude, slot) in [
        (
            vec![
                policy.clone(),
                behavior.clone(),
                guidance.clone(),
                memory.clone(),
                incidental.clone(),
            ],
            1,
        ),
        (vec![guidance, behavior, policy, memory, incidental], 1),
    ] {
        let segment = FrozenSegment {
            id: 1,
            signature: String::new(),
            configuration: 0,
            handoff: 0,
            system_prompt: String::new(),
            prelude,
            tools: Vec::new(),
            catalog: catalog.clone(),
            connection: String::new(),
            protocol: String::new(),
            model: String::new(),
            endpoint: String::new(),
            capabilities: None,
            route_selectors: None,
            behavior: "shared behavior".into(),
            project_instruction_sources: Vec::new(),
            identity_slot: slot,
            handoff_prelude: None,
            checkpoint_files: Default::default(),
            checkpoint_files_omitted: false,
            workspace: String::new(),
        };
        assert!(!custom_project_policy_has_date(&segment, "2026-05-19"));
        assert!(custom_project_policy_has_date(&segment, "2026-05-18"));
    }
}
