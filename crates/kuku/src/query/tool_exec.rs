use crate::error::Result;
use crate::event::{EventPayload, EventStore};
use crate::provider::types::ProviderToolCall;
use serde_json::json;

use super::helpers::{is_inline_skill_tool, now_timestamp, resolved_tool_available};
use super::types::{ExecSlot, PendingRun};

pub(super) struct HookBlockResult {
    pub(super) reason: String,
}

pub(super) struct HookPreResult {
    pub(super) block: Option<HookBlockResult>,
    pub(super) args: serde_json::Value,
}

fn finalize_persisted_tool_result(
    result_event_id: u64,
    result: &Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let mut result = result.clone();
    let Some(structured) = result.as_mut() else {
        return result;
    };
    if structured["kind"] == "file_content" {
        structured["read_event_id"] = serde_json::Value::from(result_event_id);
    }
    result
}

pub(crate) fn write_tool_result(
    slot: &ExecSlot,
    status: &str,
    summary: &str,
    model_content: &str,
    result: &Option<serde_json::Value>,
    events_path: &std::path::Path,
    turn: u64,
) -> crate::error::Result<Option<serde_json::Value>> {
    let mut store = crate::event::EventStore::open(events_path)?;
    let structured = finalize_persisted_tool_result(store.next_id(), result);
    let stored = store.append(crate::event::EventPayload::ToolResult {
        turn,
        ts: now_timestamp()?,
        tool_call_id: slot.tool_call_id.clone(),
        status: status.to_string(),
        summary: summary.to_string(),
        model_content: model_content.to_string(),
        truncated: false,
        structured: structured.clone(),
    })?;
    Ok(match stored.payload {
        EventPayload::ToolResult { structured, .. } => structured,
        _ => None,
    })
}

fn current_skill_events(pending: &PendingRun) -> Result<Vec<crate::event::StoredEvent>> {
    EventStore::replay(&pending.events_path)
}

fn current_skill_registry(pending: &PendingRun) -> crate::skill::registry::SkillRegistry {
    pending
        .skill_registry
        .clone()
        .unwrap_or_else(|| crate::skill::registry::SkillRegistry::builder().build())
}

fn json_result(
    summary: String,
    payload: serde_json::Value,
) -> Result<crate::tool::ToolResultEnvelope> {
    Ok(crate::tool::ToolResultEnvelope::ok(
        summary,
        serde_json::to_string_pretty(&payload)?,
        payload,
    ))
}

fn inline_skill_tool_max_result_chars(pending: &PendingRun, tool_name: &str) -> Option<usize> {
    if let Some(resolved) = pending.resolved.as_ref() {
        return resolved
            .registry
            .iter()
            .find(|tool| tool.name == tool_name)
            .map(|tool| tool.max_result_chars);
    }
    if let Some(registry) = pending.tool_registry_override.as_ref() {
        return registry
            .iter()
            .find(|tool| tool.name == tool_name)
            .map(|tool| tool.max_result_chars);
    }
    crate::tool::builtin_registry(!pending.query.disable_agents, !pending.query.disable_skills)
        .iter()
        .find(|tool| tool.name == tool_name)
        .map(|tool| tool.max_result_chars)
}

fn truncate_to_max_chars(text: &str, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        return (text.to_string(), false);
    }
    (text.chars().take(max_chars).collect(), true)
}

fn clamp_inline_skill_tool_result(
    pending: &PendingRun,
    tool_name: &str,
    mut result: crate::tool::ToolResultEnvelope,
) -> crate::tool::ToolResultEnvelope {
    let Some(max_result_chars) = inline_skill_tool_max_result_chars(pending, tool_name) else {
        return result;
    };
    let (model_content, truncated) = truncate_to_max_chars(&result.model_content, max_result_chars);
    result.model_content = model_content;
    result.truncated = result.truncated || truncated;
    result
}

