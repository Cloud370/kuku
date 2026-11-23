mod compaction;
mod dispatch;
mod resolve;
pub(super) use resolve::usable_model_routes;
mod retry;

use dispatch::dispatch_provider_step;

pub(super) use resolve::ensure_resolved;
pub(super) use retry::{
    record_retry_event, record_terminal_stop, RetryEvent, RetryPhase, RetryStopReason,
};

use crate::context::{
    assemble_context, rebuild_history_for_provider, restore_prompt_snapshot,
    AgentRegistryProvenance, CanonicalMessage, ContextInput, EnvironmentSource, MessageBlock,
    PluginRegistryProvenance, PromptCapabilityMetadata, PromptRendererIdentity, Role,
    SkillRegistryProvenance, ToolRegistryProvenance,
};
use crate::error::Result;
use crate::event::{EventPayload, EventStore};
use crate::log::{LogLevel, LogRecord, LogScope};
use crate::notice::{
    build_runtime_notices, compute_context_headroom, render_notice_body, types::ContextHeadroom,
    NoticeAssemblyInput,
};
use crate::prompt::{builtin_handoff_instruction, load_prompt_template};
use crate::tool;

use super::helpers::{
    append_model_error, append_turn_interrupted, current_date_string, last_input_tokens,
    load_memory_sources, load_project_instruction_sources, now_timestamp, platform_label,
};
use super::request::OwnedRequestBase;
use super::types::{PendingRun, PendingStep, UiEvent};

