use std::collections::BTreeSet;
use std::time::Instant;

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::context::assembly::render_project_policy;
use crate::context::runtime::{AdmittedMessage, FrozenSegment, MessageOrigin};
use crate::context::{
    assemble_context, CanonicalMessage, ContextAssembly, ContextInput, EnvironmentSource,
    MessageBlock,
};
use crate::event::{EventPayload, EventStore};
use crate::prompt::PromptCatalog;
use crate::provider::types::{PreparedProviderRequest, ResolvedProvider};
use crate::{Error, Result};

use super::helpers::{
    current_date_string, load_memory_sources, load_project_instruction_sources, now_timestamp,
    platform_label,
};
use super::request::OwnedRequestBase;
use super::types::PendingRun;

pub(super) struct PreparedContext {
    pub(super) request: OwnedRequestBase,
    pub(super) commit: ContextCommit,
}

pub(super) struct ContextCommit {
    segment: FrozenSegment,
    messages: Vec<AdmittedMessage>,
    source_head: u64,
    current_input: usize,
    new_segment: bool,
    reason: &'static str,
    context_prepare_us: u64,
    events_read: u64,
    input_skills: Vec<String>,
}

pub(super) fn prepare(pending: &mut PendingRun) -> Result<PreparedContext> {
    let started = Instant::now();
    let resolved = pending
        .resolved
        .as_ref()
        .expect("resolved runtime exists")
        .config
        .clone();
    let tools = crate::tool::to_tool_schemas(
        &pending
            .resolved
            .as_ref()
            .expect("resolved runtime exists")
            .registry,
    );
    let mut cursor = pending.context_cursor.take().unwrap_or_default();
    let read_before = cursor.events_read;
    cursor.refresh(
        &pending.events_path,
        pending.conversation.as_str(),
        pending.query.runtime_limits.max_request_bytes,
    )?;
    let catalog = cursor
        .catalog
        .clone()
        .unwrap_or_else(|| pending.catalog.clone());
    let behavior = pending
        .query
        .agent_instructions
        .clone()
        .or_else(|| {
            pending
                .agent_registry
                .as_ref()
                .and_then(|registry| registry.get(pending.conversation.root_contact().as_str()))
                .map(|definition| definition.instructions.clone())
        })
        .or_else(|| catalog.agents.get("main").map(|asset| asset.text.clone()))
        .unwrap_or_default();
    let handoff = cursor
        .handoff
        .as_ref()
        .map(|event| event.id)
        .or_else(|| cursor.segment.as_ref().map(|segment| segment.handoff))
        .unwrap_or(0);
    let signature = signature(pending, &resolved, &tools, &behavior, handoff)?;
    let previous = cursor.segment.clone();
    let new_segment = previous
        .as_ref()
        .is_none_or(|segment| segment.signature != signature);
    let route_changed = previous.as_ref().is_some_and(|segment| {
        segment.connection != resolved.connection_name
            || segment.protocol != resolved.kind.as_str()
            || segment.model != resolved.model
            || segment.endpoint != resolved.base_url
    });
    let handoff_changed = previous
        .as_ref()
        .map_or(cursor.handoff.is_some(), |segment| {
            segment.handoff != handoff
        });
    let workspace_changed = previous
        .as_ref()
        .is_some_and(|segment| segment.workspace != pending.workspace.to_string_lossy());
    let mut messages = cursor.history(&pending.conversation);
    let mut segment = match previous.as_ref() {
        Some(segment) => segment.clone(),
        None => initial_segment(pending, &catalog, &tools, &behavior)?,
    };
    if workspace_changed {
        segment = initial_segment(pending, &catalog, &tools, &behavior)?;
    }
    if handoff_changed || route_changed || workspace_changed {
        let (turn, keep) = match cursor
            .handoff
            .as_ref()
            .filter(|_| handoff_changed)
            .map(|event| &event.payload)
        {
            Some(EventPayload::Handoff {
                turn, keep_turns, ..
            }) => (*turn, *keep_turns),
            _ => (
                pending.turn.saturating_sub(1),
                pending.config.handoff().keep_turns,
            ),
        };
        select_turns(&mut messages, turn, keep);
    }
    if route_changed {
        for admitted in &mut messages {
            admitted.message.blocks.retain(|block| {
                !matches!(
                    block,
                    MessageBlock::ProviderReplay { .. } | MessageBlock::Thinking(_)
                )
            });
        }
        messages.retain(|admitted| !admitted.message.blocks.is_empty());
    }
    if new_segment {
        replace_behavior(&mut segment, &behavior)?;
        segment.signature = signature;
        segment.configuration = cursor.configuration.max(segment.configuration);
        segment.handoff = handoff;
        segment.connection = resolved.connection_name.clone();
        segment.protocol = resolved.kind.as_str().into();
        segment.model = resolved.model.clone();
        segment.endpoint = resolved.base_url.clone();
        segment.tools = tools;
        if handoff_changed {
            if let Some(EventPayload::Handoff { summary, .. }) =
                cursor.handoff.as_ref().map(|event| &event.payload)
            {
                let template = catalog.runtime.get("handoff-context").ok_or_else(|| {
                    Error::PromptRender("missing frozen handoff-context template".into())
                })?;
                if let Some(index) = segment.handoff_prelude.take() {
                    segment.prelude.remove(index);
                }
                segment.handoff_prelude = Some(segment.prelude.len());
                segment.prelude.push(CanonicalMessage::user_text(
                    template.text.replace("{{handoff_summary}}", summary),
                ));
            }
        }
    }
    let input_skills = append_input_skills(pending, &mut messages)?;
    append_dynamic(
        pending,
        &mut messages,
        &catalog,
        if handoff_changed || route_changed || workspace_changed {
            None
        } else {
            cursor.input_tokens
        },
        &resolved,
        if new_segment {
            0
        } else {
            cursor.admitted.len()
        },
    )?;
    let current_input = messages
        .iter()
        .rposition(|admitted| {
            admitted.turn == pending.turn && admitted.origin == MessageOrigin::Input
        })
        .ok_or_else(|| {
            Error::InvalidEventStream("prepared context has no admitted current input".into())
        })?;
    let assembly = ContextAssembly {
        system_prompt: segment.system_prompt.clone(),
        prelude_messages: segment.prelude.clone(),
        history: messages
            .iter()
            .map(|admitted| admitted.message.clone())
            .collect(),
        tools: segment.tools.clone(),
        prompt_asset_sources: Vec::new(),
        project_instruction_sources: Vec::new(),
        memory_sources: Vec::new(),
        runtime_context: None,
        handoff_summary: None,
    };
    let request = OwnedRequestBase::new(
        assembly,
        catalog,
        crate::provider::types::CanonicalPromptInput {
            parts: vec![messages[current_input].message.clone()],
        },
        resolved.model.clone(),
        resolved.max_output_tokens,
        pending.query.temperature,
        resolved.think_level,
        resolved.thinking.clone(),
        current_input,
    )?;
    let commit = ContextCommit {
        segment,
        messages,
        source_head: cursor.after,
        current_input,
        new_segment,
        reason: if previous.is_none() {
            "initial"
        } else if handoff_changed {
            "handoff"
        } else {
            "configuration"
        },
        context_prepare_us: micros(started),
        events_read: cursor.events_read - read_before,
        input_skills,
    };
    pending.context_cursor = Some(cursor);
    Ok(PreparedContext { request, commit })
}

