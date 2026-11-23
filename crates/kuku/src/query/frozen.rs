use std::path::PathBuf;
use std::time::Instant;

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::context::assembly::assemble_segment_context;
use crate::context::runtime::{
    AdmittedMessage, CheckpointFileChanges, FrozenSegment, MessageOrigin, RouteSelectors,
};
use crate::context::{
    CanonicalMessage, ContextAssembly, ContextInput, EnvironmentSource, MessageBlock, Role,
};
use crate::event::{EventPayload, EventStore};
use crate::prompt::PromptCatalog;
use crate::provider::types::{PreparedProviderRequest, ResolvedProvider};
use crate::{Error, Result};

use super::helpers::{
    current_date_string, load_memory_sources, load_project_instruction_sources, now_timestamp,
    platform_label,
};
use super::request::{estimated_message_tokens, OwnedRequestBase};
use super::types::PendingRun;

mod admission;
mod commit;
mod compaction;

use admission::{append_input_skills, model_history, select_turns};
use commit::commit_context;

pub(super) struct PreparedContext {
    pub(super) request: OwnedRequestBase,
    pub(super) commit: ContextCommit,
}

#[derive(Clone)]
pub(super) struct ContextCommit {
    segment: FrozenSegment,
    messages: Vec<AdmittedMessage>,
    source_head: u64,
    current_input: usize,
    runtime_date_index: Option<usize>,
    prefix_context_head: usize,
    new_segment: bool,
    reason: &'static str,
    checkpointed_messages: usize,
    checkpoint_file_delta: CheckpointFileChanges,
    pub(in crate::query) estimated_input_tokens: u32,
    context_prepare_us: u64,
    events_read: u64,
    input_skills: Vec<String>,
}

