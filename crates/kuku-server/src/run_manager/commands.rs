use kuku::execution::OutputReference;
use kuku::session::{AgentConfig, AgentStatus, Input, InputStatus, Receipt, Session};
use sha2::{Digest, Sha256};

use super::{CommandExecution, OpenSession, RunManager};
use crate::error_mapping::{ApiError, Result};
use crate::wire::{http_input_source, Command, CommandResult};

pub(crate) const INTERNAL_CREATE_PREFIX: &str = "http:agent:";

impl RunManager {
    pub(super) async fn execute_command(
        &self,
        open: &mut OpenSession,
        request: CommandExecution<'_>,
    ) -> Result<CommandResult> {
        let CommandExecution {
            address,
            command_id,
            command,
            input,
            replay_only,
        } = request;
        let replayed = open.session.receipt(command_id)?.is_some();
        if replay_only && !replayed {
            return Err(ApiError::receipt_not_found());
        }
        let original_input = input
            .as_ref()
            .map(|input| {
                open.session
                    .input_receipt_with_values(input, command.values())
            })
            .transpose()?
            .flatten();
        if replay_only {
            if let Some(receipt) = original_input {
                let status = input_status(&open.session, &receipt);
                return Ok(CommandResult::new(receipt, true, status));
            }
        }
        let deliverable = original_input.as_ref().is_none_or(|receipt| {
            input_status(&open.session, receipt) == Some(InputStatus::Queued)
        });
        if deliverable
            && (!replayed || input.is_some())
            && activating(&command, &open.session, address)
            && !open.permits.contains_key(address)
        {
            let permit = self.acquire_permit()?;
            open.permits.insert(address.to_owned(), permit);
        }
        let result = match command {
            Command::Send {
                create,
                config_patch,
                create_agent_if_missing,
                values,
                ..
            } => send(
                &open.session,
                input.expect("send has input"),
                create.as_deref(),
                config_patch.as_deref(),
                create_agent_if_missing,
                replayed,
                &values,
            ),
            Command::CreateAgent { config } => {
                open.session.create_agent(command_id, address, *config)
            }
            Command::Configure { config } => {
                open.session.configure_agent(command_id, address, *config)
            }
            Command::ReplyQuestion { request_id, text } => {
                open.session
                    .reply_question(command_id, address, &request_id, &text)
            }
            Command::ReplyPermission { request_id, allow } => {
                open.session
                    .reply_permission(command_id, address, &request_id, allow)
            }
            Command::Withdraw { input_command_id } => {
                let withdrawal =
                    open.session
                        .withdraw_input(command_id, address, &input_command_id)?;
                return Ok(CommandResult::new(
                    withdrawal.receipt,
                    replayed,
                    Some(withdrawal.status),
                ));
            }
            Command::ReplaceQueued {
                input_command_id,
                text,
            } => open.session.replace_queued(
                command_id,
                address,
                &input_command_id,
                &text,
                &http_input_source(),
            ),
            Command::RewindMessage {
                target_event_id,
                text,
                restore_files,
            } => {
                let (receipt, warnings) = open.session.rewind_message(
                    command_id,
                    address,
                    target_event_id,
                    &text,
                    restore_files,
                )?;
                return Ok(CommandResult::new(receipt, replayed, None).with_warnings(warnings));
            }
            Command::Compact => open.session.compact_command(command_id, address),
            Command::RestoreContext => open.session.restore_context_command(command_id, address),
            Command::Advance => open.session.advance_command(command_id, address),
            Command::Cancel { execution_id } => {
                open.session
                    .cancel_execution(command_id, address, execution_id)
                    .await
            }
        };
        open.release_idle_permits();
        let receipt = result?;
        let status = input_status(&open.session, &receipt);
        Ok(CommandResult::new(receipt, replayed, status))
    }
}

pub(crate) fn input_status(session: &Session, receipt: &Receipt) -> Option<InputStatus> {
    session
        .input_status(&receipt.agent, &receipt.command_id)
        .ok()
}

