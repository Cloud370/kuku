use crate::context::{CanonicalMessage, MessageBlock, Role};
use crate::error::Result;
use crate::notice::{
    build_runtime_notices, compute_context_headroom, render_notice_body, types::ContextHeadroom,
    NoticeAssemblyInput,
};

pub(super) fn should_trigger_handoff(headroom: &ContextHeadroom, threshold: f64) -> bool {
    let Some(remaining) = headroom.remaining_input_tokens else {
        return false;
    };
    let budget = headroom
        .max_context_tokens
        .saturating_sub(headroom.reserved_output_tokens)
        .saturating_sub(headroom.reserved_margin_tokens);
    if budget == 0 {
        return false;
    }

    let used_ratio = 1.0 - (f64::from(remaining) / f64::from(budget));
    used_ratio >= threshold
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_runtime_blocks(
    workspace: &std::path::Path,
    workspace_capability: Option<&dyn crate::query::WorkspaceQueryCapability>,
    conversation: &str,
    turn: u64,
    agent_registry: Option<&crate::agent::registry::AgentRegistry>,
    skill_registry: Option<&crate::skill::registry::SkillRegistry>,
    previous_skill_registry: Option<&crate::skill::registry::SkillRegistry>,
    resolved_config: &crate::provider::types::ResolvedProvider,
    existing_events: &[crate::event::StoredEvent],
    catalog: &crate::prompt::PromptCatalog,
) -> Result<(Option<String>, Option<String>, Option<String>)> {
    let estimated_input = super::last_input_tokens(&resolved_config.kind, existing_events);
    let thinking_overhead = resolved_config.think_level.overhead_tokens();
    let context_headroom = compute_context_headroom(
        resolved_config
            .max_context_tokens
            .saturating_sub(thinking_overhead),
        Some(resolved_config.max_output_tokens),
        estimated_input,
    );

    let catalog_text =
        agent_registry.and_then(|reg| crate::agent::catalog::render_agent_catalog(reg, catalog));

    let skills_text = skill_registry.and_then(|skill_reg| {
        let loaded_skill_names = if workspace_capability.is_some() {
            crate::skill::session::latest_snapshot_skill_names(existing_events, conversation)
        } else {
            crate::skill::session::loaded_skill_names(existing_events, conversation)
        };
        let skill_changes = if turn > 1 {
            previous_skill_registry.and_then(|previous_skill_registry| {
                crate::skill::registry::detect_skill_changes(previous_skill_registry, skill_reg)
            })
        } else {
            None
        };
        crate::skill::catalog::render_skill_catalog(
            skill_reg,
            &loaded_skill_names,
            skill_changes.as_ref(),
        )
    });

    let mut notice_bodies: Vec<String> = Vec::new();

    if turn > 1 {
        let conversation = crate::conversation::address::ConversationAddress::parse(conversation)
            .unwrap_or(crate::conversation::address::ConversationAddress::MAIN);
        let notice_events = existing_events
            .iter()
            .filter(|event| {
                !matches!(
                    &event.payload,
                    crate::event::EventPayload::MessageUser {
                        conversation: event_conversation,
                        from: Some(_),
                        via_tool_call_id: Some(_),
                        ..
                    } if event_conversation == conversation.as_str()
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        let notices = build_runtime_notices(NoticeAssemblyInput {
            workspace,
            workspace_capability,
            events: &notice_events,
            context_budget_tier: context_headroom.tier,
            conversation: &conversation,
            agent_registry,
        })?;
        for notice in &notices {
            if let Some(body) = render_notice_body(notice, catalog) {
                notice_bodies.push(body);
            }
        }
    }

    let runtime_blocks = if notice_bodies.is_empty() {
        None
    } else {
        Some(notice_bodies.join("\n\n"))
    };

    Ok((catalog_text, skills_text, runtime_blocks))
}

pub(super) fn assembly_runtime_prefix(
    runtime_context: Option<&str>,
    skill_body: Option<&str>,
    catalog: &crate::prompt::PromptCatalog,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(runtime_context) = runtime_context.filter(|value| !value.is_empty()) {
        let template = catalog
            .blocks
            .get("runtime-notices")
            .map(|asset| asset.text.as_str())
            .unwrap_or("<kuku_runtime_notices>{{runtime_notices_content}}</kuku_runtime_notices>");
        parts.push(template.replace("{{runtime_notices_content}}", runtime_context));
    }
    if let Some(skill_body) = skill_body.filter(|value| !value.is_empty()) {
        let template = catalog
            .blocks
            .get("conversation-inbox")
            .map(|asset| asset.text.as_str())
            .unwrap_or(
                "<kuku_conversation_inbox>{{conversation_inbox_content}}</kuku_conversation_inbox>",
            );
        parts.push(template.replace("{{conversation_inbox_content}}", skill_body));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

pub(super) fn append_handoff_instruction(
    prefix: Option<String>,
    instruction: &str,
    catalog: &crate::prompt::PromptCatalog,
) -> Option<String> {
    let Some(prefix) = prefix else {
        let notices_template = catalog
            .blocks
            .get("runtime-notices")
            .map(|asset| asset.text.as_str())
            .unwrap_or("<kuku_runtime_notices>{{runtime_notices_content}}</kuku_runtime_notices>");
        let inbox_template = catalog
            .blocks
            .get("conversation-inbox")
            .map(|asset| asset.text.as_str())
            .unwrap_or(
                "<kuku_conversation_inbox>{{conversation_inbox_content}}</kuku_conversation_inbox>",
            );
        return Some(format!(
            "{}\n{}",
            notices_template.replace("{{runtime_notices_content}}", instruction),
            inbox_template.replace("{{conversation_inbox_content}}", ""),
        ));
    };

    if let Some(index) = prefix.rfind("</kuku_runtime_notices>") {
        let (before, after) = prefix.split_at(index);
        Some(format!("{before}\n\n{instruction}{after}"))
    } else {
        Some(format!("{prefix}\n{instruction}"))
    }
}

pub(super) fn build_current_user_message(prefix: Option<String>, prompt: &str) -> CanonicalMessage {
    let mut blocks = Vec::new();
    if let Some(prefix) = prefix {
        blocks.push(MessageBlock::Text(prefix));
    }
    blocks.push(MessageBlock::Text(prompt.to_string()));
    CanonicalMessage::user(blocks)
}

fn replace_latest_user_message(
    history: &mut [CanonicalMessage],
    prompt: &str,
    replacement: CanonicalMessage,
) -> bool {
    for message in history.iter_mut().rev() {
        if message.role != Role::User || message.blocks.len() != 1 {
            continue;
        }
        let MessageBlock::Text(text) = &message.blocks[0] else {
            continue;
        };
        if text == prompt {
            *message = replacement;
            return true;
        }
    }
    false
}

pub(super) fn replace_current_user_message(
    history: &mut [CanonicalMessage],
    raw_prompt: &str,
    current_body: &str,
    replacement: CanonicalMessage,
) -> bool {
    if current_body != raw_prompt
        && replace_latest_user_message(history, current_body, replacement.clone())
    {
        return true;
    }
    replace_latest_user_message(history, raw_prompt, replacement)
}

pub(super) fn append_current_turn_prefix_once(messages: &mut Vec<CanonicalMessage>, prefix: &str) {
    if messages.iter().any(|message| {
        message.blocks.iter().any(|block| match block {
            MessageBlock::Text(text) => text.contains(prefix),
            MessageBlock::Thinking(_) | MessageBlock::ToolUse(_) | MessageBlock::ToolResult(_) => {
                false
            }
        })
    }) {
        return;
    }
    messages.push(CanonicalMessage::user_text(prefix.to_string()));
}

pub(super) fn insert_current_turn_metadata_block(message: &mut CanonicalMessage, text: String) {
    let insert_at = message.blocks.len().saturating_sub(1);
    message.blocks.insert(insert_at, MessageBlock::Text(text));
}
