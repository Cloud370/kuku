use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use kuku::session::{AgentConfig, Session};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Params;
use super::{SessionTarget, WorkspaceQuery, manager, session_options};
use crate::AppState;
use crate::error_mapping::{ApiError, Result};
use crate::run_manager::{INTERNAL_CREATE_PREFIX, input_status};
use crate::wire::{Command, CommandRequest, CommandResult};

#[derive(Deserialize)]
pub(super) struct SubmitQuery {
    #[serde(default)]
    replay_only: bool,
}

pub(super) async fn submit(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Params(query): Params<SubmitQuery>,
    body: std::result::Result<Json<CommandRequest>, JsonRejection>,
) -> Result<Json<CommandResult>> {
    let Json(mut body) = body.map_err(|error| ApiError::invalid(error.body_text()))?;
    validate_command_id(&body.command_id)?;
    let target = SessionTarget::new(&state, &body.workspace, &id)?;
    let mut options = session_options(&state, target.workspace, id);
    options.prompts_dir = validate_command(&options.workspace, &mut body.command)?;
    let result = manager(&state)
        .await
        .command(
            options,
            &body.agent,
            &body.command_id,
            body.command,
            query.replay_only,
        )
        .await?;
    Ok(Json(result))
}

pub(super) async fn receipt(
    State(state): State<Arc<AppState>>,
    Path((id, command_id)): Path<(String, String)>,
    Params(query): Params<WorkspaceQuery>,
) -> Result<Json<Value>> {
    validate_command_id(&command_id)?;
    let target = SessionTarget::new(&state, &query.workspace, &id)?;
    if !target.path.is_file() {
        return Err(ApiError::receipt_not_found());
    }
    let found = if let Some(session) = manager(&state).await.open_session(&target.path) {
        session.receipt(&command_id)?.map(|receipt| {
            let status = input_status(&session, &receipt);
            (receipt, status)
        })
    } else {
        Session::read_receipt(&target.path, &command_id)?
    };
    let (receipt, status) = found.ok_or_else(ApiError::receipt_not_found)?;
    let mut value = json!({"ok":true,"receipt":receipt});
    if let Some(status) = status {
        value["status"] = json!(status);
    }
    Ok(Json(value))
}

fn validate_command_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || id.chars().any(char::is_control)
        || id.contains('/')
        || id.contains('\\')
        || matches!(id, "." | "..")
        || id.starts_with(INTERNAL_CREATE_PREFIX)
    {
        return Err(ApiError::invalid("invalid command identity"));
    }
    Ok(())
}

fn validate_command(workspace: &FsPath, command: &mut Command) -> Result<Option<PathBuf>> {
    match command {
        Command::Send {
            text,
            values,
            images,
            create,
            config_patch,
            create_agent_if_missing,
            prompts_dir,
            bootstrap_skill,
            ..
        } => {
            let bootstrap = bootstrap_skill.is_some()
                || create
                    .as_ref()
                    .or(config_patch.as_ref())
                    .is_some_and(|config| config.bootstrap_skill.is_some());
            if (text.trim().is_empty() && !bootstrap && values.is_empty() && images.is_empty())
                || text.len() > kuku::RuntimeLimits::default().max_input_bytes
            {
                return Err(ApiError::invalid(
                    "input text is empty or exceeds the byte limit",
                ));
            }
            if let Some(config) = create {
                if config_patch.is_some() || *create_agent_if_missing || !values.is_empty() {
                    return Err(ApiError::invalid(
                        "initial creation accepts its initial Agent configuration and no retained Session values",
                    ));
                }
                validate_agent_workspace(workspace, config)?;
            }
            if let Some(config) = config_patch {
                validate_agent_workspace(workspace, config)?;
            }
            if let Some(prompts_dir) = prompts_dir {
                if create.is_none() {
                    return Err(ApiError::invalid(
                        "prompts_dir is selected with initial Session creation",
                    ));
                }
                return super::workspaces::resolve_workspace(prompts_dir).map(Some);
            }
        }
        Command::Configure { config } | Command::CreateAgent { config } => {
            validate_agent_workspace(workspace, config)?;
        }
        Command::ReplaceQueued {
            input_command_id,
            text,
        } => {
            validate_command_id(input_command_id)?;
            if text.trim().is_empty() || text.len() > kuku::RuntimeLimits::default().max_input_bytes
            {
                return Err(ApiError::invalid(
                    "replacement text is empty or exceeds the byte limit",
                ));
            }
        }
        Command::ReplyQuestion { .. }
        | Command::ReplyPermission { .. }
        | Command::Withdraw { .. }
        | Command::RewindMessage { .. }
        | Command::HistoryJump { .. }
        | Command::Compact
        | Command::RestoreContext
        | Command::Advance
        | Command::Cancel { .. } => {}
    }
    Ok(None)
}

pub(super) fn validate_agent_workspace(workspace: &FsPath, config: &mut AgentConfig) -> Result<()> {
    if let Some(requested) = config.workspace.as_mut() {
        let absolute = workspace.join(&*requested);
        let resolved = super::workspaces::resolve_workspace(&absolute)?;
        if !requested.is_absolute() && !resolved.starts_with(workspace) {
            return Err(ApiError::invalid(
                "Agent workspace must be inside the selected Session workspace",
            ));
        }
        *requested = resolved;
    }
    Ok(())
}