pub(super) fn record_prepared(
    pending: &mut PendingRun,
    request_id: &str,
    prepared: &PreparedProviderRequest,
    mut context: ContextCommit,
    request_prepare_us: u64,
) -> Result<()> {
    let execution = pending
        .query
        .runtime_execution
        .expect("prepared context belongs to a session execution");
    let session = super::provider::runtime_session(pending, execution)?;
    let cursor = pending
        .context_cursor
        .as_mut()
        .expect("runtime context cursor exists");
    let mut writer = EventStore::open(&pending.events_path)?;
    let predecessor = cursor.segment.as_ref().map(|segment| segment.id);
    if context.new_segment {
        context.segment.id = writer.append(EventPayload::Unknown(json!({"kind":"runtime.context_segment","ts":now_timestamp()?,"agent":pending.conversation.as_str(),
            "signature":context.segment.signature,"predecessor":predecessor,"configuration":context.segment.configuration,"handoff":context.segment.handoff,"reason":context.reason,
            "connection":context.segment.connection,"protocol":context.segment.protocol,"model":context.segment.model,"endpoint":context.segment.endpoint,
            "system_prompt":context.segment.system_prompt,"prelude":context.segment.prelude,"tools":context.segment.tools,"catalog":context.segment.catalog,
            "behavior":context.segment.behavior,"identity_slot":context.segment.identity_slot,"handoff_prelude":context.segment.handoff_prelude,"workspace":context.segment.workspace})))?.id;
    }
    let frozen_head = if context.new_segment {
        0
    } else {
        cursor.admitted.len()
    };
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
        writer.append(EventPayload::Unknown(json!({"kind":"runtime.context_admitted","ts":now_timestamp()?,"agent":pending.conversation.as_str(),"segment":context.segment.id,
            "ordinal":ordinal,"turn":admitted.turn,"origin":admitted.origin,"source_head":context.source_head,"admitted_at":admitted.admitted_at,"message":admitted.message})))?;
    }
    let body_hash = format!("{:x}", Sha256::digest(serde_json::to_vec(&prepared.body)?));
    let payload = EventPayload::Unknown(
        json!({"kind":"runtime.step_prepared","ts":now_timestamp()?,"agent":pending.conversation.as_str(),"turn":pending.turn,"request_id":request_id,
        "segment":context.segment.id,"context_head":context.messages.len(),"current_input":context.current_input,"source_head":context.source_head,
        "connection":prepared.connection_name,"protocol":context.segment.protocol,"model":prepared.model,"url":prepared.url,"settings":prepared.settings,"correlation_id":prepared.correlation_id,
        "max_output_tokens":prepared.body.get("max_output_tokens").or_else(|| prepared.body.get("max_tokens")).or_else(|| prepared.body.get("max_completion_tokens")),"temperature":prepared.body.get("temperature"),
        "context_window_tokens":pending.resolved.as_ref().map(|runtime| runtime.config.max_context_tokens),"max_request_bytes":pending.query.runtime_limits.max_request_bytes,"request_bytes":serde_json::to_vec(&prepared.body)?.len(),
        "body_hash":body_hash,"context_prepare_us":context.context_prepare_us,"request_prepare_us":request_prepare_us,"context_events_read":context.events_read,
        "input_skills":context.input_skills}),
    );
    session.commit_model_facts(pending.conversation.as_str(), execution, [payload])?;
    cursor.commit(context.segment, context.messages, context.source_head)
}