pub(super) async fn call_provider_step(mut pending: PendingRun) -> Result<PendingStep> {
    pending.request_num += 1;

    apply_runtime_boundary(&mut pending)?;
    ensure_resolved(&mut pending)?;
    let resolved = pending.resolved.as_ref().expect("resolved runtime exists");
    let resolved_config = resolved.config.clone();
    let registry = resolved.registry.clone();
    if pending.query.runtime.is_some() {
        let context = super::frozen::prepare(&mut pending, false)?;
        pending.request_base = Some(context.request);
        return dispatch_provider_step(pending, resolved_config, Vec::new(), Some(context.commit))
            .await;
    }
    let existing_events = EventStore::replay(&pending.events_path)?;
    let (handoff_summary, history) =
        rebuild_history_for_provider(&existing_events, &pending.conversation);
    let max_bytes = pending.query.runtime_limits.workspace_io.context_file_bytes;
    let project_instructions = load_project_instruction_sources(&pending.workspace, max_bytes)?;
    let (global_memory, project_memory) =
        load_memory_sources(&pending.kuku_home, &pending.workspace, max_bytes)?;
    let platform = platform_label().to_string();
    let current_date = current_date_string();
    let model_tiers = resolve::usable_model_routes(&pending.config);

    let catalog = if let Some(dir) = &pending.prompts_dir {
        crate::prompt::PromptCatalog::load_from_dir(dir).map_err(|e| {
            crate::error::Error::PromptRender(format!(
                "failed to load prompts from {}: {e}",
                dir.display()
            ))
        })?
    } else {
        crate::prompt::builtin_prompt_catalog()
    };

    let (catalog_text, skills_text, runtime_blocks) = build_runtime_blocks(
        &pending.workspace,
        pending.conversation.as_str(),
        pending.turn,
        pending.agent_registry.as_ref(),
        pending.skill_registry.as_ref(),
        pending.previous_skill_registry.as_ref(),
        &resolved_config,
        &existing_events,
        &catalog,
    )?;

    let runtime_blocks = if pending.turn == 1 {
        if let Some(ref plugin_reg) = pending.plugin_registry {
            if !plugin_reg.is_empty() {
                let pkg_names = plugin_reg.names().join(", ");
                let notice = format!(
                    "Plugins loaded: {pkg_names}. \
                     If not relevant to your current task, ignore."
                );
                let wrapper_tmpl = catalog
                    .blocks
                    .get("system-notice")
                    .map(|a| a.text.as_str())
                    .unwrap_or("<kuku_system_notice>\n{{notice_body}}\n</kuku_system_notice>");
                let wrapped = wrapper_tmpl.replace("{{notice_body}}", &notice);
                Some(match runtime_blocks {
                    Some(existing) => format!("{existing}\n\n{wrapped}"),
                    None => wrapped,
                })
            } else {
                runtime_blocks
            }
        } else {
            runtime_blocks
        }
    } else {
        runtime_blocks
    };

    let mut assembly = match assemble_context(
        ContextInput {
            environment: EnvironmentSource {
                workspace_path: pending.workspace.display().to_string(),
                platform: platform.clone(),
                current_date: current_date.clone(),
            },
            project_instructions,
            global_memory,
            project_memory,
            history,
            tools: tool::to_tool_schemas(&registry),
            model_tiers,
            runtime_blocks,
            enable_memory: true,
            agent_name: pending.conversation.root_contact().as_str().to_string(),
            agent_instructions: pending
                .agent_registry
                .as_ref()
                .and_then(|r| r.get(pending.conversation.root_contact().as_str()))
                .map(|d| d.instructions.clone())
                .or_else(|| pending.query.agent_instructions.clone())
                .or_else(|| catalog.agents.get("main").map(|a| a.text.clone()))
                .unwrap_or_default(),
            response_contract: pending.query.response_contract.clone(),
        },
        &catalog,
    ) {
        Ok(assembly) => assembly,
        Err(error) => {
            let request_id = format!("req_{}", pending.request_num);
            append_model_error(
                &pending.events_path,
                &pending.conversation,
                pending.turn,
                request_id,
                "prompt_render",
                &error.to_string(),
            )?;
            append_turn_interrupted(
                &pending.events_path,
                &pending.conversation,
                pending.turn,
                "prompt_render_error",
            )?;
            return Err(error);
        }
    };

    let frozen = restore_prompt_snapshot(&existing_events, pending.conversation.as_str());
    let is_first_request = frozen.is_none();
    if let Some(frozen) = frozen {
        assembly.prelude_messages = frozen;
    }

    // Layer 4: inject agent catalog + loaded skills into snapshot prelude (first turn only;
    // subsequent turns reuse the frozen snapshot)
    if is_first_request {
        if let Some(catalog_text) = catalog_text {
            if !catalog_text.is_empty() {
                assembly
                    .prelude_messages
                    .push(CanonicalMessage::user_text(catalog_text));
            }
        }
        if let Some(skills_text) = skills_text {
            if !skills_text.is_empty() {
                assembly
                    .prelude_messages
                    .push(CanonicalMessage::user_text(skills_text));
            }
        }
    }

    if let Some(prefix) = pending
        .query
        .current_turn_prefix
        .as_ref()
        .filter(|value| !value.is_empty())
    {
        append_current_turn_prefix_once(&mut assembly.prelude_messages, prefix);
    }

    if let Some(skill) = pending.bootstrap_skill.as_ref() {
        assembly
            .prelude_messages
            .push(crate::context::CanonicalMessage::user_text(
                skill.body.clone(),
            ));
    }

    assembly.handoff_summary = handoff_summary;

    let estimated_input = last_input_tokens(
        &resolved_config.kind,
        &existing_events,
        pending.conversation.as_str(),
    );
    let thinking_overhead = resolved_config.think_level.overhead_tokens();
    let headroom = compute_context_headroom(
        resolved_config
            .max_context_tokens
            .saturating_sub(thinking_overhead),
        resolved_config.max_output_tokens,
        estimated_input,
    );

    let handoff_instruction = {
        let handoff_config = pending.config.handoff();
        if !pending.handoff_triggered
            && handoff_config.enabled
            && should_trigger_handoff(&headroom, handoff_config.threshold)
        {
            pending.handoff_triggered = true;
            pending.handoff_keep_turns = handoff_config.keep_turns;
            Some(if let Some(dir) = &pending.prompts_dir {
                load_prompt_template(dir, "runtime/handoff-instruction")
                    .unwrap_or_else(|_| builtin_handoff_instruction().to_string())
            } else {
                builtin_handoff_instruction().to_string()
            })
        } else {
            None
        }
    };

    let prelude_snapshot = assembly.snapshot_prelude();
    let runtime_date_text = crate::prompt::render_runtime_date(&catalog, &current_date)?;
    let runtime_date = if contains_current_date(&assembly, &runtime_date_text)
        || legacy_project_policy_has_date(&assembly, &catalog, &current_date)
    {
        None
    } else {
        Some(runtime_date_text)
    };
    let dynamic_turn_prefix = assembly_runtime_prefix(
        assembly.runtime_context.as_deref(),
        pending
            .bootstrap_skill
            .as_ref()
            .map(|skill| skill.body.as_str()),
        runtime_date.as_deref(),
        &catalog,
    );
    let mut current_turn_prefix = pending
        .frozen_turn_prefix
        .freeze_or_reuse(dynamic_turn_prefix);
    if let Some(instruction) = handoff_instruction {
        current_turn_prefix =
            append_handoff_instruction(current_turn_prefix, &instruction, &catalog);
        pending
            .frozen_turn_prefix
            .replace(current_turn_prefix.clone());
    }
    let current_body = pending
        .query
        .current_turn_body
        .as_deref()
        .unwrap_or(&pending.query.prompt);
    let current_input = build_current_user_message(current_turn_prefix, current_body);
    let current_user_index = if let Some(index) = replace_current_user_message(
        &mut assembly.history,
        &pending.query.prompt,
        current_body,
        current_input.clone(),
    ) {
        index
    } else {
        assembly.history.push(current_input.clone());
        assembly.history.len() - 1
    };

    if !pending.hook_context.is_empty() {
        let hook_text = pending.hook_context.join("\n");
        pending.hook_context.clear();
        if let Some(last_user) = assembly
            .history
            .iter_mut()
            .rev()
            .find(|m| m.role == crate::context::Role::User)
        {
            let hook_tmpl = catalog
                .blocks
                .get("hook-context")
                .map(|a| a.text.as_str())
                .unwrap_or("<kuku_hook_context>\n{{hook_text}}\n</kuku_hook_context>");
            insert_current_turn_metadata_block(
                last_user,
                hook_tmpl.replace("{{hook_text}}", &hook_text),
            );
        }
    }

    let request_id = format!("req_{}", pending.request_num);
    let _tier_name = pending
        .query
        .tier
        .clone()
        .unwrap_or_else(|| pending.config.default_tier().to_string());
    let think = resolved_config.think_level;
    let max_output = resolved_config.max_output_tokens;
    let _params = serde_json::json!({
        "max_output_tokens": max_output,
        "temperature": pending.query.temperature,
    });

    {
        let mut store = EventStore::open(&pending.events_path)?;
        if is_first_request {
            let tool_registry = ToolRegistryProvenance {
                hash: format!("count:{}", assembly.tools.len()),
                names: assembly
                    .tools
                    .iter()
                    .map(|tool| tool.name.clone())
                    .collect(),
                tool_count: assembly.tools.len(),
            };
            let agent_registry =
                pending
                    .agent_registry
                    .as_ref()
                    .map(|registry| AgentRegistryProvenance {
                        hash: registry.hash().to_string(),
                        names: registry.names().to_vec(),
                    });
            let skill_registry =
                pending
                    .skill_registry
                    .as_ref()
                    .map(|registry| SkillRegistryProvenance {
                        hash: registry.hash().to_string(),
                        names: registry.names().to_vec(),
                    });
            let plugin_registry =
                pending
                    .plugin_registry
                    .as_ref()
                    .map(|registry| PluginRegistryProvenance {
                        hash: format!("count:{}", registry.names().len()),
                        names: registry.names().to_vec(),
                        count: registry.names().len(),
                    });
            store.append(EventPayload::PromptSnapshot {
                ts: now_timestamp()?,
                conversation: pending.conversation.as_str().to_string(),
                binding_id: pending
                    .agent_binding_id
                    .clone()
                    .unwrap_or_else(|| pending.conversation.as_str().to_string()),
                snapshot_id: format!(
                    "{}:{}:{}",
                    pending.conversation.as_str(),
                    pending.turn,
                    pending.request_num
                ),
                turn: pending.turn,
                messages: prelude_snapshot,
                project_instruction_sources: assembly
                    .project_instruction_sources
                    .iter()
                    .map(|source| crate::context::FileSource {
                        path: source.path.clone(),
                        hash: source.hash.clone(),
                    })
                    .collect(),
                memory_sources: assembly
                    .memory_sources
                    .iter()
                    .map(|source| crate::context::FileSource {
                        path: source.path.clone(),
                        hash: source.hash.clone(),
                    })
                    .collect(),
                prompt_asset_sources: assembly.prompt_asset_sources.clone(),
                skills: pending
                    .skill_registry
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()?
                    .unwrap_or_else(|| serde_json::json!({})),
                bootstrap_loaded: pending
                    .bootstrap_skill
                    .as_ref()
                    .and_then(|skill| skill.name.clone())
                    .into_iter()
                    .collect(),
                provider: resolved_config.kind.as_str().to_string(),
                model: resolved_config.model.clone(),
                renderer: PromptRendererIdentity {
                    provider: resolved_config.kind.as_str().to_string(),
                    renderer: resolved_config.kind.as_str().to_string(),
                },
                tool_registry: Box::new(tool_registry),
                agent_registry,
                skill_registry: Box::new(skill_registry),
                plugin_registry: Box::new(plugin_registry),
                capabilities: PromptCapabilityMetadata {
                    context_budget_tier: match headroom.tier {
                        crate::notice::types::ContextBudgetTier::Tight => "tight",
                        crate::notice::types::ContextBudgetTier::Normal => "normal",
                        crate::notice::types::ContextBudgetTier::Roomy => "roomy",
                    }
                    .to_string(),
                    max_context_tokens: Some(headroom.max_context_tokens),
                    remaining_input_tokens: headroom.remaining_input_tokens,
                },
            })?;
        }
        store.append(EventPayload::ContextSources {
            turn: pending.turn,
            ts: now_timestamp()?,
            request_id: request_id.clone(),
            project_instruction_sources: assembly
                .project_instruction_sources
                .iter()
                .map(|source| crate::context::FileSource {
                    path: source.path.clone(),
                    hash: source.hash.clone(),
                })
                .collect(),
            memory_sources: assembly
                .memory_sources
                .iter()
                .map(|source| crate::context::FileSource {
                    path: source.path.clone(),
                    hash: source.hash.clone(),
                })
                .collect(),
        })?;
    }

    let current_input = assembly
        .history
        .get(current_user_index)
        .cloned()
        .ok_or_else(|| {
            crate::error::Error::InvalidEventStream(
                "assembled request has no current user message".to_string(),
            )
        })?;
    let request_base = OwnedRequestBase::new(
        assembly,
        catalog,
        crate::provider::types::CanonicalPromptInput {
            parts: vec![current_input],
        },
        resolved_config.model.clone(),
        max_output,
        pending.query.temperature,
        think,
        resolved_config.thinking.clone(),
        current_user_index,
    )?;
    pending.request_base = Some(request_base);

    dispatch_provider_step(pending, resolved_config, existing_events, None).await
}