fn activating(command: &Command, session: &Session, address: &str) -> bool {
    let agent = session.agent(address).ok();
    match command {
        Command::Send { .. } => agent
            .as_ref()
            .is_none_or(|agent| matches!(agent.status, AgentStatus::Idle | AgentStatus::Stopped)),
        Command::ReplyPermission { request_id, .. } => agent.is_some_and(|agent| {
            agent.status == AgentStatus::Waiting
                && agent.question.is_none()
                && agent
                    .permission
                    .is_some_and(|request| request.id == *request_id)
        }),
        Command::ReplyQuestion { request_id, .. } => agent.is_some_and(|agent| {
            agent.status == AgentStatus::Waiting
                && agent.permission.is_none()
                && agent
                    .question
                    .is_some_and(|request| request.id == *request_id)
        }),
        Command::Advance => agent.is_some_and(|agent| {
            matches!(
                agent.status,
                AgentStatus::Idle | AgentStatus::Stopped | AgentStatus::Interrupted
            ) && agent.pending_inputs > 0
        }),
        Command::Compact => agent.is_some_and(|agent| {
            matches!(agent.status, AgentStatus::Idle | AgentStatus::Interrupted)
                && agent.pending_inputs == 0
        }),
        Command::CreateAgent { .. }
        | Command::Configure { .. }
        | Command::Withdraw { .. }
        | Command::ReplaceQueued { .. }
        | Command::RewindMessage { .. }
        | Command::RestoreContext
        | Command::Cancel { .. } => false,
    }
}

fn send(
    session: &Session,
    input: Input,
    create: Option<&AgentConfig>,
    patch: Option<&AgentConfig>,
    create_agent_if_missing: bool,
    replayed: bool,
    values: &[OutputReference],
) -> kuku::Result<Receipt> {
    if replayed {
        return session.send_with_values(input, values);
    }
    let address = input.target.clone();
    let current = session.agent(&address);
    let initial = create.cloned().or_else(|| {
        (create_agent_if_missing && current.is_err()).then(|| patch.cloned().unwrap_or_default())
    });
    if let Some(config) = initial {
        let internal_id = format!(
            "{INTERNAL_CREATE_PREFIX}{:x}",
            Sha256::digest(input.command_id.as_bytes())
        );
        session.create_agent(&internal_id, &address, config)?;
    }
    let current = session.agent(&address)?;
    match patch.filter(|_| create.is_none()) {
        Some(patch) => session.send_configured_with_values(
            input,
            merge_config(current.config, patch.clone()),
            values,
        ),
        None => session.send_with_values(input, values),
    }
}

fn merge_config(mut current: AgentConfig, update: AgentConfig) -> AgentConfig {
    if update.tier.is_some() || update.connection.is_some() || update.model.is_some() {
        current.reasoning_effort = None;
        current.service_tier = None;
    }
    if update.connection.is_some() {
        current.connection = update.connection;
    }
    if update.tier.is_some() {
        current.tier = update.tier;
        current.model = None;
    }
    if update.model.is_some() {
        current.model = update.model;
    }
    if update.reasoning_effort.is_some() {
        current.reasoning_effort = update.reasoning_effort;
    }
    if update.output.is_some() {
        current.output = update.output;
    }
    if update.service_tier.is_some() {
        current.service_tier = update.service_tier;
    }
    if update.preset.is_some() {
        current.preset = update.preset;
    }
    if update.instructions.is_some() {
        current.instructions = update.instructions;
    }
    if update.workspace.is_some() {
        current.workspace = update.workspace;
    }
    if update.tools.is_some() {
        current.tools = update.tools;
    }
    if update.max_output_tokens.is_some() {
        current.max_output_tokens = update.max_output_tokens;
    }
    current.bootstrap_skill = update.bootstrap_skill;
    current.no_agents |= update.no_agents;
    current.no_skills |= update.no_skills;
    current
}
