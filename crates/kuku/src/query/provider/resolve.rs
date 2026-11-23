use crate::error::Result;
use crate::event::EventStore;
use crate::provider::config::{resolve_config, ResolveConfigInput};
use crate::query::helpers::{append_model_error, append_turn_interrupted};
use crate::query::types::{PendingRun, ResolvedRuntime};
use crate::tool;

pub(in crate::query) fn ensure_resolved(pending: &mut PendingRun) -> Result<()> {
    if pending.resolved.is_some() {
        return Ok(());
    }

    let config = match resolve_config(ResolveConfigInput {
        provider: pending.query.provider,
        model: pending.query.model.clone(),
        tier: pending.query.tier.clone(),
        base_url: pending.query.base_url.clone(),
        api_key: pending.query.api_key.clone(),
        max_output_tokens: pending.query.max_output_tokens,
        max_output_tokens_per_request: pending.query.runtime_limits.max_output_tokens_per_request,
        config: Some((*pending.config).clone()),
        connection: pending.query.connection.clone(),
        reasoning_effort: pending.query.reasoning_effort.clone(),
        output: pending.query.output,
        service_tier: pending.query.service_tier.clone(),
    }) {
        Ok(config) => config,
        Err(error) => {
            let request_id = format!(
                "req_{}",
                EventStore::open(&pending.events_path)?.last_id() + 1
            );
            append_model_error(
                &pending.events_path,
                &pending.conversation,
                pending.turn,
                request_id,
                "missing_config",
                &error.to_string(),
            )?;
            append_turn_interrupted(
                &pending.events_path,
                &pending.conversation,
                pending.turn,
                "missing_config",
            )?;
            return Err(error);
        }
    };

    let mut registry = if let Some(ref overridden) = pending.tool_registry_override {
        overridden.clone()
    } else if pending.query.runtime.is_some() {
        tool::registry::session_registry(!pending.query.disable_skills)
    } else {
        tool::builtin_registry(!pending.query.disable_skills)
    };
    if pending.query.runtime.is_some() {
        registry.retain(|tool| tool.name != "fetch_web");
        if pending.query.tool_registry_override.is_none() {
            registry.extend(
                crate::query::runtime_tools::definitions()
                    .into_iter()
                    .filter(|tool| {
                        !pending.query.disable_agents
                            || !matches!(
                                tool.name.as_str(),
                                "create_agent" | "send_message" | "list_agents"
                            )
                    }),
            );
        }
        if pending.query.disable_agents {
            registry.retain(|tool| {
                !matches!(
                    tool.name.as_str(),
                    "create_agent" | "send_message" | "list_agents"
                )
            });
        }
        registry.retain(|tool| {
            crate::query::runtime_tool_supported(
                &config,
                pending.query.model_adapter.is_none(),
                &tool.name,
            )
        });
    }
    pending.resolved = Some(ResolvedRuntime { config, registry });
    Ok(())
}

pub(in crate::query) fn usable_model_routes(
    config: &crate::config::Config,
) -> Vec<crate::config::TierInfo> {
    config
        .tier_infos()
        .into_iter()
        .filter_map(|mut info| {
            let selected = crate::adapter::inspect_model(
                config,
                &crate::AgentConfig {
                    tier: Some(info.name.clone()),
                    ..crate::AgentConfig::default()
                },
            )
            .ok()?;
            if selected.output != crate::config::RequestedOutput::Text {
                return None;
            }
            info.reasoning_effort = selected.settings.reasoning_effort;
            info.reasoning_choices = selected
                .capabilities
                .selectable_reasoning_efforts()
                .map(str::to_owned)
                .collect();
            info.service_tier = selected.settings.service_tier;
            info.service_choices = selected
                .capabilities
                .service_tier_source
                .map(|_| selected.capabilities.service_tiers);
            Some(info)
        })
        .collect()
}
