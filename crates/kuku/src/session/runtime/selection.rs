use std::collections::BTreeMap;

use crate::adapter::{AgentSelection, RegisteredTool};
use crate::provider::config::{ResolveConfigInput, resolve_config};
use crate::provider::types::ResolvedProvider;
use crate::tool::ToolDefinition;
use crate::{Error, PromptCatalog, Result};

use super::{AgentConfig, Session, SessionOptions};

pub(super) fn register_tools(options: &SessionOptions) -> Result<BTreeMap<String, RegisteredTool>> {
    if options.tools.len() > 128 {
        return Err(Error::InvalidArgument(
            "at most 128 host tools are supported".into(),
        ));
    }
    let reserved = crate::tool::builtin_registry(true)
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
            || crate::util::json::write_bounded_json(&spec.input_schema, 65_536, std::io::sink())
                .is_err()
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

pub(crate) fn resolve_agent_model(
    host_config: &crate::config::Config,
    config: &AgentConfig,
    max_output_tokens_per_request: Option<u32>,
) -> Result<ResolvedProvider> {
    resolve_config(ResolveConfigInput {
        connection: config.connection.clone(),
        model: config.model.clone(),
        tier: config.tier.clone(),
        reasoning_effort: config.reasoning_effort.clone(),
        output: config.output,
        service_tier: config.service_tier.clone(),
        max_output_tokens: config.max_output_tokens,
        max_output_tokens_per_request,
        config: Some(host_config.clone()),
        ..Default::default()
    })
}

pub(super) fn validate_initial_model(options: &SessionOptions, config: &AgentConfig) -> Result<()> {
    let config = resolve_defaults(options, config.clone(), || Ok(None))?;
    resolve_agent_model(
        &options.config,
        &config,
        options.limits.max_output_tokens_per_request,
    )
    .map(|_| ())
}

impl Session {
    pub(crate) fn workspace_for<'a>(&'a self, config: &'a AgentConfig) -> &'a std::path::Path {
        config
            .workspace
            .as_deref()
            .unwrap_or(&self.inner.options.workspace)
    }

    /// Inspect model settings available locally and final tools without dispatching work.
    pub fn agent_selection(&self, address: &str) -> Result<AgentSelection> {
        let agent = self.agent(address)?;
        self.selection_for_config(address, agent.active_config.unwrap_or(agent.config))
    }

    /// Inspect model settings available locally from an Agent snapshot without dispatching work.
    pub fn selection_for_config(
        &self,
        address: &str,
        config: AgentConfig,
    ) -> Result<AgentSelection> {
        let config = self.resolve_defaults(address, config)?;
        let resolved = self.resolve_model(&config)?;
        let tools = crate::tool::to_tool_schemas(&self.selected_tools(&config, &resolved)?);
        Ok(AgentSelection {
            request_url: crate::provider::prepare::request_url(&resolved)
                .map_err(|failure| Error::InvalidArgument(failure.message))?,
            connection: resolved.connection_name,
            protocol: crate::adapter::protocol(resolved.kind),
            model: resolved.model,
            context_window_tokens: resolved.max_context_tokens,
            max_output_tokens: resolved.max_output_tokens,
            settings: resolved.settings,
            capabilities: resolved.capabilities,
            output: resolved.output,
            tools,
        })
    }

    pub(super) fn resolve_defaults(
        &self,
        address: &str,
        config: AgentConfig,
    ) -> Result<AgentConfig> {
        if config.instructions.is_some() && config.tier.is_some() && config.tools.is_some() {
            return Ok(config);
        }
        self.resolve_agent_preset(address, config)
    }

    pub(super) fn resolve_agent_preset(
        &self,
        address: &str,
        config: AgentConfig,
    ) -> Result<AgentConfig> {
        resolve_defaults(&self.inner.options, config, || {
            crate::context::runtime::read_frozen_catalog(&self.inner.events_path, address)
        })
    }

    pub(super) fn selected_tools(
        &self,
        config: &AgentConfig,
        resolved: &ResolvedProvider,
    ) -> Result<Vec<ToolDefinition>> {
        let disabled_collaboration_tool = |name: &str| {
            config.no_agents && matches!(name, "create_agent" | "send_message" | "list_agents")
        };
        let available = crate::tool::registry::session_registry(!config.no_skills)
            .into_iter()
            .chain(crate::query::runtime_tool_definitions())
            .chain(
                self.inner
                    .tool_adapters
                    .values()
                    .map(|tool| tool.spec.definition()),
            )
            .filter(|tool| tool.name != "fetch_web")
            .filter(|tool| !disabled_collaboration_tool(tool.name.as_str()))
            .filter(|tool| {
                crate::query::runtime_tool_supported(
                    resolved,
                    self.inner.options.model_adapter.is_none(),
                    &tool.name,
                )
            })
            .collect::<Vec<_>>();
        let mut selected_tools = if let Some(selected) = &config.tools {
            for name in selected {
                if disabled_collaboration_tool(name) {
                    return Err(Error::InvalidArgument(format!(
                        "no_agents disables '{name}'. Set no_agents to false or remove '{name}' from tools."
                    )));
                }
                if !available.iter().any(|tool| tool.name == *name) {
                    return Err(Error::InvalidArgument(format!(
                        "selected tool '{name}' is not available"
                    )));
                }
            }
            available
                .into_iter()
                .filter(|tool| selected.contains(&tool.name))
                .collect()
        } else {
            available
        };
        crate::tool::run_js::describe_selected_tools(&mut selected_tools);
        Ok(selected_tools)
    }

    pub(crate) fn current_model_config(&self) -> std::borrow::Cow<'_, crate::config::Config> {
        self.inner.options.model_config()
    }

    pub(super) fn resolve_model(&self, config: &AgentConfig) -> Result<ResolvedProvider> {
        resolve_agent_model(
            &self.current_model_config(),
            config,
            self.inner.options.limits.max_output_tokens_per_request,
        )
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
        let resolved = self.resolve_model(config)?;
        self.selected_tools(config, &resolved)?;
        Ok(())
    }
}

fn resolve_defaults(
    options: &SessionOptions,
    mut config: AgentConfig,
    frozen_catalog: impl FnOnce() -> Result<Option<PromptCatalog>>,
) -> Result<AgentConfig> {
    let Some(preset) = &config.preset else {
        return Ok(config);
    };
    let catalog = match frozen_catalog()? {
        Some(catalog) => catalog,
        None => match &options.prompts_dir {
            Some(dir) => PromptCatalog::load_from_dir(dir)?,
            None => crate::builtin_prompt_catalog(),
        },
    };
    let registry = crate::agent::registry::AgentRegistry::builder()
        .builtins(&catalog)
        .build_with_discovery(
            config.workspace.as_ref().unwrap_or(&options.workspace),
            &options.config.discovery,
        )?
        .build();
    let definition = registry.get(preset).ok_or_else(|| {
        let guidance = if registry.is_empty() {
            "No Agent presets are available. Omit preset to use defaults.".into()
        } else {
            let examples = registry
                .names()
                .iter()
                .take(8)
                .map(String::as_str)
                .collect::<Vec<_>>();
            format!(
                "Available examples: {}. Omit preset to use defaults.",
                examples.join(", ")
            )
        };
        Error::InvalidArgument(format!("preset '{preset}' was not found. {guidance}"))
    })?;
    config
        .instructions
        .get_or_insert_with(|| definition.instructions.clone());
    if config.tier.is_none() && options.model_config().tier(&definition.tier).is_some() {
        config.tier = Some(definition.tier.clone());
    }
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
            .filter(|name| name != "fetch_web")
            .collect()
    });
    Ok(config)
}