pub(super) fn prepare(pending: &mut PendingRun, compact_only: bool) -> Result<PreparedContext> {
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
        pending.query.runtime_limits.context_replay_bytes(),
    )?;
    let route_selectors = RouteSelectors {
        query_connection: pending.query.connection.clone(),
        query_model: pending.query.model.clone(),
        query_tier: pending.query.tier.clone(),
    };
    cursor.select_configuration(&route_selectors);
    let catalog = cursor
        .catalog
        .clone()
        .unwrap_or_else(|| pending.catalog.clone());
    let current_date = current_date_string();
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
    let current_signature = signature(
        pending,
        &resolved,
        &route_selectors,
        &tools,
        &behavior,
        handoff,
    )?;
    let previous = cursor.segment.clone();
    let snapshot = previous
        .as_ref()
        .map(|segment| {
            check_route_snapshot(
                pending.conversation.as_str(),
                segment,
                cursor.configured,
                &resolved,
                &route_selectors,
            )
        })
        .transpose()?;
    let (legacy_segment, route_changed) = match snapshot {
        Some(RouteSnapshot::Legacy) => (true, false),
        Some(RouteSnapshot::Current { route_changed }) => (false, route_changed),
        None => (false, false),
    };
    let legacy_signature_for_comparison = if legacy_segment {
        Some(legacy_signature(
            pending, &resolved, &tools, &behavior, handoff,
        )?)
    } else {
        None
    };
    let signature_for_comparison = legacy_signature_for_comparison
        .as_deref()
        .unwrap_or(&current_signature);
    let new_segment = previous
        .as_ref()
        .is_none_or(|segment| segment.signature.as_str() != signature_for_comparison);
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
    if legacy_segment {
        reject_ambiguous_legacy_replay(pending.conversation.as_str(), &messages)?;
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
    let date_message =
        CanonicalMessage::user_text(crate::prompt::render_runtime_date(&catalog, &current_date)?);
    let date_already_admitted = messages.iter().any(|admitted| {
        admitted.origin == MessageOrigin::Runtime && admitted.message == date_message
    }) || legacy_project_policy_has_date(&segment, &current_date);
    if new_segment {
        replace_behavior(&mut segment, &behavior)?;
        segment.signature = current_signature;
        segment.configuration = cursor.configuration.max(segment.configuration);
        segment.handoff = handoff;
        segment.connection = resolved.connection_name.clone();
        segment.protocol = resolved.kind.as_str().into();
        segment.model = resolved.model.clone();
        segment.endpoint = resolved.base_url.clone();
        segment.capabilities = Some(resolved.capabilities.clone());
        segment.route_selectors = Some(route_selectors.clone());
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
        (!date_already_admitted).then_some(&date_message),
        if new_segment {
            0
        } else {
            cursor.admitted.len()
        },
    )?;
    let current_input = if compact_only {
        messages
            .iter()
            .rposition(|entry| entry.origin == MessageOrigin::Input)
    } else if pending.query.skip_user_message {
        let source_turn = pending.query.resume_input_turn.unwrap_or(pending.turn);
        let input_is_present = messages.iter().any(|admitted| {
            admitted.origin == MessageOrigin::Input
                && (admitted.turn == pending.turn || admitted.turn == source_turn)
        });
        let admitted_at = if input_is_present {
            0
        } else {
            let mut admitted_at = None;
            EventStore::visit_after_path(
                &pending.events_path,
                source_turn.saturating_sub(1),
                |event| {
                    if admitted_at.is_none()
                        && matches!(&event.payload, EventPayload::MessageUser { turn, conversation, text, .. }
                            if *turn == source_turn && conversation == pending.conversation.as_str()
                                && text == &pending.query.prompt)
                    {
                        admitted_at = Some(event.id);
                    }
                    Ok(())
                },
            )?;
            admitted_at.unwrap_or(cursor.after)
        };
        resumed_current_input(
            &mut messages,
            pending.turn,
            pending.query.resume_input_turn,
            &pending.query.prompt,
            admitted_at,
        )
    } else {
        messages.iter().rposition(|admitted| {
            admitted.turn == pending.turn && admitted.origin == MessageOrigin::Input
        })
    }
    .ok_or_else(|| {
        if compact_only {
            Error::InvalidArgument("there is no remaining conversation history to compact".into())
        } else {
            Error::InvalidEventStream("prepared context has no admitted current input".into())
        }
    })?;
    let assembly = ContextAssembly {
        system_prompt: segment.system_prompt.clone(),
        prelude_messages: segment.prelude.clone(),
        history: model_history(&messages, pending.turn),
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
    let measured = cursor.measured_input.filter(|_| !new_segment);
    let estimated_input_tokens = measured
        .and_then(|(head, tokens)| {
            messages.get(head..).map(|suffix| {
                tokens.saturating_add(estimated_message_tokens(
                    suffix.iter().map(|entry| &entry.message),
                ))
            })
        })
        .unwrap_or_else(|| request.estimated_input_tokens());
    let commit = ContextCommit {
        segment,
        runtime_date_index: messages.iter().rposition(|entry| {
            entry.origin == MessageOrigin::Runtime && entry.message == date_message
        }),
        messages,
        source_head: cursor.after,
        current_input,
        prefix_context_head: if new_segment {
            0
        } else {
            cursor.admitted.len()
        },
        new_segment,
        checkpointed_messages: 0,
        checkpoint_file_delta: CheckpointFileChanges::default(),
        estimated_input_tokens,
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
    context: ContextCommit,
    request_prepare_us: u64,
) -> Result<()> {
    let body = prepared.body.as_json();
    let wire = prepared.body.as_bytes();
    let body_hash = format!("{:x}", Sha256::digest(wire));
    let prefix_hash = segment_prefix_hash(&context.segment)?;
    let history_prefix_hash = pending
        .context_cursor
        .as_ref()
        .expect("runtime context cursor exists")
        .history_prefix_hash(context.prefix_context_head);
    let payload = json!({"kind":"runtime.step_prepared","ts":now_timestamp()?,"agent":pending.conversation.as_str(),"turn":pending.turn,"request_id":request_id,
        "segment":context.segment.id,"context_head":context.messages.len(),"current_input":context.current_input,"source_head":context.source_head,
        "connection":prepared.connection_name,"protocol":context.segment.protocol,"model":prepared.model,"url":prepared.url,"settings":prepared.settings,"correlation_id":prepared.correlation_id,
        "max_output_tokens":body.get("max_output_tokens").or_else(|| body.get("max_tokens")).or_else(|| body.get("max_completion_tokens")),"temperature":body.get("temperature"),
        "context_window_tokens":pending.resolved.as_ref().map(|runtime| runtime.config.max_context_tokens),"max_request_bytes":pending.query.runtime_limits.max_request_bytes,"request_bytes":wire.len(),
        "body_hash":body_hash,"prefix_context_head":context.prefix_context_head,"prefix_hash":prefix_hash,"history_prefix_hash":history_prefix_hash,
        "context_prepare_us":context.context_prepare_us,"request_prepare_us":request_prepare_us,"context_events_read":context.events_read,
        "input_skills":context.input_skills});
    commit_context(pending, context, payload, false)?;
    Ok(())
}

fn initial_segment(
    pending: &PendingRun,
    catalog: &PromptCatalog,
    tools: &[crate::context::ToolSchema],
    behavior: &str,
) -> Result<FrozenSegment> {
    let max_bytes = pending.query.runtime_limits.workspace_io.context_file_bytes;
    let project_instructions = load_project_instruction_sources(&pending.workspace, max_bytes)?;
    let (global_memory, project_memory) =
        load_memory_sources(&pending.kuku_home, &pending.workspace, max_bytes)?;
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
        model_tiers: super::provider::usable_model_routes(&pending.config),
        runtime_blocks: None,
        enable_memory: true,
        agent_name: pending.conversation.as_str().into(),
        agent_instructions: behavior.into(),
        response_contract: pending.query.response_contract.clone(),
    };
    let (mut assembly, slot) = assemble_segment_context(input, catalog)?;
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
        .and_then(crate::skill::catalog::render_static_skill_catalog)
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
        capabilities: None,
        route_selectors: None,
        behavior: behavior.into(),
        project_instruction_sources: assembly
            .project_instruction_sources
            .into_iter()
            .map(|source| crate::context::FileSource {
                path: source.path,
                hash: source.hash,
            })
            .collect(),
        identity_slot: slot,
        handoff_prelude: None,
        checkpoint_files: Default::default(),
        checkpoint_files_omitted: false,
        workspace: pending.workspace.to_string_lossy().into_owned(),
    })
}