fn initial_segment(
    pending: &PendingRun,
    catalog: &PromptCatalog,
    tools: &[crate::context::ToolSchema],
    behavior: &str,
) -> Result<FrozenSegment> {
    let project_instructions = load_project_instruction_sources(&pending.workspace)?;
    let (global_memory, project_memory) =
        load_memory_sources(&pending.kuku_home, &pending.workspace)?;
    let input = ContextInput {
        environment: EnvironmentSource {
            workspace_path: pending.workspace.display().to_string(),
            platform: platform_label().into(),
            current_date: current_date_string(),
        },
        project_instructions,
        global_memory,
        project_memory,
        history: Vec::new(),
        tools: tools.to_vec(),
        model_tiers: pending.config.tier_infos(),
        runtime_blocks: None,
        enable_memory: true,
        agent_name: pending.conversation.as_str().into(),
        agent_instructions: behavior.into(),
        response_contract: pending.query.response_contract.clone(),
    };
    let slot = usize::from(!render_project_policy(&input, catalog)?.is_empty());
    let mut assembly = assemble_context(input, catalog)?;
    if let Some(text) = pending
        .agent_registry
        .as_ref()
        .and_then(|registry| crate::agent::catalog::render_agent_catalog(registry, catalog))
    {
        assembly
            .prelude_messages
            .push(CanonicalMessage::user_text(text));
    }
    if let Some(text) = pending
        .skill_registry
        .as_ref()
        .and_then(|registry| crate::skill::catalog::render_skill_catalog(registry, &[], None))
    {
        assembly
            .prelude_messages
            .push(CanonicalMessage::user_text(text));
    }
    if let Some(skill) = &pending.bootstrap_skill {
        assembly
            .prelude_messages
            .push(CanonicalMessage::user_text(skill.body.clone()));
    }
    Ok(FrozenSegment {
        id: 0,
        signature: String::new(),
        configuration: 0,
        handoff: 0,
        system_prompt: assembly.system_prompt,
        prelude: assembly.prelude_messages,
        tools: tools.to_vec(),
        catalog: catalog.clone(),
        connection: String::new(),
        protocol: String::new(),
        model: String::new(),
        endpoint: String::new(),
        behavior: behavior.into(),
        identity_slot: slot,
        handoff_prelude: None,
        workspace: pending.workspace.to_string_lossy().into_owned(),
    })
}

