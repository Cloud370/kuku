use serde_json::json;

use crate::event::{EventPayload, EventStore};
use crate::{Error, Result};

use super::{now_timestamp, ContextCommit, PendingRun};

fn context_batch(
    pending: &PendingRun,
    context: &mut ContextCommit,
    mut confirmation: serde_json::Value,
    first_id: u64,
) -> Result<Vec<EventPayload>> {
    let cursor = pending
        .context_cursor
        .as_ref()
        .expect("runtime context cursor exists");
    let mut facts = Vec::new();
    let predecessor = cursor.segment.as_ref().map(|segment| segment.id);
    if context.new_segment {
        context.segment.id = first_id;
        facts.push(EventPayload::Unknown(json!({"kind":"runtime.context_segment","ts":now_timestamp()?,"agent":pending.conversation.as_str(),
            "signature":context.segment.signature,"predecessor":predecessor,"configuration":context.segment.configuration,"handoff":context.segment.handoff,"reason":context.reason,"checkpointed_messages":context.checkpointed_messages,
            "connection":context.segment.connection,"protocol":context.segment.protocol,"model":context.segment.model,"endpoint":context.segment.endpoint,
            "capabilities":context.segment.capabilities,"route_selectors":context.segment.route_selectors,
            "system_prompt":context.segment.system_prompt,"prelude":context.segment.prelude,"tools":context.segment.tools,"catalog":context.segment.catalog,
            "behavior":context.segment.behavior,"identity_slot":context.segment.identity_slot,"handoff_prelude":context.segment.handoff_prelude,"workspace":context.segment.workspace,
            "project_instruction_sources":context.segment.project_instruction_sources,
            "checkpoint_files":context.segment.checkpoint_files,
            "checkpoint_files_omitted":context.segment.checkpoint_files_omitted})));
        context.checkpoint_file_delta.append_events(
            &mut facts,
            pending.conversation.as_str(),
            first_id,
            &now_timestamp()?,
        )?;
    }
    let frozen_head = context.prefix_context_head;
    let expected_frozen_head = if context.new_segment {
        0
    } else {
        cursor.admitted.len()
    };
    if frozen_head != expected_frozen_head {
        return Err(Error::InvalidEventStream(
            "prepared history boundary differs from the committed prefix".into(),
        ));
    }
    if context.messages.len() < frozen_head
        || context
            .messages
            .iter()
            .zip(&cursor.admitted)
            .take(frozen_head)
            .any(|(selected, frozen)| selected.message != frozen.message)
    {
        return Err(Error::InvalidEventStream(
            "prepared context would change its committed prefix without a successor segment".into(),
        ));
    }
    for (ordinal, admitted) in context.messages.iter().enumerate().skip(frozen_head) {
        facts.push(EventPayload::Unknown(json!({"kind":"runtime.context_admitted","ts":now_timestamp()?,"agent":pending.conversation.as_str(),"segment":context.segment.id,
            "ordinal":ordinal,"turn":admitted.turn,"origin":admitted.origin,"source_head":context.source_head,"admitted_at":admitted.admitted_at,"message":admitted.message})));
    }
    confirmation["segment"] = context.segment.id.into();
    confirmation["context_head"] = context.messages.len().into();
    confirmation["source_head"] = context.source_head.into();
    facts.push(EventPayload::Unknown(confirmation));
    Ok(facts)
}

pub(super) fn commit_context(
    pending: &mut PendingRun,
    mut context: ContextCommit,
    confirmation: serde_json::Value,
    optional: bool,
) -> Result<Option<u64>> {
    let execution = pending
        .query
        .runtime_execution
        .expect("context belongs to a session execution");
    let session = super::super::provider::runtime_session(pending, execution)?;
    let mut accepted = true;
    session.commit_model_batch(pending.conversation.as_str(), execution, |first| {
        let payloads = context_batch(pending, &mut context, confirmation, first)?;
        if optional {
            match EventStore::validate_batch_records(first, &payloads) {
                Err(Error::InvalidArgument(_)) => {
                    accepted = false;
                    return Ok(Vec::new());
                }
                result => result?,
            }
        }
        Ok(payloads)
    })?;
    if !accepted {
        return Ok(None);
    }
    let segment_id = context.segment.id;
    pending
        .context_cursor
        .as_mut()
        .expect("runtime context cursor exists")
        .commit(context.segment, context.messages, context.source_head)?;
    Ok(Some(segment_id))
}
