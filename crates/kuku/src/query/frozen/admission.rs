use std::collections::BTreeSet;

use crate::context::runtime::{AdmittedMessage, MessageOrigin};
use crate::context::{CanonicalMessage, MessageBlock};
use crate::skill::bundle;
use crate::{Error, Result};

use super::PendingRun;

pub(super) fn model_history(messages: &[AdmittedMessage], turn: u64) -> Vec<CanonicalMessage> {
    let media_turn = messages
        .iter()
        .map(|admitted| admitted.turn)
        .filter(|candidate| *candidate < turn)
        .max()
        .unwrap_or(turn);
    messages
        .iter()
        .map(|admitted| {
            let mut message = admitted.message.clone();
            if admitted.turn < media_turn {
                for block in &mut message.blocks {
                    if let MessageBlock::ToolResult(result) = block {
                        if result
                            .structured
                            .as_ref()
                            .is_some_and(|facts| facts["kind"] == "image_view")
                        {
                            result.structured = None;
                        }
                    }
                }
            }
            message
        })
        .collect()
}

pub(super) fn select_turns(messages: &mut Vec<AdmittedMessage>, through: u64, keep: usize) {
    let retained = messages
        .iter()
        .filter(|message| message.turn <= through)
        .map(|message| message.turn)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .rev()
        .take(keep)
        .collect::<BTreeSet<_>>();
    messages.retain(|message| message.turn > through || retained.contains(&message.turn));
}

pub(super) fn append_input_skills(
    pending: &mut PendingRun,
    messages: &mut Vec<AdmittedMessage>,
) -> Result<Vec<String>> {
    let admitted_at = messages
        .iter()
        .rev()
        .find(|message| message.turn == pending.turn && message.origin == MessageOrigin::Input)
        .map_or(0, |message| message.admitted_at);
    let mut names = std::mem::take(&mut pending.query.input_skills);
    if names.is_empty() {
        return Ok(names);
    }
    if pending.query.disable_skills {
        return Err(Error::InvalidArgument(
            "selected input skill cannot load while skills are disabled".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    names.retain(|name| seen.insert(name.clone()));
    if names.iter().any(|name| name == bundle::NAME) {
        let version = pending
            .skill_registry
            .as_ref()
            .and_then(|registry| registry.docs_bundle())
            .ok_or_else(|| {
                Error::InvalidArgument(
                    "kuku-docs is unavailable in this Turn's Skill snapshot".into(),
                )
            })?;
        let events = super::super::tool_exec::current_skill_events(pending)?;
        if crate::skill::session::docs_loaded_in_turn(
            &events,
            pending.conversation.as_str(),
            pending.turn,
            version,
        ) {
            names.retain(|name| name != bundle::NAME);
        } else {
            bundle::retain(&pending.kuku_home, version)?;
        }
    }
    let discovered = names
        .iter()
        .any(|name| {
            pending
                .skill_registry
                .as_ref()
                .is_none_or(|registry| registry.get(name).is_none())
        })
        .then(|| {
            crate::skill::session::build_registry_snapshot_for_host(
                &pending.kuku_home,
                &pending.workspace,
                &pending.config,
            )
        })
        .transpose()?;
    for name in &names {
        let definition = pending
            .skill_registry
            .as_ref()
            .and_then(|registry| registry.get(name))
            .or_else(|| discovered.as_ref().and_then(|registry| registry.get(name)))
            .ok_or_else(|| {
                Error::InvalidArgument(format!("selected input skill '{name}' was not found"))
            })?;
        messages.push(AdmittedMessage {
            turn: pending.turn,
            origin: MessageOrigin::Runtime,
            message: CanonicalMessage::user_text(definition.instructions.clone()),
            admitted_at,
        });
    }
    Ok(names)
}