pub(super) async fn compact_context(mut pending: PendingRun, closed_turn: bool) -> Result<()> {
    ensure_resolved(&mut pending)?;
    let resolved = pending
        .resolved
        .as_ref()
        .expect("resolved runtime exists")
        .config
        .clone();
    let capacity = pending
        .query
        .model_capacity
        .clone()
        .expect("Session model capacity");
    let _permit = tokio::select! {
        biased;
        _ = pending.cancel_token.notified() => return Ok(()),
        permit = capacity.acquire_owned() => permit.map_err(|_| crate::Error::InvalidArgument("session model capacity is closed".into()))?,
    };
    let mut context = super::frozen::prepare(&mut pending, true)?;
    let correlation = crate::provider::types::ProviderCorrelation {
        session_id: pending.session_id.clone(),
        agent_address: pending.conversation.as_str().into(),
        connection_name: resolved.connection_name.clone(),
        workspace: pending.workspace.clone(),
    };
    let before = dispatch::prepare_request(&pending, &resolved, &correlation, &context.request)?;
    pending.request_base = Some(context.request);
    if matches!(
        Box::pin(compaction::compact(
            &mut pending,
            &mut context.commit,
            &resolved,
            &correlation,
            before.body.as_bytes().len(),
            compaction::Trigger::Manual { closed_turn }
        ))
        .await?,
        compaction::Outcome::Keep
    ) {
        return Err(crate::Error::InvalidArgument(
            "this context could not be made smaller within the available limits".into(),
        ));
    }
    Ok(())
}

