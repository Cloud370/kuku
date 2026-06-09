use std::path::Path;
use std::sync::Arc;

use crate::error::Result;
use crate::query::PermissionMode;

use super::definition::AgentDefinition;

#[allow(clippy::too_many_arguments, dead_code)]
pub async fn start_delegated_run(
    _parent_session_dir: &Path,
    nested_session_id: &str,
    definition: &AgentDefinition,
    delegated_prompt: &str,
    workspace: &Path,
    kuku_home: &Path,
    config: Arc<crate::config::Config>,
    prompts_dir: Option<&Path>,
    _parent_mode: PermissionMode,
    _delegation_depth: u32,
) -> Result<crate::query::Run> {
    let definition_block = super::catalog::render_agent_definition_block(definition);
    let user_prompt = format!(
        "{definition_block}\n\n<kuku_delegated_prompt>\n{delegated_prompt}\n</kuku_delegated_prompt>"
    );

    let full_registry = crate::tool::builtin_registry(false, false);
    let constrained_registry: Vec<crate::tool::ToolDefinition> = match &definition.tools {
        None => full_registry,
        Some(list) => full_registry
            .into_iter()
            .filter(|t| list.contains(&t.name))
            .collect(),
    };

    let mut query = crate::query::Query::new(user_prompt)
        .workspace(workspace.to_path_buf())
        .tier(definition.tier.clone())
        .config((*config).clone())
        .no_agents();

    query.captured_kuku_home = Some(kuku_home.to_path_buf());
    query.tool_registry_override = Some(constrained_registry);

    if let Some(dir) = prompts_dir {
        query = query.prompts_dir(dir.to_path_buf());
    }

    query.session(nested_session_id.to_string()).start().await
}
