use std::collections::BTreeSet;

use crate::context::runtime::{AdmittedMessage, MessageOrigin};
use crate::context::{CanonicalMessage, MessageBlock};
use crate::skill::session::{admission_body, has_admission};
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

pub(super) async fn append_input_skills(
    pending: &mut PendingRun,
    messages: &mut Vec<AdmittedMessage>,
    prelude: &[CanonicalMessage],
) -> Result<Vec<String>> {
    let admitted_at = messages
        .iter()
        .rev()
        .find(|message| message.turn == pending.turn && message.origin == MessageOrigin::Input)
        .map_or(0, |message| message.admitted_at);
    let mut names = pending.query.input_skills.clone();
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
    let mut admitted = Vec::new();
    for name in names {
        let registry = pending.skill_registry.as_ref();
        let body = registry
            .and_then(|registry| admission_body(registry, &name))
            .ok_or_else(|| {
                Error::InvalidArgument(format!("selected input skill '{name}' was not found"))
            })?;
        if prelude
            .iter()
            .chain(messages.iter().map(|admitted| &admitted.message))
            .any(|message| has_admission(message, &body))
        {
            continue;
        }
        if let Some(version) = registry
            .filter(|registry| {
                registry
                    .get(&name)
                    .is_some_and(|def| def.source == crate::skill::definition::SkillSource::Builtin)
            })
            .and_then(|registry| registry.docs_bundle())
        {
            crate::skill::bundle::retain(&pending.kuku_home, version).await?;
        }
        admitted.push(name);
        messages.push(AdmittedMessage {
            turn: pending.turn,
            origin: MessageOrigin::Runtime,
            message: CanonicalMessage::user_text(body),
            admitted_at,
        });
    }
    Ok(admitted)
}