pub(super) fn require_runtime_execution(pending: &PendingRun) -> Result<()> {
    if let Some(execution) = pending.query.runtime_execution {
        runtime_session(pending, execution)?
            .require_execution(pending.conversation.as_str(), execution)?;
    }
    Ok(())
}

fn apply_runtime_boundary(pending: &mut PendingRun) -> Result<()> {
    let Some(execution) = pending.query.runtime_execution else {
        return Ok(());
    };
    let session = runtime_session(pending, execution)?;
    let boundary =
        session.prepare_step_boundary(pending.conversation.as_str(), execution, pending.turn)?;
    pending.query.input_skills.extend(boundary.skills);
    if let Some(source) = boundary.source {
        pending.query.input_source = source;
    }
    if let Some(config) = boundary.config {
        pending.config = std::sync::Arc::new(session.current_model_config().into_owned());
        pending.query.connection = config.connection;
        pending.query.tier = config.tier;
        pending.query.model = config.model;
        pending.query.reasoning_effort = config.reasoning_effort;
        pending.query.output = config.output;
        pending.query.service_tier = config.service_tier;
        pending.query.max_output_tokens = config.max_output_tokens;
        pending.resolved = None;
    }
    #[cfg(feature = "test_support")]
    crate::test_support::wait_after_runtime_boundary(&pending.events_path);
    Ok(())
}

