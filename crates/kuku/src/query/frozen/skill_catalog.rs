use crate::Result;
use crate::context::runtime::{AdmittedMessage, FrozenSegment, MessageOrigin};
use crate::skill::model_catalog::{self, CatalogAnchor};

use super::{PendingRun, insert_runtime_notice};

pub(super) fn append_notice(
    pending: &PendingRun,
    segment: &FrozenSegment,
    messages: &mut Vec<AdmittedMessage>,
    frozen_head: usize,
) -> Result<Option<usize>> {
    let (Some(registry), Some(current)) = (&pending.skill_registry, &pending.skill_catalog_id)
    else {
        return Ok(None);
    };
    if !segment.tools.iter().any(|tool| {
        matches!(
            tool.name.as_str(),
            "list_skills" | "search_skills" | "use_skill"
        )
    }) {
        return Ok(None);
    }

    let visible = segment
        .prelude
        .iter()
        .chain(
            messages
                .iter()
                .filter(|entry| entry.origin == MessageOrigin::Runtime)
                .map(|entry| &entry.message),
        )
        .filter_map(model_catalog::anchor);
    let mut known = None;
    for anchor in visible {
        match anchor {
            CatalogAnchor::Full(id) => known = Some(id),
            CatalogAnchor::Delta { from, to } if known == Some(from) => known = Some(to),
            CatalogAnchor::Delta { .. } => known = None,
        }
    }
    let notice = match known {
        Some(id) if id == current => return Ok(None),
        Some(id) => {
            let previous = crate::skill::session::read_catalog_id(&pending.kuku_home, id)?;
            model_catalog::changes(&previous, registry, id, current)
        }
        None => Some(model_catalog::full(registry, current)),
    };
    let admitted_at = messages
        .iter()
        .rev()
        .find(|entry| entry.turn == pending.turn && entry.origin == MessageOrigin::Input)
        .map_or(0, |entry| entry.admitted_at);
    Ok(notice.as_ref().map(|notice| {
        insert_runtime_notice(messages, pending.turn, notice, frozen_head, admitted_at)
    }))
}