fn handle_use_skill(
    pending: &PendingRun,
    tool_call: &ProviderToolCall,
) -> Result<crate::tool::ToolResultEnvelope> {
    let skill_name = tool_call
        .args
        .get("skill_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let definition = pending
        .skill_registry
        .as_ref()
        .and_then(|reg| reg.get(skill_name))
        .cloned();

    let Some(def) = definition else {
        return Ok(crate::tool::ToolResultEnvelope::error(
            format!("skill '{skill_name}' not found"),
            format!("no skill named '{skill_name}' in the registry"),
        ));
    };

    let skill_dir = def.source_path.as_deref().unwrap_or("").to_string();
    let result = format!("<!-- loaded: {skill_dir} -->\n\n{}", def.instructions);

    Ok(crate::tool::ToolResultEnvelope {
        status: "ok".to_string(),
        summary: format!("loaded skill: {skill_name}"),
        model_content: result,
        truncated: false,
        structured: Some(json!({
            "kind": "skill_load",
            "name": def.name,
            "description": def.description,
            "source": def.source.as_str(),
            "loaded": true,
        })),
    })
}

fn handle_list_skills(
    pending: &PendingRun,
    tool_call: &ProviderToolCall,
) -> Result<crate::tool::ToolResultEnvelope> {
    let events = current_skill_events(pending)?;
    let registry = current_skill_registry(pending);
    let payload = crate::skill::search::list_skills_result(&registry, &events, &tool_call.args);
    let count = payload["skills"].as_array().map_or(0, Vec::len);
    json_result(format!("{count} skills returned"), payload)
}

fn handle_search_skills(
    pending: &PendingRun,
    tool_call: &ProviderToolCall,
) -> Result<crate::tool::ToolResultEnvelope> {
    let events = current_skill_events(pending)?;
    let registry = current_skill_registry(pending);
    let payload = crate::skill::search::search_skills_result(&registry, &events, &tool_call.args);
    let count = payload["skills"].as_array().map_or(0, Vec::len);
    json_result(format!("{count} skill matches returned"), payload)
}

pub(super) async fn execute_tool_call(
    pending: &mut PendingRun,
    tool_call: &ProviderToolCall,
) -> Result<crate::tool::ToolResultEnvelope> {
    if is_inline_skill_tool(&tool_call.name) && !resolved_tool_available(pending, &tool_call.name) {
        return Ok(crate::tool::ToolResultEnvelope::error(
            format!("failed: unknown tool: {}", tool_call.name),
            format!("unknown tool: {}", tool_call.name),
        ));
    }
    if matches!(
        tool_call.name.as_str(),
        "use_skill" | "list_skills" | "search_skills"
    ) {
        let result = match tool_call.name.as_str() {
            "use_skill" => handle_use_skill(pending, tool_call)?,
            "list_skills" => handle_list_skills(pending, tool_call)?,
            "search_skills" => handle_search_skills(pending, tool_call)?,
            _ => unreachable!(),
        };
        let result = clamp_inline_skill_tool_result(pending, &tool_call.name, result);
        let mut store = EventStore::open(&pending.events_path)?;
        store.append(EventPayload::ToolResult {
            turn: pending.turn,
            ts: now_timestamp()?,
            tool_call_id: tool_call.id.clone(),
            status: result.status.clone(),
            summary: result.summary.clone(),
            model_content: result.model_content.clone(),
            truncated: result.truncated,
            structured: result.structured.clone(),
        })?;
        return Ok(result);
    }

    let prior_events = EventStore::replay(&pending.events_path)?;
    let result_event_id = EventStore::open(&pending.events_path)?.next_id();
    let result = crate::tool::dispatch(
        &tool_call.name,
        &tool_call.args,
        &pending.workspace,
        &pending.kuku_home,
        &prior_events,
        result_event_id,
        Some(&tool_call.id),
        &pending.config,
        &pending.catalog,
        &pending.events_path,
    )
    .await;
    let mut store = EventStore::open(&pending.events_path)?;
    let stored = store.append(EventPayload::ToolResult {
        turn: pending.turn,
        ts: now_timestamp()?,
        tool_call_id: tool_call.id.clone(),
        status: result.status.clone(),
        summary: result.summary.clone(),
        model_content: result.model_content.clone(),
        truncated: result.truncated,
        structured: result.structured.clone(),
    })?;
    let mut result = result;
    if let EventPayload::ToolResult { structured, .. } = stored.payload {
        result.structured = structured;
    }
    Ok(result)
}

pub(super) async fn run_tool_pre_hooks(
    pending: &mut PendingRun,
    tool_name: &str,
    tool_args: &serde_json::Value,
    tool_call_id: &str,
) -> Result<HookPreResult> {
    let Some(ref plugin_reg) = pending.plugin_registry else {
        return Ok(HookPreResult {
            block: None,
            args: tool_args.clone(),
        });
    };
    let hooks = plugin_reg.hooks_for(crate::plugin::HookEvent::ToolPreExecute);
    if hooks.is_empty() {
        return Ok(HookPreResult {
            block: None,
            args: tool_args.clone(),
        });
    }
    let input = crate::plugin::executor::HookInput {
        event: "tool.pre_execute".to_string(),
        session_dir: pending
            .events_path
            .parent()
            .unwrap()
            .to_string_lossy()
            .to_string(),
        extra: serde_json::json!({
            "tool_name": tool_name,
            "tool_args": tool_args,
            "tool_call_id": tool_call_id,
        }),
    };
    let session_dir = pending.events_path.parent().unwrap().to_path_buf();
    let workspace = pending.workspace.clone();
    let results =
        crate::plugin::executor::execute_hooks(hooks, &input, &session_dir, &workspace).await?;
    record_plugin_hooks(
        &pending.events_path,
        pending.turn,
        "tool.pre_execute",
        &results,
    )?;

    let mut updated_args = tool_args.clone();
    for r in &results {
        if r.output.block {
            return Ok(HookPreResult {
                block: Some(HookBlockResult {
                    reason: r.stderr.trim_end().to_string(),
                }),
                args: updated_args,
            });
        }
        if let Some(ref ctx) = r.output.additional_context {
            pending.hook_context.push(ctx.clone());
        }
        if let Some(ref new_args) = r.output.updated_args {
            updated_args = new_args.clone();
        }
    }

    Ok(HookPreResult {
        block: None,
        args: updated_args,
    })
}

pub(super) fn record_plugin_hooks(
    events_path: &std::path::Path,
    turn: u64,
    hook_event_name: &str,
    results: &[crate::plugin::executor::HookExecResult],
) -> Result<()> {
    let _ = (events_path, turn, hook_event_name, results);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::provider::types::{ProviderKind, ProviderToolCall, ResolvedProvider, SecretString};
    use crate::query::types::{CumulativeUsage, PendingRun, Query, ResolvedRuntime};
    use crate::skill::definition::{SkillDefinition, SkillSource};
    use crate::tool::ToolDefinition;

    fn test_config() -> crate::config::Config {
        crate::config::Config {
            tiers: std::collections::BTreeMap::new(),
            providers: std::collections::BTreeMap::new(),
            default_tier: "balanced".to_string(),
            discovery: crate::config::DiscoveryConfig::default(),
            handoff: crate::config::HandoffConfig::default(),
            logs: crate::config::LogsConfig::default(),
            plugin: crate::config::PluginConfig::default(),
            update: crate::config::UpdateConfig::default(),
        }
    }

    fn test_resolved_runtime(registry: Vec<ToolDefinition>) -> ResolvedRuntime {
        ResolvedRuntime {
            config: ResolvedProvider {
                kind: ProviderKind::OpenAiCompatible,
                model: "test-model".to_string(),
                base_url: "https://example.test".to_string(),
                api_key: SecretString::new("test-key"),
                max_context_tokens: 1000,
                max_output_tokens: 1000,
                think_level: crate::config::ThinkLevel::Off,
                thinking: crate::config::ResolvedThinking::default(),
            },
            registry,
        }
    }

    fn skill_registry() -> crate::skill::registry::SkillRegistry {
        let mut definition = SkillDefinition {
            name: "review".to_string(),
            description: "D".repeat(200),
            instructions: "I".repeat(500),
            source: SkillSource::Project,
            hash: String::new(),
            source_path: Some("/skills/review".to_string()),
            allowed_tools: None,
            disallowed_tools: None,
            max_turns: None,
            model: None,
            license: None,
            compatibility: None,
            metadata: serde_json::json!({"title": "Very Long Review Skill"}),
        };
        definition.hash = definition.compute_hash();
        crate::skill::registry::SkillRegistry::builder()
            .with_definition(definition)
            .build()
    }

    fn pending_with_skill_tool(
        tool: ToolDefinition,
        override_registry: Option<Vec<ToolDefinition>>,
        no_skills: bool,
    ) -> PendingRun {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().to_path_buf();
        std::mem::forget(dir);
        let events_path = workspace.join("events.jsonl");
        std::fs::write(&events_path, "").unwrap();
        let mut query = Query::new("test");
        if no_skills {
            query = query.no_skills();
        }
        PendingRun {
            session_id: "test".to_string(),
            query,
            events_path,
            kuku_home: workspace.clone(),
            workspace: workspace.clone(),
            policy_path: workspace.join("policy.md"),
            turn: 1,
            request_num: 1,
            cumulative: CumulativeUsage::default(),
            resolved: Some(test_resolved_runtime(vec![tool])),
            queued_tool_calls: std::collections::VecDeque::new(),
            resumed_permission_requests: std::collections::VecDeque::new(),
            config: Arc::new(test_config()),
            prompts_dir: None,
            subagent_registry: None,
            skill_registry: Some(skill_registry()),
            previous_skill_registry: None,
            bootstrap_skill: None,
            child_session_count: 0,
            tool_registry_override: override_registry,
            catalog: crate::prompt::builtin_prompt_catalog(),
            pending_events: std::collections::VecDeque::new(),
            pending_error: None,
            cancel_token: Arc::new(tokio::sync::Notify::new()),
            handoff_triggered: false,
            handoff_keep_turns: test_config().handoff().keep_turns,
            plugin_registry: None,
            hook_context: Vec::new(),
            force_continue_count: 0,
            model_request_count: 0,
            tool_rounds: 0,
            tool_calls: 0,
            tool_names: Vec::new(),
            tool_denied: 0,
            tool_errors: 0,
            thinking_duration_ms: 0,
            runtime_log_writer: crate::log::BufferedLogWriter::new(workspace.join("runtime.jsonl")),
        }
    }

    #[tokio::test]
    async fn no_skills_override_membership_does_not_reenable_inline_skill_tools() {
        let mut tool = crate::tool::builtin::use_skill_definition();
        tool.max_result_chars = 20;
        let override_registry = Some(vec![tool.clone()]);
        let mut pending = pending_with_skill_tool(tool, override_registry, true);
        pending.resolved = None;
        let call = ProviderToolCall {
            id: "tool_use_skill".to_string(),
            name: "use_skill".to_string(),
            args: serde_json::json!({"skill_name": "review"}),
            index: 0,
        };

        let result = execute_tool_call(&mut pending, &call).await.unwrap();

        assert_eq!(result.status, "error");
        assert_eq!(result.summary, "failed: unknown tool: use_skill");
    }

    #[tokio::test]
    async fn use_skill_result_is_truncated_to_tool_limit() {
        let mut tool = crate::tool::builtin::use_skill_definition();
        tool.max_result_chars = 40;
        let mut pending = pending_with_skill_tool(tool, None, false);
        let call = ProviderToolCall {
            id: "tool_use_skill".to_string(),
            name: "use_skill".to_string(),
            args: serde_json::json!({"skill_name": "review"}),
            index: 0,
        };

        let result = execute_tool_call(&mut pending, &call).await.unwrap();

        assert_eq!(result.status, "ok");
        assert!(result.truncated);
        assert_eq!(result.model_content.chars().count(), 40);
    }

    #[tokio::test]
    async fn list_and_search_skill_results_are_truncated_to_tool_limit() {
        for (tool_name, args, definition_fn) in [
            (
                "list_skills",
                serde_json::json!({}),
                crate::tool::builtin::list_skills_definition as fn() -> ToolDefinition,
            ),
            (
                "search_skills",
                serde_json::json!({"query": "review"}),
                crate::tool::builtin::search_skills_definition as fn() -> ToolDefinition,
            ),
        ] {
            let mut tool = definition_fn();
            tool.max_result_chars = 30;
            let mut pending = pending_with_skill_tool(tool, None, false);
            let call = ProviderToolCall {
                id: format!("tool_{tool_name}"),
                name: tool_name.to_string(),
                args,
                index: 0,
            };

            let result = execute_tool_call(&mut pending, &call).await.unwrap();

            assert_eq!(result.status, "ok");
            assert!(result.truncated, "{tool_name} should be truncated");
            assert_eq!(result.model_content.chars().count(), 30);
        }
    }
}
