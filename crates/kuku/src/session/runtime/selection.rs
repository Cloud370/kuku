use std::collections::BTreeMap;

use crate::adapter::{AgentSelection, RegisteredTool};
use crate::provider::config::{resolve_config, ResolveConfigInput};
use crate::provider::types::ResolvedProvider;
use crate::tool::ToolDefinition;
use crate::{Error, Result};

use super::{AgentConfig, Session, SessionOptions};

pub(super) fn register_tools(options: &SessionOptions) -> Result<BTreeMap<String, RegisteredTool>> {
    if options.tools.len() > 128 {
        return Err(Error::InvalidArgument(
            "at most 128 host tools are supported".into(),
        ));
    }
    let reserved = crate::tool::builtin_registry(true, true)
        .into_iter()
        .chain(crate::query::runtime_tool_definitions())
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    let mut tools = BTreeMap::new();
    for adapter in &options.tools {
        let spec = adapter.definition();
        if spec.name.is_empty()
            || spec.name.len() > 128
            || !spec
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
            || spec.description.len() > 16_384
            || !spec.input_schema.is_object()
            || serde_json::to_vec(&spec.input_schema)?.len() > 65_536
            || !(1024..=65_536).contains(&spec.max_result_bytes)
        {
            return Err(Error::InvalidArgument(format!(
                "invalid host tool definition '{}'",
                spec.name
            )));
        }
        if reserved.contains(&spec.name) || tools.contains_key(&spec.name) {
            return Err(Error::InvalidArgument(format!(
                "duplicate or reserved host tool '{}'",
                spec.name
            )));
        }
        tools.insert(
            spec.name.clone(),
            RegisteredTool {
                spec,
                adapter: adapter.clone(),
            },
        );
    }
    Ok(tools)
}

impl Session {
    /// Inspect effective model settings and final tools without dispatching work.
    pub fn agent_selection(&self, address: &str) -> Result<AgentSelection> {
        let config = self.resolve_defaults(address, self.agent(address)?.config)?;
        let resolved = self.resolve_model(&config)?;
        let tools = crate::tool::to_tool_schemas(&self.selected_tools(&config)?);
        Ok(AgentSelection {
            connection: resolved.connection_name,
            protocol: crate::adapter::protocol(resolved.kind),
            model: resolved.model,
            context_window_tokens: resolved.max_context_tokens,
            max_output_tokens: resolved.max_output_tokens,
            settings: resolved.settings,
            capabilities: resolved.capabilities,
            tools,
        })
    }

    pub(super) fn resolve_defaults(
        &self,
        address: &str,
        mut config: AgentConfig,
    ) -> Result<AgentConfig> {
        let Some(preset) = &config.preset else {
            return Ok(config);
        };
        if config.instructions.is_some() && config.tier.is_some() && config.tools.is_some() {
            return Ok(config);
        }
        let catalog =
            match crate::context::runtime::read_frozen_catalog(&self.inner.events_path, address)? {
                Some(catalog) => catalog,
                None => match &self.inner.options.prompts_dir {
                    Some(dir) => crate::PromptCatalog::load_from_dir(dir)?,
                    None => crate::builtin_prompt_catalog(),
                },
            };
        let registry = crate::agent::registry::AgentRegistry::builder()
            .builtins(&catalog)
            .build_with_discovery(
                config
                    .workspace
                    .as_ref()
                    .unwrap_or(&self.inner.options.workspace),
                &self.inner.options.config.discovery,
            )?
            .build();
        let definition = registry
            .get(preset)
            .ok_or_else(|| Error::InvalidArgument(format!("preset '{preset}' was not found")))?;
        config
            .instructions
            .get_or_insert_with(|| definition.instructions.clone());
        config.tier.get_or_insert_with(|| definition.tier.clone());
        config.tools.get_or_insert_with(|| {
            definition
                .tools
                .clone()
                .unwrap_or_else(|| {
                    definition
                        .tool_profile
                        .allowed_tools()
                        .iter()
                        .map(|tool| (*tool).into())
                        .collect()
                })
                .into_iter()
                .filter(|name| !matches!(name.as_str(), "agent" | "fetch_web"))
                .collect()
        });
        Ok(config)
    }

    pub(super) fn selected_tools(&self, config: &AgentConfig) -> Result<Vec<ToolDefinition>> {
        let available = crate::tool::builtin_registry(false, !config.no_skills)
            .into_iter()
            .chain(crate::query::runtime_tool_definitions())
            .chain(
                self.inner
                    .tool_adapters
                    .values()
                    .map(|tool| tool.spec.definition()),
            )
            .filter(|tool| !matches!(tool.name.as_str(), "agent" | "fetch_web"))
            .filter(|tool| {
                !config.no_agents
                    || !matches!(
                        tool.name.as_str(),
                        "create_agent" | "send_message" | "list_agents"
                    )
            })
            .collect::<Vec<_>>();
        let Some(selected) = &config.tools else {
            return Ok(available);
        };
        for name in selected {
            if !available.iter().any(|tool| tool.name == *name) {
                return Err(Error::InvalidArgument(format!(
                    "selected tool '{name}' is not available"
                )));
            }
        }
        Ok(available
            .into_iter()
            .filter(|tool| selected.contains(&tool.name))
            .collect())
    }

    pub(super) fn resolve_model(&self, config: &AgentConfig) -> Result<ResolvedProvider> {
        let mut resolved = resolve_config(ResolveConfigInput {
            connection: config.connection.clone(),
            model: config.model.clone(),
            tier: config.tier.clone(),
            reasoning_effort: config.reasoning_effort.clone(),
            service_tier: config.service_tier.clone(),
            max_output_tokens: config.max_output_tokens,
            config: Some(self.inner.options.config.clone()),
            ..Default::default()
        })?;
        if let Some(limit) = self.inner.options.limits.max_output_tokens_per_request {
            resolved.max_output_tokens = resolved.max_output_tokens.min(limit);
        }
        Ok(resolved)
    }

    pub(super) fn validate_config(&self, config: &AgentConfig) -> Result<()> {
        if config
            .instructions
            .as_ref()
            .is_some_and(|value| value.len() > 65_536)
            || [
                &config.preset,
                &config.connection,
                &config.tier,
                &config.model,
                &config.reasoning_effort,
                &config.service_tier,
                &config.bootstrap_skill,
            ]
            .iter()
            .any(|value| value.as_ref().is_some_and(|value| value.len() > 512))
        {
            return Err(Error::InvalidArgument(
                "agent instructions or model selection exceeds its byte limit".into(),
            ));
        }
        if config.tools.as_ref().is_some_and(|tools| {
            tools.len() > 128 || tools.iter().any(|name| name.is_empty() || name.len() > 128)
        }) {
            return Err(Error::InvalidArgument(
                "tool selection exceeds its supported bounds".into(),
            ));
        }
        if config.max_output_tokens == Some(0) {
            return Err(Error::InvalidArgument(
                "max_output_tokens must be positive".into(),
            ));
        }
        if let Some(workspace) = &config.workspace {
            std::fs::canonicalize(workspace)
                .map_err(|_| Error::InvalidWorkspacePath(workspace.display().to_string()))?;
        }
        self.resolve_model(config)?;
        self.selected_tools(config)?;
        Ok(())
    }
}
