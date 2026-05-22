use std::path::Path;
use std::sync::Arc;

use crate::error::Result;
use crate::query::PermissionMode;

use super::definition::SubagentDefinition;

/// Result of a completed child session.
#[derive(Debug, Clone)]
pub struct ChildSessionResult {
    pub child_session_id: String,
    pub output_text: String,
    pub turns_completed: u32,
    pub status: ChildSessionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildSessionStatus {
    Completed,
    TurnLimitReached,
    Error(String),
}

// Each parameter comes from a different source (config, registry, caller, filesystem);
// bundling them into a struct would obscure which call sites provide which data.
#[allow(clippy::too_many_arguments)]
pub async fn spawn_child_session(
    parent_session_dir: &Path,
    child_session_id: &str,
    definition: &SubagentDefinition,
    delegated_prompt: &str,
    workspace: &Path,
    _kuku_home: &Path,
    config: Arc<crate::config::Config>,
    prompts_dir: Option<&Path>,
    parent_mode: PermissionMode,
) -> Result<ChildSessionResult> {
    let subs_dir = parent_session_dir.join("subs");
    std::fs::create_dir_all(&subs_dir)?;

    let definition_block = super::catalog::render_agent_definition_block(definition);
    let user_prompt = format!(
        "{definition_block}\n\n<kuku_delegated_prompt>\n{delegated_prompt}\n</kuku_delegated_prompt>"
    );

    // Build constrained tool registry from definition.tools
    let full_registry = crate::tool::builtin_registry(false, false);
    let constrained_registry: Vec<crate::tool::ToolDefinition> = match &definition.tools {
        None => full_registry,
        Some(list) => full_registry
            .into_iter()
            .filter(|t| list.contains(&t.name))
            .collect(),
    };

    // Query builder with overrides
    let mut query = crate::query::Query::new(user_prompt)
        .workspace(workspace.to_path_buf())
        .tier(definition.tier.clone())
        .config((*config).clone())
        .no_agents();

    query.tool_registry_override = Some(constrained_registry);

    if let Some(dir) = prompts_dir {
        query = query.prompts_dir(dir.to_path_buf());
    }

    let mut run = query.session(child_session_id.to_string()).start().await?;

    let mut turns = 0u32;
    let mut cumulative_text = String::new();

    loop {
        turns += 1;
        if turns > definition.max_turns {
            return Ok(ChildSessionResult {
                child_session_id: child_session_id.to_string(),
                output_text: cumulative_text,
                turns_completed: turns - 1,
                status: ChildSessionStatus::TurnLimitReached,
            });
        }

        match run.next().await? {
            Some(crate::UiEvent::Done { output, .. }) => {
                return Ok(ChildSessionResult {
                    child_session_id: child_session_id.to_string(),
                    output_text: output.text,
                    turns_completed: turns,
                    status: ChildSessionStatus::Completed,
                });
            }
            Some(crate::UiEvent::TextDelta { text }) => {
                cumulative_text.push_str(&text);
            }
            Some(crate::UiEvent::PermissionRequested { request }) => match parent_mode {
                PermissionMode::AutoAllow => {
                    run.decide(request.id, crate::query::PermissionChoice::Once)
                        .await?;
                }
                PermissionMode::Interactive => {
                    return Err(crate::error::Error::ChildPermissionRequested {
                        tool: request.tool,
                        candidate: request.summary,
                    });
                }
            },
            None => {
                return Ok(ChildSessionResult {
                    child_session_id: child_session_id.to_string(),
                    output_text: cumulative_text,
                    turns_completed: turns,
                    status: ChildSessionStatus::Error("stream ended unexpectedly".into()),
                });
            }
            // Child session does not need to act on these events
            Some(crate::UiEvent::ThinkingDelta { .. })
            | Some(crate::UiEvent::ToolCall { .. })
            | Some(crate::UiEvent::ToolResult { .. })
            | Some(crate::UiEvent::TurnStart { .. })
            | Some(crate::UiEvent::Error { .. })
            | Some(crate::UiEvent::ModelRequest { .. }) => continue,
        }
    }
}
