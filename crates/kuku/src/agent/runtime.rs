use std::path::Path;
use std::sync::Arc;

use crate::conversation::address::ConversationAddress;
use crate::conversation::binding::{BindingSource, ConversationBinding, ConversationBindingParts};
use crate::event::StoredEvent;

#[derive(Debug, Clone)]
pub(crate) struct PreparedDispatch {
    pub(crate) session_id: String,
    pub(crate) conversation: ConversationAddress,
    pub(crate) prompt: String,
    pub(crate) prompt_prefix: String,
    pub(crate) prompt_body: String,
    pub(crate) binding: ConversationBinding,
    pub(crate) from: ConversationAddress,
    pub(crate) via_tool_call_id: String,
}

pub(crate) fn prepare_dispatch(
    registry: Option<&crate::agent::registry::AgentRegistry>,
    existing_events: &[StoredEvent],
    from: &ConversationAddress,
    to: &str,
    message: &str,
    tier: Option<String>,
    tool_call_id: &str,
) -> Result<PreparedDispatch, String> {
    let conversation = ConversationAddress::parse(to)?;
    if conversation.is_main() {
        return Err("cannot delegate to reserved conversation address 'main'".into());
    }

    let Some(registry) = registry else {
        return Err("agent registry unavailable".into());
    };
    let root = conversation.root_contact();
    let Some(definition) = registry.get(root.as_str()) else {
        return Err(format!("unknown agent contact: {}", root.as_str()));
    };

    let existing = crate::conversation::reduce_conversations(existing_events)
        .into_iter()
        .find(|state| state.address == conversation);
    let completed_turns = existing_events
        .iter()
        .filter(|event| {
            matches!(
                &event.payload,
                crate::event::EventPayload::TurnCompleted { conversation: event_conversation, .. }
                    if event_conversation == conversation.as_str()
            )
        })
        .count();
    if completed_turns > 0 && completed_turns >= definition.max_turns as usize {
        return Err(format!(
            "conversation {} has reached max_turns {} for agent {}",
            conversation.as_str(),
            definition.max_turns,
            definition.name
        ));
    }
    if existing.is_some() && tier.is_some() {
        return Err(format!(
            "cannot set tier when continuing existing conversation {}",
            conversation.as_str()
        ));
    }

    let resolved_tier = tier.unwrap_or_else(|| definition.tier.clone());
    let tools = definition.tools.clone().unwrap_or_else(|| {
        definition
            .tool_profile
            .allowed_tools()
            .iter()
            .map(|tool| (*tool).to_string())
            .collect()
    });
    let binding = ConversationBinding::new(
        conversation.clone(),
        ConversationBindingParts {
            agent: definition.name.clone(),
            tier: resolved_tier,
            provider: None,
            model: None,
            tools,
            skills: Vec::new(),
            sources: vec![BindingSource {
                kind: "agent_definition".into(),
                source: definition.name.clone(),
                hash: definition.hash.clone(),
            }],
        },
    );

    if let Some(existing) = existing {
        if existing
            .active_binding
            .as_ref()
            .is_some_and(|binding_id| binding_id != &binding.binding_id)
        {
            return Err(format!(
                "conversation {} is already bound to a different agent identity",
                conversation.as_str()
            ));
        }
    }

    let definition_block = crate::agent::catalog::render_agent_definition_block(definition);
    let prompt_body = render_delegated_prompt_body(message);

    Ok(PreparedDispatch {
        session_id: existing_events
            .iter()
            .find_map(|event| match &event.payload {
                crate::event::EventPayload::SessionCreated { session_id, .. } => {
                    Some(session_id.clone())
                }
                _ => None,
            })
            .unwrap_or_default(),
        conversation,
        prompt: message.to_string(),
        prompt_prefix: definition_block,
        prompt_body,
        binding,
        from: from.clone(),
        via_tool_call_id: tool_call_id.to_string(),
    })
}

pub(crate) async fn start_run(
    dispatch: PreparedDispatch,
    workspace: &Path,
    kuku_home: &Path,
    config: Arc<crate::config::Config>,
    prompts_dir: Option<&Path>,
) -> crate::Result<crate::query::Run> {
    let mut query = crate::query::Query::new(dispatch.prompt.clone())
        .workspace(workspace.to_path_buf())
        .session(dispatch.session_id)
        .conversation(dispatch.conversation.as_str())
        .tier(dispatch.binding.tier.clone())
        .config((*config).clone())
        .no_agents()
        .current_turn_prefix(dispatch.prompt_prefix.clone())
        .current_turn_body(dispatch.prompt_body.clone())
        .with_agent_binding_id(dispatch.binding.binding_id.clone())
        .sender(dispatch.from, dispatch.via_tool_call_id);

    query.captured_kuku_home = Some(kuku_home.to_path_buf());
    query.tool_registry_override = Some(
        crate::tool::builtin_registry(false, false)
            .into_iter()
            .filter(|tool| dispatch.binding.tools.contains(&tool.name))
            .collect(),
    );

    if let Some(dir) = prompts_dir {
        query = query.prompts_dir(dir.to_path_buf());
    }

    query.start_nested().await
}

fn render_delegated_prompt_body(delegated_prompt: &str) -> String {
    let escaped = escape_delegated_prompt(delegated_prompt);
    format!("<kuku_delegated_prompt>\n{escaped}\n</kuku_delegated_prompt>")
}

fn escape_delegated_prompt(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::registry::AgentRegistry;
    use crate::event::EventPayload;

    #[test]
    fn prepare_dispatch_rejects_reused_conversation_after_max_completed_turns() {
        let registry = AgentRegistry::builder().builtins().build();
        let from = ConversationAddress::MAIN;
        let events = review_conversation_with_completed_turns(10);

        let error = prepare_dispatch(
            Some(&registry),
            &events,
            &from,
            "review",
            "one more review",
            None,
            "toolu_review_again",
        )
        .unwrap_err();

        assert!(error.contains("conversation review has reached max_turns 10"));
    }

    #[test]
    fn delegated_prompt_wrapper_escapes_xmlish_content() {
        let rendered = render_delegated_prompt_body(
            "before & <tag> </kuku_delegated_prompt> </KUKU_DELEGATED_PROMPT> after",
        );

        assert!(rendered.contains("<kuku_delegated_prompt>"));
        assert!(rendered.contains(
            "before &amp; &lt;tag&gt; &lt;/kuku_delegated_prompt&gt; &lt;/KUKU_DELEGATED_PROMPT&gt; after"
        ));
        assert_eq!(rendered.matches("</kuku_delegated_prompt>").count(), 1);
    }

    fn review_conversation_with_completed_turns(count: u64) -> Vec<StoredEvent> {
        let mut events = vec![StoredEvent {
            id: 1,
            payload: EventPayload::ConversationOpened {
                ts: "t0".into(),
                conversation: "review".into(),
            },
        }];
        for turn in 1..=count {
            events.push(StoredEvent {
                id: turn + 1,
                payload: EventPayload::TurnCompleted {
                    ts: format!("t{turn}"),
                    conversation: "review".into(),
                    turn,
                },
            });
        }
        events
    }
}