fn signature(
    pending: &PendingRun,
    resolved: &ResolvedProvider,
    route_selectors: &RouteSelectors,
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
            &resolved.capabilities,
            route_selectors,
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

fn legacy_signature(
    pending: &PendingRun,
    resolved: &ResolvedProvider,
    tools: &[crate::context::ToolSchema],
    behavior: &str,
    handoff: u64,
) -> Result<String> {
    let serialized = legacy_signature_bytes((
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
        handoff,
    ))?;
    Ok(format!("{:x}", Sha256::digest(serialized)))
}

type LegacySignatureParts<'a> = (
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    &'a crate::provider::types::ProviderSettings,
    u32,
    Option<u32>,
    Option<f32>,
    &'a PathBuf,
    &'a [crate::context::ToolSchema],
    &'a str,
    u64,
);

fn legacy_signature_bytes(parts: LegacySignatureParts<'_>) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&parts)?)
}

fn reject_ambiguous_legacy_replay(agent: &str, messages: &[AdmittedMessage]) -> Result<()> {
    if messages.iter().any(|admitted| {
        admitted.message.role == Role::Assistant
            && admitted
                .message
                .blocks
                .iter()
                .any(|block| matches!(block, MessageBlock::Thinking(_)))
            && !admitted
                .message
                .blocks
                .iter()
                .any(|block| matches!(block, MessageBlock::ProviderReplay { .. }))
    }) {
        return Err(Error::MissingProviderConfig(format!(
            "agent '{agent}' has legacy assistant thinking without a durable provider replay decision; refusing ambiguous continuation under the active provider profile. Start a new Session with fresh context."
        )));
    }
    Ok(())
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

#[derive(Debug)]
enum RouteSnapshot {
    Legacy,
    Current { route_changed: bool },
}

fn check_route_snapshot(
    agent: &str,
    segment: &FrozenSegment,
    configured: u64,
    resolved: &ResolvedProvider,
    selectors: &RouteSelectors,
) -> Result<RouteSnapshot> {
    let connection_changed = segment.connection != resolved.connection_name;
    let protocol_changed = segment.protocol != resolved.kind.as_str();
    let model_changed = segment.model != resolved.model;
    let endpoint_changed = segment.endpoint != resolved.base_url;
    let effective_route_changed =
        connection_changed || protocol_changed || model_changed || endpoint_changed;
    let (capabilities, recorded_selectors) = match (
        segment.capabilities.as_ref(),
        segment.route_selectors.as_ref(),
    ) {
        (None, None) => {
            if effective_route_changed {
                return Err(Error::MissingProviderConfig(format!(
                    "agent '{agent}' has a legacy context segment whose recorded effective connection, protocol, model, or endpoint differs from the active route; restore the recorded route or start a new Session"
                )));
            }
            return Ok(RouteSnapshot::Legacy);
        }
        (Some(capabilities), Some(recorded_selectors)) => (capabilities, recorded_selectors),
        _ => {
            return Err(Error::MissingProviderConfig(format!(
                "agent '{agent}' has an inconsistent partial provider route snapshot; restore the recorded configuration or start a new Session"
            )));
        }
    };
    let selectors_changed = recorded_selectors != selectors;
    let route_changed = effective_route_changed || selectors_changed;
    let capabilities_changed = capabilities != &resolved.capabilities;
    let explicit_route_change = configured > segment.configuration && selectors_changed;
    if (route_changed || capabilities_changed) && !explicit_route_change {
        return Err(Error::MissingProviderConfig(format!(
            "agent '{agent}' has a frozen provider route or capabilities that differ from the active configuration; restore the recorded route/profile, explicitly configure a different connection, model, or tier with configure_agent, or start a new Session"
        )));
    }
    Ok(RouteSnapshot::Current { route_changed })
}

fn append_dynamic(
    pending: &mut PendingRun,
    messages: &mut Vec<AdmittedMessage>,
    catalog: &PromptCatalog,
    date_message: Option<&CanonicalMessage>,
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
    if let Some(date_message) = date_message {
        insert_runtime_date(
            messages,
            pending.turn,
            date_message,
            frozen_head,
            admitted_at,
        );
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
    Ok(())
}

fn insert_runtime_date(
    messages: &mut Vec<AdmittedMessage>,
    turn: u64,
    date_message: &CanonicalMessage,
    frozen_head: usize,
    admitted_at: u64,
) {
    let current_input = messages
        .iter()
        .position(|admitted| admitted.turn == turn && admitted.origin == MessageOrigin::Input)
        .unwrap_or(messages.len());
    let insertion_index = if current_input < frozen_head {
        messages.len()
    } else {
        current_input
    };
    messages.insert(
        insertion_index,
        AdmittedMessage {
            turn,
            origin: MessageOrigin::Runtime,
            message: date_message.clone(),
            admitted_at,
        },
    );
}

fn resumed_current_input(
    messages: &mut Vec<AdmittedMessage>,
    current_turn: u64,
    source_turn: Option<u64>,
    prompt: &str,
    admitted_at: u64,
) -> Option<usize> {
    if let Some(index) = messages.iter().rposition(|admitted| {
        admitted.turn == current_turn && admitted.origin == MessageOrigin::Input
    }) {
        return Some(index);
    }
    if let Some(source_turn) = source_turn {
        if let Some(index) = messages.iter().rposition(|admitted| {
            admitted.turn == source_turn && admitted.origin == MessageOrigin::Input
        }) {
            return Some(index);
        }
    }
    let turn = source_turn.unwrap_or(current_turn);
    messages.push(AdmittedMessage {
        turn,
        origin: MessageOrigin::Input,
        message: CanonicalMessage::user_text(prompt.to_owned()),
        admitted_at,
    });
    Some(messages.len() - 1)
}

fn legacy_project_policy_has_date(segment: &FrozenSegment, current_date: &str) -> bool {
    segment.catalog.blocks.get("project-policy").is_some_and(|template| {
        segment.prelude.iter().any(|message| {
            message.blocks.iter().any(|block| {
                matches!(block, MessageBlock::Text(text) if crate::prompt::render::project_policy_has_date(text, &template.text, current_date))
            })
        })
    })
}

fn content_hash(value: &impl serde::Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}

fn segment_prefix_hash(segment: &FrozenSegment) -> Result<String> {
    content_hash(&(&segment.system_prompt, &segment.tools, &segment.prelude))
}

fn micros(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}
#[cfg(test)]
#[path = "frozen/date_tests.rs"]
mod date_tests;
#[cfg(test)]
#[path = "frozen/resume_tests.rs"]
mod resume_tests;
#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::config::ProviderProfile;
    use crate::provider::types::ProviderKind;

    fn snapshot_fixture() -> (FrozenSegment, ResolvedProvider, RouteSelectors) {
        let resolved = ResolvedProvider::default();
        let selectors = RouteSelectors {
            query_connection: Some("default".into()),
            query_model: Some("test-model".into()),
            query_tier: Some("balanced".into()),
        };
        let segment = FrozenSegment {
            id: 1,
            signature: "fixture".into(),
            configuration: 5,
            handoff: 0,
            system_prompt: String::new(),
            prelude: Vec::new(),
            tools: Vec::new(),
            catalog: crate::prompt::builtin_prompt_catalog(),
            connection: resolved.connection_name.clone(),
            protocol: resolved.kind.as_str().into(),
            model: resolved.model.clone(),
            endpoint: resolved.base_url.clone(),
            capabilities: Some(resolved.capabilities.clone()),
            route_selectors: Some(selectors.clone()),
            behavior: String::new(),
            identity_slot: 1,
            project_instruction_sources: Vec::new(),
            handoff_prelude: None,
            checkpoint_files: Default::default(),
            checkpoint_files_omitted: false,
            workspace: String::new(),
        };
        (segment, resolved, selectors)
    }

    #[test]
    fn capability_drift_requires_a_changed_route_selector() {
        let (segment, mut resolved, selectors) = snapshot_fixture();
        resolved.capabilities.profile = ProviderProfile::Anthropic;
        let error = check_route_snapshot("main", &segment, 6, &resolved, &selectors)
            .unwrap_err()
            .to_string();
        assert!(error.contains("restore the recorded route/profile"));
    }

    #[test]
    fn a_new_configured_route_selector_authorizes_projection() {
        let (segment, mut resolved, _) = snapshot_fixture();
        resolved.connection_name = "native".into();
        resolved.kind = ProviderKind::Anthropic;
        resolved.capabilities.profile = ProviderProfile::Anthropic;
        let selectors = RouteSelectors {
            query_connection: Some("native".into()),
            query_model: Some("test-model".into()),
            query_tier: Some("balanced".into()),
        };
        assert!(matches!(
            check_route_snapshot(
                "main",
                &segment,
                segment.configuration + 1,
                &resolved,
                &selectors,
            )
            .unwrap(),
            RouteSnapshot::Current {
                route_changed: true
            }
        ));
    }

    #[test]
    fn a_configured_boundary_without_route_change_does_not_authorize_drift() {
        let (segment, mut resolved, selectors) = snapshot_fixture();
        resolved.capabilities.profile = ProviderProfile::Anthropic;
        let error = check_route_snapshot("main", &segment, 6, &resolved, &selectors)
            .unwrap_err()
            .to_string();
        assert!(error.contains("configure_agent"));
    }

    #[test]
    fn legacy_segment_matches_only_the_recorded_effective_route() {
        let (mut segment, mut resolved, selectors) = snapshot_fixture();
        segment.capabilities = None;
        segment.route_selectors = None;
        assert!(matches!(
            check_route_snapshot("main", &segment, 0, &resolved, &selectors).unwrap(),
            RouteSnapshot::Legacy
        ));

        resolved.base_url = "https://changed.example".into();
        let error = check_route_snapshot("main", &segment, 0, &resolved, &selectors)
            .unwrap_err()
            .to_string();
        assert!(error.contains("restore the recorded route"));
    }

    #[test]
    fn partial_legacy_route_snapshot_is_rejected() {
        let (mut segment, resolved, selectors) = snapshot_fixture();
        segment.capabilities = None;
        let error = check_route_snapshot("main", &segment, 0, &resolved, &selectors)
            .unwrap_err()
            .to_string();
        assert!(error.contains("inconsistent partial provider route snapshot"));
    }

    #[test]
    fn legacy_signature_uses_the_exact_pre_snapshot_tuple() {
        let settings = crate::provider::types::ProviderSettings::default();
        let workspace = std::path::PathBuf::from("workspace");
        let tools = Vec::<crate::context::ToolSchema>::new();
        let bytes = legacy_signature_bytes((
            "anthropic",
            "conn",
            "https://endpoint",
            "model",
            &settings,
            100_000,
            Some(2048),
            Some(0.5),
            &workspace,
            &tools,
            "behavior",
            7,
        ))
        .unwrap();
        assert_eq!(
            br#"["anthropic","conn","https://endpoint","model",{"requested_reasoning_effort":null,"reasoning_effort":null,"requested_service_tier":null,"service_tier":null},100000,2048,0.5,"workspace",[],"behavior",7]"#,
            bytes.as_slice()
        );
    }
}
