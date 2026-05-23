use std::path::Path;
use std::sync::Arc;

use crate::error::Result;
use crate::query::PermissionMode;

use super::definition::SubagentDefinition;

/// Start a child session and return the Run handle without blocking.
/// For TUI/interactive use — the caller polls child events via Run::next().
// Each parameter comes from a different source (config, registry, caller, filesystem);
// bundling them into a struct would obscure which call sites provide which data.
#[allow(clippy::too_many_arguments)]
pub async fn start_child_session(
    _parent_session_dir: &Path,
    child_session_id: &str,
    definition: &SubagentDefinition,
    delegated_prompt: &str,
    workspace: &Path,
    _kuku_home: &Path,
    config: Arc<crate::config::Config>,
    prompts_dir: Option<&Path>,
    _parent_mode: PermissionMode,
    _child_session_count: u32,
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

    query.tool_registry_override = Some(constrained_registry);

    if let Some(dir) = prompts_dir {
        query = query.prompts_dir(dir.to_path_buf());
    }

    query.session(child_session_id.to_string()).start().await
}
