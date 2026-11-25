use std::time::Instant;

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::config::MediaAdapterSupport;
use crate::context::assembly::assemble_segment_context;
use crate::context::runtime::{
    AdmittedMessage, CheckpointFileChanges, FrozenSegment, MessageOrigin, RouteSelectors,
};
use crate::context::{
    CanonicalMessage, ContextAssembly, ContextInput, EnvironmentSource, MessageBlock,
};
use crate::event::{EventPayload, EventStore};
use crate::prompt::PromptCatalog;
use crate::provider::types::{
    PreparedProviderRequest, ProviderCapabilities, ProviderKind, ResolvedProvider,
};
use crate::{Error, Result};

use super::helpers::{
    current_date_string, load_memory_sources, load_project_instruction_sources, now_timestamp,
    platform_label,
};
use super::request::{OwnedRequestBase, estimated_message_tokens};
use super::types::PendingRun;

mod admission;
mod commit;
mod compaction;
mod skill_catalog;

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
    permission_mode_index: Option<usize>,
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

pub(super) async fn prepare(
    pending: &mut PendingRun,
    compact_only: bool,
) -> Result<PreparedContext> {
    let started = Instant::now();
    let resolved = pending
        .resolved
        .as_ref()
        .expect("resolved runtime exists")
        .config
        .clone();
    let mut tools = crate::tool::to_tool_schemas(
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
    };
    cursor.select_configuration(&route_selectors);
    let catalog = cursor
        .catalog
        .clone()
        .unwrap_or_else(|| pending.catalog.clone());
    let current_date = current_date_string();
    super::provider::render_tool_descriptions(&mut tools, &catalog)?;
    let behavior = super::provider::render_agent_behavior(pending, &tools, &catalog, &resolved)?;
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
    let route_changed = previous
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
        .transpose()?
        .unwrap_or(false);
    let new_segment = previous
        .as_ref()
        .is_none_or(|segment| segment.signature != current_signature);
    let handoff_changed = previous
        .as_ref()
        .map_or(cursor.handoff.is_some(), |segment| {
            segment.handoff != handoff
        });
    let workspace_changed = previous
        .as_ref()
        .is_some_and(|segment| segment.workspace != pending.workspace.to_string_lossy());
    let tool_names_changed = previous.as_ref().is_some_and(|segment| {
        !segment
            .tools
            .iter()
            .map(|tool| &tool.name)
            .eq(tools.iter().map(|tool| &tool.name))
    });
    let mut messages = cursor.history(&pending.conversation);
    let mut segment = match previous.as_ref() {
        Some(segment) => segment.clone(),
        None => initial_segment(pending, &catalog, &tools, &behavior, &resolved)?,
    };
    let missing_modalities = new_segment
        && previous.as_ref().is_some_and(|segment| {
            super::provider::model_modalities_slot(&segment.prelude).is_none()
        });
    if route_changed || workspace_changed || tool_names_changed || missing_modalities {
        segment = initial_segment(pending, &catalog, &tools, &behavior, &resolved)?;
        // The new prelude either starts its own catalog or does not expose Skill tools.
        // Notices tied to the previous prelude are no longer applicable in either case.
        messages.retain(|entry| {
            entry.origin != MessageOrigin::Runtime
                || crate::skill::model_catalog::anchor(&entry.message).is_none()
        });
        if let Some(summary) = previous.as_ref().and_then(|segment| {
            segment
                .handoff_prelude
                .and_then(|index| segment.prelude.get(index))
        }) {
            segment.handoff_prelude = Some(segment.prelude.len());
            segment.prelude.push(summary.clone());
        }
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
    let date_message =
        CanonicalMessage::user_text(crate::prompt::render_runtime_date(&catalog, &current_date)?);
    let date_already_admitted = messages.iter().any(|admitted| {
        admitted.origin == MessageOrigin::Runtime && admitted.message == date_message
    }) || custom_project_policy_has_date(&segment, &current_date);
    if new_segment {
        replace_behavior(&mut segment, &behavior)?;
        segment.signature = current_signature;
        segment.configuration = cursor.configuration.max(segment.configuration);
        segment.handoff = handoff;
        segment.connection = resolved.connection_name.clone();
        segment.protocol = resolved.kind.format().as_str().into();
        segment.model = resolved.model.clone();
        segment.endpoint = resolved.base_url.clone();
        segment.capabilities = Some(resolved.capabilities.clone());
        segment.route_selectors = Some(route_selectors.clone());
        super::provider::set_model_modalities(
            &mut segment.prelude,
            &tools,
            &resolved,
            pending.query.model_adapter.is_none(),
            &catalog,
        )?;
        segment.tools = tools;
        adapt_new_segment_tools(&resolved, &mut segment.tools);
        if handoff != 0 && (handoff_changed || tool_names_changed) {
            let event = if let Some(event) = &cursor.handoff {
                event.clone()
            } else {
                EventStore::read_page(&pending.events_path, handoff - 1, 1)?
                    .into_iter()
                    .next()
                    .filter(|event| event.id == handoff)
                    .ok_or_else(|| Error::InvalidEventStream("missing committed handoff".into()))?
            };
            let EventPayload::Handoff { summary, .. } = event.payload else {
                return Err(Error::InvalidEventStream(
                    "invalid committed handoff".into(),
                ));
            };
            let text = catalog.render_handoff(&summary, &segment.tools)?;
            if let Some(index) = segment.handoff_prelude {
                segment.prelude[index] = CanonicalMessage::user_text(text);
            } else {
                segment.handoff_prelude = Some(segment.prelude.len());
                segment.prelude.push(CanonicalMessage::user_text(text));
            }
        }
    }
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
    let permission_mode = append_permission_mode(
        pending,
        &mut messages,
        &segment.tools,
        &catalog,
        if new_segment {
            0
        } else {
            cursor.admitted.len()
        },
    )?;
    let mut current_input = if compact_only {
        messages
            .iter()
            .rposition(|entry| entry.origin == MessageOrigin::Input)
    } else if pending.query.skip_user_message {
        let source_turn = pending.query.resume_input_turn.unwrap_or(pending.turn);
        let input_is_present = messages.iter().any(|admitted| {
            admitted.origin == MessageOrigin::Input
                && (admitted.turn == pending.turn || admitted.turn == source_turn)
        });
        let mut source_sender = None;
        let mut source_images = Vec::new();
        let admitted_at = if input_is_present {
            0
        } else {
            let mut admitted_at = None;
            EventStore::visit_after_path(
                &pending.events_path,
                source_turn.saturating_sub(1),
                |event| {
                    if let EventPayload::MessageUser {
                        turn,
                        conversation,
                        text,
                        from,
                        images,
                        ..
                    } = event.payload
                    {
                        if admitted_at.is_none()
                            && turn == source_turn
                            && conversation == pending.conversation.as_str()
                            && text == pending.query.prompt
                        {
                            admitted_at = Some(event.id);
                            source_sender = from;
                            source_images = images;
                        }
                    }
                    Ok(())
                },
            )?;
            admitted_at.unwrap_or(cursor.after)
        };
        let prompt = source_sender
            .map(|from| crate::context::replay::render_agent_message(&pending.query.prompt, &from));
        let index = resumed_current_input(
            &mut messages,
            pending.turn,
            pending.query.resume_input_turn,
            prompt.as_deref().unwrap_or(&pending.query.prompt),
            admitted_at,
        );
        if let Some(index) = index {
            messages[index]
                .message
                .blocks
                .extend(source_images.into_iter().map(MessageBlock::InputImage));
        }
        index
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
    // A recovered input may only have been restored from the journal above. Its
    // original event ID is the rewind boundary for both kinds of Skill admission.
    let notice = skill_catalog::append_notice(
        pending,
        &segment,
        &mut messages,
        if new_segment {
            0
        } else {
            cursor.admitted.len()
        },
    )?;
    if notice.is_some_and(|index| index <= current_input) {
        current_input += 1;
    }
    let input_skills = append_input_skills(pending, &mut messages, &segment.prelude)?;
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
        permission_mode_index: messages.iter().rposition(|entry| {
            entry.origin == MessageOrigin::Runtime && entry.message == permission_mode
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

// Keep this out of the signature and provider serializer so an admitted segment
// reuses its original declaration, including when reopened by a newer binary.
fn adapt_new_segment_tools(
    resolved: &crate::provider::types::ResolvedProvider,
    tools: &mut [crate::context::ToolSchema],
) {
    use crate::provider::{prepare, types::ProviderKind};

    if resolved.kind != ProviderKind::OpenAiCompatible
        || !resolved.model.starts_with("google/gemini-")
        || !prepare::request_url(resolved)
            .ok()
            .and_then(|url| url::Url::parse(&url).ok())
            .is_some_and(|url| prepare::is_official_openrouter(&url))
    {
        return;
    }
    // OpenRouter's Gemini conversion turns a root oneOf into a required "input"
    // object. Keep the properties flat; the tool handlers still enforce exactly
    // one selector before accessing or creating any retained output.
    for tool in tools {
        if matches!(
            tool.name.as_str(),
            "read_output" | "search_output" | "save_value"
        ) {
            if let Some(schema) = tool.input_schema.as_object_mut() {
                schema.remove("oneOf");
            }
        }
    }
}

fn initial_segment(
    pending: &PendingRun,
    catalog: &PromptCatalog,
    tools: &[crate::context::ToolSchema],
    behavior: &str,
    resolved: &crate::provider::types::ResolvedProvider,
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
        models: Vec::new(),
        runtime_blocks: None,
        enable_memory: true,
        agent_instructions: behavior.into(),
        response_contract: pending.query.response_contract.clone(),
    };
    let agent = crate::context::assembly::agent_prompt_facts(
        Some(resolved),
        pending.query.model_adapter.is_none(),
    );
    let (mut assembly, slot) = assemble_segment_context(input, catalog, agent)?;
    if !pending.conversation.is_main() {
        assembly
            .prelude_messages
            .push(CanonicalMessage::user_text(format!(
                "<kuku_agent_identity>\nYour Session Agent address is {}.\n</kuku_agent_identity>",
                pending.conversation.as_str()
            )));
    }
    if let Some(text) = crate::skill::catalog::render_skill_discovery_hint(tools) {
        assembly
            .prelude_messages
            .push(CanonicalMessage::user_text(text));
        if let (Some(registry), Some(id)) = (&pending.skill_registry, &pending.skill_catalog_id) {
            assembly
                .prelude_messages
                .push(crate::skill::model_catalog::full(registry, id));
        }
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
        last_request_url: None,
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
            resolved.kind.format(),
            &resolved.connection_name,
            &resolved.base_url,
            &resolved.model,
            &resolved.capabilities,
            (
                resolved.output,
                resolved
                    .capabilities
                    .image_input_support(pending.query.model_adapter.is_none()),
            ),
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

fn check_route_snapshot(
    agent: &str,
    segment: &FrozenSegment,
    configured: u64,
    resolved: &ResolvedProvider,
    selectors: &RouteSelectors,
) -> Result<bool> {
    let connection_changed = segment.connection != resolved.connection_name;
    let protocol_changed = segment.protocol != resolved.kind.format().as_str();
    let model_changed = segment.model != resolved.model;
    let endpoint_changed = segment.endpoint != resolved.base_url;
    let effective_route_changed =
        connection_changed || protocol_changed || model_changed || endpoint_changed;
    let (capabilities, recorded_selectors) = match (
        segment.capabilities.as_ref(),
        segment.route_selectors.as_ref(),
    ) {
        (Some(capabilities), Some(recorded_selectors)) => (capabilities, recorded_selectors),
        _ => {
            return Err(Error::MissingProviderConfig(format!(
                "agent '{agent}' has no complete provider route snapshot; start a new Session"
            )));
        }
    };
    let selectors_changed = recorded_selectors != selectors;
    let route_changed = effective_route_changed || selectors_changed;
    let capabilities_changed = capabilities != &resolved.capabilities;
    let media_upgraded = !route_changed
        && builtin_media_adapter_upgraded(capabilities, resolved)
        && segment.last_request_url.as_ref().map_or_else(
            || resolved.options.endpoint_url.is_none(),
            |url| {
                crate::provider::prepare::request_url(resolved).is_ok_and(|current| &current == url)
            },
        );
    let explicit_route_change = configured > segment.configuration && selectors_changed;
    if (route_changed || (capabilities_changed && !media_upgraded)) && !explicit_route_change {
        return Err(Error::MissingProviderConfig(format!(
            "agent '{agent}' has a frozen provider route or capabilities that differ from the active configuration; restore the recorded route/profile, explicitly configure a different connection and model pair with configure_agent, or start a new Session"
        )));
    }
    Ok(route_changed)
}

fn builtin_media_adapter_upgraded(old: &ProviderCapabilities, resolved: &ResolvedProvider) -> bool {
    let adapter = resolved.capabilities.media_adapter;
    if old.media_adapter != MediaAdapterSupport::TextOnly {
        return false;
    }
    if resolved.kind == ProviderKind::OpenAiResponses
        && adapter == MediaAdapterSupport::ResponsesImageInput
    {
        let other_modalities = |capabilities: &ProviderCapabilities| {
            capabilities.input_modalities.as_ref().map_or_else(
                || vec!["text".to_owned()],
                |input| {
                    input
                        .value
                        .iter()
                        .filter(|value| *value != "image")
                        .cloned()
                        .collect()
                },
            )
        };
        return other_modalities(old) == other_modalities(&resolved.capabilities)
            && (ProviderCapabilities {
                media_adapter: adapter,
                input_modalities: resolved.capabilities.input_modalities.clone(),
                ..old.clone()
            }) == resolved.capabilities;
    }
    if !matches!(
        (resolved.kind, adapter),
        (
            ProviderKind::OpenAiCompatible,
            MediaAdapterSupport::ChatCompletionsImageParts
        ) | (
            ProviderKind::Anthropic,
            MediaAdapterSupport::AnthropicMessagesImageBlocks
        )
    ) {
        return false;
    }
    // The new request encoder is not a route change. All declared capabilities
    // still have to match so the successor segment can retain this history.
    ProviderCapabilities {
        media_adapter: adapter,
        ..old.clone()
    } == resolved.capabilities
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
    if let Some(date_message) = date_message {
        insert_runtime_notice(
            messages,
            pending.turn,
            date_message,
            frozen_head,
            admitted_at,
        );
    }
    if !pending.hook_context.is_empty() {
        let hook_text = std::mem::take(&mut pending.hook_context).join("\n");
        let text = catalog.render("blocks/hook-context.md", minijinja::context! { hook_text })?;
        messages.push(AdmittedMessage {
            turn: pending.turn,
            origin: MessageOrigin::Runtime,
            message: CanonicalMessage::user_text(text),
            admitted_at,
        });
    }
    Ok(())
}

fn append_permission_mode(
    pending: &PendingRun,
    messages: &mut Vec<AdmittedMessage>,
    tools: &[crate::context::ToolSchema],
    catalog: &PromptCatalog,
    frozen_head: usize,
) -> Result<CanonicalMessage> {
    use crate::permission::PermissionMode;

    const TAG: &str = "<kuku_permission_mode>";
    let permission_mode = super::runtime_tools::effective_permission_mode(pending)?;
    let mode = match permission_mode {
        PermissionMode::Full => "Full",
        PermissionMode::Ask => "Ask",
        PermissionMode::ReadOnly => "Read-only",
    };
    let tools: Vec<_> = tools.iter().map(|tool| tool.name.as_str()).collect();
    let text = catalog.renderer().render("runtime/permission-mode.md", minijinja::context! {
        step => minijinja::context! { permission_mode => minijinja::value::Serde(permission_mode) },
        tools => minijinja::value::Serde(&tools),
        mode,
    })?;
    let message = CanonicalMessage::user_text(text);
    let last = messages.iter().rev().find(|entry| {
        entry.origin == MessageOrigin::Runtime
            && entry
                .message
                .blocks
                .iter()
                .any(|block| matches!(block, MessageBlock::Text(text) if text.starts_with(TAG)))
    });
    if last.is_none_or(|entry| entry.message != message) {
        let admitted_at = messages.last().map_or(0, |entry| entry.admitted_at);
        insert_runtime_notice(messages, pending.turn, &message, frozen_head, admitted_at);
    }
    Ok(message)
}

fn insert_runtime_notice(
    messages: &mut Vec<AdmittedMessage>,
    turn: u64,
    message: &CanonicalMessage,
    frozen_head: usize,
    admitted_at: u64,
) -> usize {
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
            message: message.clone(),
            admitted_at,
        },
    );
    insertion_index
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

fn custom_project_policy_has_date(segment: &FrozenSegment, current_date: &str) -> bool {
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
            protocol: resolved.kind.format().as_str().into(),
            model: resolved.model.clone(),
            endpoint: resolved.base_url.clone(),
            last_request_url: None,
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
    fn new_openrouter_gemini_segments_keep_retained_output_selectors_flat() {
        let (mut frozen, mut route, _) = snapshot_fixture();
        let registry = super::super::runtime_tools::definitions();
        frozen.tools = crate::tool::to_tool_schemas(&registry);
        let frozen_bytes = serde_json::to_vec(&frozen.tools).unwrap();
        let original_prefix = segment_prefix_hash(&frozen).unwrap();

        route.model = "google/gemini-4-neon".into();
        let mut declarations = frozen.tools.clone();
        adapt_new_segment_tools(&route, &mut declarations);
        assert_eq!(
            frozen.tools, declarations,
            "other gateways keep their schemas"
        );

        route.base_url = "https://openrouter.ai/api/v1".into();
        adapt_new_segment_tools(&route, &mut declarations);
        for (before, after) in frozen.tools.iter().zip(&declarations) {
            assert_eq!(before.name, after.name);
            if matches!(
                before.name.as_str(),
                "read_output" | "search_output" | "save_value"
            ) {
                let mut expected = before.input_schema.clone();
                assert!(expected.as_object_mut().unwrap().remove("oneOf").is_some());
                assert_eq!(expected, after.input_schema);
            } else {
                assert_eq!(before, after);
            }
        }
        assert_eq!(frozen_bytes, serde_json::to_vec(&frozen.tools).unwrap());
        assert_eq!(original_prefix, segment_prefix_hash(&frozen).unwrap());
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
    fn builtin_image_encoders_upgrade_without_a_route_change() {
        for (kind, adapter) in [
            (
                ProviderKind::OpenAiCompatible,
                MediaAdapterSupport::ChatCompletionsImageParts,
            ),
            (
                ProviderKind::Anthropic,
                MediaAdapterSupport::AnthropicMessagesImageBlocks,
            ),
        ] {
            let (mut segment, mut route, selectors) = snapshot_fixture();
            route.kind = kind;
            route.options.endpoint_url = Some("https://example.test/v2/generate".into());
            segment.protocol = kind.format().as_str().into();
            segment
                .last_request_url
                .clone_from(&route.options.endpoint_url);
            route.capabilities.media_adapter = adapter;
            assert!(!check_route_snapshot("main", &segment, 5, &route, &selectors).unwrap());

            route.capabilities.tool_calls = Some(false);
            assert!(check_route_snapshot("main", &segment, 5, &route, &selectors).is_err());
            route.capabilities.tool_calls = None;
            route.options.endpoint_url = Some("https://example.test/v2/other".into());
            assert!(check_route_snapshot("main", &segment, 5, &route, &selectors).is_err());
            route.options.endpoint_url = None;
            assert!(check_route_snapshot("main", &segment, 5, &route, &selectors).is_err());
            route
                .options
                .endpoint_url
                .clone_from(&segment.last_request_url);
            route.base_url = "https://other.example.test".into();
            assert!(check_route_snapshot("main", &segment, 5, &route, &selectors).is_err());
            route.base_url.clone_from(&segment.endpoint);

            segment.capabilities = Some(route.capabilities.clone());
            route.capabilities.media_adapter = MediaAdapterSupport::TextOnly;
            assert!(check_route_snapshot("main", &segment, 5, &route, &selectors).is_err());
        }
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
        };
        assert!(
            check_route_snapshot(
                "main",
                &segment,
                segment.configuration + 1,
                &resolved,
                &selectors,
            )
            .unwrap()
        );
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
    fn provider_route_snapshot_requires_capabilities_and_selectors() {
        let (mut segment, route, selectors) = snapshot_fixture();
        segment.capabilities = None;
        segment.route_selectors = None;
        let error = check_route_snapshot("main", &segment, 0, &route, &selectors)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no complete provider route snapshot"));
    }
}