pub(super) fn runtime_session(pending: &PendingRun, execution: u64) -> Result<crate::Session> {
    pending
        .query
        .runtime
        .as_ref()
        .and_then(std::sync::Weak::upgrade)
        .map(|inner| crate::Session { inner })
        .ok_or(crate::Error::ExecutionNotActive(execution))
}

pub(super) fn pending_failure_step(
    mut pending: PendingRun,
    lead_events: Vec<UiEvent>,
    error: crate::error::Error,
) -> PendingStep {
    pending.pending_events.extend(lead_events.into_iter().rev());
    pending.flush_runtime_logs();
    pending.pending_error = Some(error);
    PendingStep::Pending {
        pending: Box::new(pending),
        slot: None,
        event: None,
    }
}

pub(super) fn emit_runtime_log(
    pending: &mut PendingRun,
    level: LogLevel,
    kind: impl Into<String>,
    message: impl Into<String>,
    data: Option<serde_json::Value>,
) -> Result<()> {
    let mut record = LogRecord::new(now_timestamp()?, level, LogScope::Runtime);
    record.kind = kind.into();
    record.message = message.into();
    record.session_id = Some(pending.session_id.clone());
    record.run_id = Some(pending.session_id.clone());
    record.workspace = Some(pending.workspace.display().to_string());
    record.turn = Some(pending.turn);
    record.data = data;
    pending.pending_events.push_back(UiEvent::Log { record });
    Ok(())
}

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
fn build_runtime_blocks(
    workspace: &std::path::Path,
    conversation: &str,
    turn: u64,
    agent_registry: Option<&crate::agent::registry::AgentRegistry>,
    skill_registry: Option<&crate::skill::registry::SkillRegistry>,
    previous_skill_registry: Option<&crate::skill::registry::SkillRegistry>,
    resolved_config: &crate::provider::types::ResolvedProvider,
    existing_events: &[crate::event::StoredEvent],
    catalog: &crate::prompt::PromptCatalog,
) -> Result<(Option<String>, Option<String>, Option<String>)> {
    let estimated_input = last_input_tokens(&resolved_config.kind, existing_events, conversation);
    let thinking_overhead = resolved_config.think_level.overhead_tokens();
    let context_headroom = compute_context_headroom(
        resolved_config
            .max_context_tokens
            .saturating_sub(thinking_overhead),
        resolved_config.max_output_tokens,
        estimated_input,
    );

    // Build agent catalog — this goes into snapshot prelude, not runtime_blocks
    let catalog_text =
        agent_registry.and_then(|reg| crate::agent::catalog::render_agent_catalog(reg, catalog));

    // Build skill catalog — this goes into snapshot prelude, not runtime_blocks
    let skills_text = skill_registry.and_then(crate::skill::catalog::render_static_skill_catalog);
    let skill_changes = if turn > 1 {
        skill_registry.and_then(|skill_registry| {
            previous_skill_registry.and_then(|previous| {
                crate::skill::registry::detect_skill_changes(previous, skill_registry)
            })
        })
    } else {
        None
    };

    // Dynamic notices remain in runtime_blocks (conversations, inbox, drift)
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
            events: &notice_events,
            context_budget_tier: context_headroom.tier,
            conversation: &conversation,
            agent_registry,
        });
        for notice in &notices {
            if let Some(body) = render_notice_body(notice, catalog) {
                notice_bodies.push(body);
            }
        }
        let skill_change_line = skill_changes
            .as_ref()
            .and_then(crate::skill::catalog::render_skill_change_notice);
        if let Some(change_line) = skill_change_line {
            let template = catalog
                .blocks
                .get("notice-skill-changes")
                .map(|asset| asset.text.as_str())
                .unwrap_or("<kuku_system_notice>\n{{skill_changes}}\n</kuku_system_notice>");
            notice_bodies.push(template.replace("{{skill_changes}}", &change_line));
        }
    }

    // Notices are now self-wrapped with <kuku_system_notice> via templates;
    // just join them without adding an outer wrapper.
    let runtime_blocks = if notice_bodies.is_empty() {
        None
    } else {
        Some(notice_bodies.join("\n\n"))
    };

    Ok((catalog_text, skills_text, runtime_blocks))
}

