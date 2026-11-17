use sha2::{Digest, Sha256};

use crate::context::{CanonicalMessage, ContextAssembly};
use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::provider::types::{PreparedProviderRequest, ResolvedProvider};
use crate::{Error, Result};

use super::helpers::now_timestamp;
use super::types::PendingRun;

pub(super) fn freeze(
    pending: &PendingRun,
    assembly: &mut ContextAssembly,
    resolved: &ResolvedProvider,
    events: &[StoredEvent],
    catalog: &crate::prompt::PromptCatalog,
) -> Result<(u64, usize)> {
    let agent = pending.conversation.as_str();
    let configuration = events
        .iter()
        .rev()
        .find_map(|event| match &event.payload {
            EventPayload::Unknown(value)
                if value["kind"] == "runtime.operation"
                    && value["record"]["address"] == agent
                    && matches!(
                        value["record"]["operation"].as_str(),
                        Some("agent_created" | "agent_configured")
                    ) =>
            {
                Some(event.id)
            }
            _ => None,
        })
        .unwrap_or(0);
    let handoff = events.iter().rev().find_map(|event| match &event.payload {
        EventPayload::Handoff { turn, .. } if events.iter().any(|entry| matches!(&entry.payload, EventPayload::TurnStarted { turn: candidate, conversation, .. } if candidate == turn && conversation == agent)) => Some(event.id),
        _ => None,
    }).unwrap_or(0);
    let signature = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            configuration,
            handoff,
            resolved.kind.as_str(),
            &assembly.tools
        ))?)
    );
    let previous = events.iter().rev().find_map(|event| match &event.payload {
        EventPayload::Unknown(value)
            if value["kind"] == "runtime.context_segment" && value["agent"] == agent =>
        {
            Some((event.id, value))
        }
        _ => None,
    });
    let mut writer = EventStore::open(&pending.events_path)?;
    let segment = if let Some((id, value)) =
        previous.filter(|(_, value)| value["signature"] == signature)
    {
        assembly.system_prompt = serde_json::from_value(value["system_prompt"].clone())?;
        assembly.prelude_messages = serde_json::from_value(value["prelude"].clone())?;
        assembly.tools = serde_json::from_value(value["tools"].clone())?;
        id
    } else {
        writer.append(EventPayload::Unknown(serde_json::json!({"kind":"runtime.context_segment","ts":now_timestamp()?,"agent":agent,"signature":signature,"predecessor":previous.map(|(id,_)|id),"configuration":configuration,"handoff":handoff,"system_prompt":assembly.system_prompt,"prelude":assembly.prelude_messages,"tools":assembly.tools,"catalog":catalog})))?.id
    };
    let frozen = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::Unknown(value)
                if value["kind"] == "runtime.context_admitted"
                    && value["agent"] == agent
                    && value["segment"] == segment =>
            {
                Some(serde_json::from_value::<CanonicalMessage>(
                    value["message"].clone(),
                ))
            }
            _ => None,
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if frozen.len() > assembly.history.len() {
        return Err(Error::InvalidEventStream(format!("agent '{agent}' prepared history would remove committed context without a successor segment")));
    }
    for (index, message) in frozen.iter().enumerate() {
        if assembly.history[index].role != message.role {
            return Err(Error::InvalidEventStream(format!(
                "agent '{agent}' prepared history changed a committed context role at {index}"
            )));
        }
        assembly.history[index] = message.clone();
    }
    for (ordinal, message) in assembly.history.iter().enumerate().skip(frozen.len()) {
        writer.append(EventPayload::Unknown(serde_json::json!({"kind":"runtime.context_admitted","ts":now_timestamp()?,"agent":agent,"segment":segment,"ordinal":ordinal,"message":message})))?;
    }
    Ok((segment, assembly.history.len()))
}

pub(super) fn record_prepared(
    pending: &PendingRun,
    request_id: &str,
    prepared: &PreparedProviderRequest,
    context: (u64, usize),
) -> Result<()> {
    let body_hash = format!("{:x}", Sha256::digest(serde_json::to_vec(&prepared.body)?));
    EventStore::open(&pending.events_path)?.append(EventPayload::Unknown(serde_json::json!({"kind":"runtime.step_prepared","ts":now_timestamp()?,"agent":pending.conversation.as_str(),"turn":pending.turn,"request_id":request_id,"segment":context.0,"context_head":context.1,"connection":prepared.connection_name,"model":prepared.model,"url":prepared.url,"settings":prepared.settings,"correlation_id":prepared.correlation_id,"body_hash":body_hash})))?;
    Ok(())
}
