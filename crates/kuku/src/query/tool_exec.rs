use crate::error::Result;
use crate::event::{EventPayload, EventStore};
use crate::provider::types::ProviderToolCall;

use super::helpers::now_timestamp;
use super::types::{ExecSlot, PendingRun};

pub(super) struct HookBlockResult {
    pub(super) reason: String,
    pub(super) _package: String,
}

pub(super) struct HookPreResult {
    pub(super) block: Option<HookBlockResult>,
    pub(super) args: serde_json::Value,
}

pub(crate) fn write_tool_result(
    slot: &ExecSlot,
    status: &str,
    summary: &str,
    model_content: &str,
    result: &Option<serde_json::Value>,
    events_path: &std::path::Path,
    turn: u64,
) -> crate::error::Result<()> {
    let mut store = crate::event::EventStore::open(events_path)?;
    store.append(crate::event::EventPayload::ToolResult {
        turn,
        ts: now_timestamp()?,
        tool_call_id: slot.tool_call_id.clone(),
        status: status.to_string(),
        summary: summary.to_string(),
        model_content: model_content.to_string(),
        truncated: false,
        structured: result.clone(),
    })?;
    Ok(())
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

    let skill_md_path = std::path::Path::new(&skill_dir).join("SKILL.md");
    let content = std::fs::read_to_string(&skill_md_path)?;
    let (_, body) = crate::util::yaml::split_yaml_frontmatter(&content);

    let result = format!("<!-- loaded: {skill_dir} -->\n\n{body}");

    Ok(crate::tool::ToolResultEnvelope {
        status: "ok".to_string(),
        summary: format!("loaded skill: {skill_name}"),
        model_content: result,
        truncated: false,
        structured: None,
    })
}

pub(super) async fn execute_tool_call(
    pending: &mut PendingRun,
    tool_call: &ProviderToolCall,
) -> Result<crate::tool::ToolResultEnvelope> {
    if tool_call.name == "use_skill" {
        let result = handle_use_skill(pending, tool_call)?;
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
                    reason: r.stderr.clone(),
                    _package: r.package_name.clone(),
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
    let mut store = EventStore::open(events_path)?;
    for r in results {
        if r.exit_code != 0 || r.output.block || r.timed_out {
            store.append(EventPayload::PluginHook {
                turn,
                ts: now_timestamp()?,
                event: hook_event_name.to_string(),
                package: r.package_name.clone(),
                command: String::new(),
                exit_code: r.exit_code,
                blocked: r.output.block,
                duration_ms: r.duration_ms,
                output_summary: r
                    .output
                    .additional_context
                    .as_deref()
                    .unwrap_or("")
                    .to_string(),
            })?;
        }
    }
    Ok(())
}