fn assembly_runtime_prefix(
    runtime_context: Option<&str>,
    skill_body: Option<&str>,
    runtime_date: Option<&str>,
    catalog: &crate::prompt::PromptCatalog,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(runtime_context) = runtime_context.filter(|value| !value.is_empty()) {
        let tmpl = catalog
            .blocks
            .get("runtime-notices")
            .map(|a| a.text.as_str())
            .unwrap_or("<kuku_runtime_notices>{{runtime_notices_content}}</kuku_runtime_notices>");
        parts.push(tmpl.replace("{{runtime_notices_content}}", runtime_context));
    }
    if let Some(runtime_date) = runtime_date.filter(|value| !value.is_empty()) {
        parts.push(runtime_date.to_string());
    }
    if let Some(skill_body) = skill_body.filter(|value| !value.is_empty()) {
        let tmpl = catalog
            .blocks
            .get("conversation-inbox")
            .map(|a| a.text.as_str())
            .unwrap_or(
                "<kuku_conversation_inbox>{{conversation_inbox_content}}</kuku_conversation_inbox>",
            );
        parts.push(tmpl.replace("{{conversation_inbox_content}}", skill_body));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn contains_current_date(assembly: &crate::context::ContextAssembly, runtime_date: &str) -> bool {
    assembly
        .prelude_messages
        .iter()
        .chain(&assembly.history)
        .any(|message| {
            message.blocks.iter().any(
                |block| matches!(block, MessageBlock::Text(text) if text.contains(runtime_date)),
            )
        })
}

fn legacy_project_policy_has_date(
    assembly: &crate::context::ContextAssembly,
    catalog: &crate::prompt::PromptCatalog,
    current_date: &str,
) -> bool {
    catalog.blocks.get("project-policy").is_some_and(|template| {
        assembly.prelude_messages.iter().any(|message| {
            message.blocks.iter().any(|block| {
                matches!(block, MessageBlock::Text(text) if crate::prompt::render::project_policy_has_date(text, &template.text, current_date))
            })
        })
    })
}

pub(super) fn append_handoff_instruction(
    prefix: Option<String>,
    instruction: &str,
    catalog: &crate::prompt::PromptCatalog,
) -> Option<String> {
    let Some(prefix) = prefix else {
        let notices_tmpl = catalog
            .blocks
            .get("runtime-notices")
            .map(|a| a.text.as_str())
            .unwrap_or("<kuku_runtime_notices>{{runtime_notices_content}}</kuku_runtime_notices>");
        let inbox_tmpl = catalog
            .blocks
            .get("conversation-inbox")
            .map(|a| a.text.as_str())
            .unwrap_or(
                "<kuku_conversation_inbox>{{conversation_inbox_content}}</kuku_conversation_inbox>",
            );
        return Some(format!(
            "{}\n{}",
            notices_tmpl.replace("{{runtime_notices_content}}", instruction),
            inbox_tmpl.replace("{{conversation_inbox_content}}", ""),
        ));
    };

    if let Some(index) = prefix.rfind("</kuku_runtime_notices>") {
        let (before, after) = prefix.split_at(index);
        Some(format!("{before}\n\n{instruction}{after}"))
    } else {
        Some(format!("{prefix}\n{instruction}"))
    }
}

fn build_current_user_message(
    prefix: Option<String>,
    prompt: &str,
) -> crate::context::CanonicalMessage {
    let mut blocks = Vec::new();
    if let Some(prefix) = prefix {
        blocks.push(crate::context::MessageBlock::Text(prefix));
    }
    blocks.push(crate::context::MessageBlock::Text(prompt.to_string()));
    crate::context::CanonicalMessage::user(blocks)
}

fn replace_latest_user_message(
    history: &mut [CanonicalMessage],
    prompt: &str,
    replacement: CanonicalMessage,
) -> Option<usize> {
    for (index, message) in history.iter_mut().enumerate().rev() {
        if message.role != Role::User || message.blocks.len() != 1 {
            continue;
        }
        let MessageBlock::Text(text) = &message.blocks[0] else {
            continue;
        };
        if text == prompt {
            *message = replacement;
            return Some(index);
        }
    }
    None
}

fn replace_current_user_message(
    history: &mut [CanonicalMessage],
    raw_prompt: &str,
    current_body: &str,
    replacement: CanonicalMessage,
) -> Option<usize> {
    if current_body != raw_prompt {
        if let Some(index) = replace_latest_user_message(history, current_body, replacement.clone())
        {
            return Some(index);
        }
    }
    replace_latest_user_message(history, raw_prompt, replacement)
}

fn append_current_turn_prefix_once(messages: &mut Vec<CanonicalMessage>, prefix: &str) {
    if messages.iter().any(|message| {
        message.blocks.iter().any(|block| match block {
            MessageBlock::Text(text) => text.contains(prefix),
            MessageBlock::Thinking(_)
            | MessageBlock::ToolUse(_)
            | MessageBlock::ToolResult(_)
            | MessageBlock::ProviderReplay { .. } => false,
        })
    }) {
        return;
    }
    messages.push(CanonicalMessage::user_text(prefix.to_string()));
}

fn insert_current_turn_metadata_block(message: &mut CanonicalMessage, text: String) {
    let insert_at = message.blocks.len().saturating_sub(1);
    message
        .blocks
        .insert(insert_at, crate::context::MessageBlock::Text(text));
}

#[cfg(test)]
mod tests;
