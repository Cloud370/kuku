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
        config: Some((*pending.config).clone()),
        connection: pending.query.connection.clone(),
        reasoning_effort: pending.query.reasoning_effort.clone(),
        service_tier: pending.query.service_tier.clone(),
    }) {
        Ok(config) => config,
        Err(error) => {
            let request_id = format!(
                "req_{}",
                EventStore::replay(&pending.events_path)?.len() + 1
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
    } else {
        tool::builtin_registry(!pending.query.disable_agents, !pending.query.disable_skills)
    };
    if pending.query.runtime.is_some() {
        registry.retain(|tool| !matches!(tool.name.as_str(), "agent" | "fetch_web"));
        registry.extend(
            crate::query::runtime_tools::definitions()
                .into_iter()
                .filter(|tool| !pending.query.disable_agents || tool.name == "ask_user"),
        );
    }
    pending.resolved = Some(ResolvedRuntime { config, registry });
    Ok(())
}