fn signature(
    pending: &PendingRun,
    resolved: &ResolvedProvider,
    tools: &[crate::context::ToolSchema],
    behavior: &str,
    handoff: u64,
) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            resolved.kind.as_str(),
            &resolved.connection_name,
            &resolved.base_url,
            &resolved.model,
            &resolved.settings,
            resolved.max_context_tokens,
            resolved.max_output_tokens,
            pending.query.temperature,
            &pending.workspace,
            tools,
            behavior,
            handoff
        ))?)
    ))
}

fn replace_behavior(segment: &mut FrozenSegment, behavior: &str) -> Result<()> {
    if segment.behavior == behavior {
        return Ok(());
    }
    if !segment.behavior.is_empty() {
        if segment.prelude.get(segment.identity_slot)
            != Some(&CanonicalMessage::user_text(&segment.behavior))
        {
            return Err(Error::InvalidEventStream(
                "frozen agent instructions do not match their recorded prelude position".into(),
            ));
        }
        segment.prelude.remove(segment.identity_slot);
        if let Some(index) = &mut segment.handoff_prelude {
            if *index > segment.identity_slot {
                *index -= 1;
            }
        }
    }
    if !behavior.is_empty() {
        segment.prelude.insert(
            segment.identity_slot.min(segment.prelude.len()),
            CanonicalMessage::user_text(behavior),
        );
        if let Some(index) = &mut segment.handoff_prelude {
            if *index >= segment.identity_slot {
                *index += 1;
            }
        }
    }
    segment.behavior = behavior.into();
    Ok(())
}

fn select_turns(messages: &mut Vec<AdmittedMessage>, through: u64, keep: usize) {
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

fn append_input_skills(
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

fn append_dynamic(
    pending: &mut PendingRun,
    messages: &mut Vec<AdmittedMessage>,
    catalog: &PromptCatalog,
    input_tokens: Option<u32>,
    resolved: &ResolvedProvider,
    frozen_head: usize,
) -> Result<()> {
    let admitted_at = messages
        .iter()
        .rev()
        .find(|message| message.turn == pending.turn && message.origin == MessageOrigin::Input)
        .map_or(0, |message| message.admitted_at);
    for admitted in messages
        .iter_mut()
        .skip(frozen_head)
        .filter(|admitted| admitted.turn == pending.turn && admitted.origin == MessageOrigin::Input)
    {
        if let Some(prefix) = &pending.query.current_turn_prefix {
            admitted
                .message
                .blocks
                .insert(0, MessageBlock::Text(prefix.clone()));
        }
    }
    if !pending.hook_context.is_empty() {
        let template = catalog
            .blocks
            .get("hook-context")
            .ok_or_else(|| Error::PromptRender("missing frozen hook-context template".into()))?;
        let text = template.text.replace(
            "{{hook_text}}",
            &std::mem::take(&mut pending.hook_context).join("\n"),
        );
        messages.push(AdmittedMessage {
            turn: pending.turn,
            origin: MessageOrigin::Runtime,
            message: CanonicalMessage::user_text(text),
            admitted_at,
        });
    }
    let headroom = crate::notice::compute_context_headroom(
        resolved
            .max_context_tokens
            .saturating_sub(resolved.think_level.overhead_tokens()),
        Some(resolved.max_output_tokens),
        input_tokens,
    );
    let config = pending.config.handoff();
    if !pending.handoff_triggered
        && config.enabled
        && super::provider::should_trigger_handoff(&headroom, config.threshold)
    {
        pending.handoff_triggered = true;
        pending.handoff_keep_turns = config.keep_turns;
        let instruction = catalog.runtime.get("handoff-instruction").ok_or_else(|| {
            Error::PromptRender("missing frozen handoff-instruction template".into())
        })?;
        let prefix = super::provider::append_handoff_instruction(None, &instruction.text, catalog)
            .unwrap_or_default();
        messages.push(AdmittedMessage {
            turn: pending.turn,
            origin: MessageOrigin::Runtime,
            message: CanonicalMessage::user_text(prefix),
            admitted_at,
        });
    }
    Ok(())
}

fn micros(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}
