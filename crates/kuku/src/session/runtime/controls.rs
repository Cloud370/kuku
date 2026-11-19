use std::collections::BTreeSet;

use crate::event::{EventPayload, EventStore};
use crate::{Error, Result};

use super::record::{
    fingerprint, timestamp, validate_address, validate_key, AgentState, Command,
    InterruptedExecution, Record, State,
};
use super::{ensure_open, AgentStatus, InputStatus, Receipt, Session, Withdrawal};

impl Session {
    /// Hide a committed user message and the following visible history for one Agent.
    pub fn rewind_message(
        &self,
        command_id: &str,
        address: &str,
        target_event_id: u64,
        expected_text: &str,
        restore_files: bool,
    ) -> Result<(Receipt, Vec<String>)> {
        let command = command(
            command_id,
            address,
            &(
                "rewind_message",
                address,
                target_event_id,
                expected_text,
                restore_files,
            ),
        )?;
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        if let Some(receipt) = state.retry(command_id, &command.fingerprint)? {
            return Ok((
                receipt,
                state
                    .message_rewind_warnings
                    .get(command_id)
                    .cloned()
                    .unwrap_or_default(),
            ));
        }
        let agent = state.agent(address)?;
        if rewind_blocked(agent) {
            return Err(Error::InvalidArgument(
                "cannot rewind while this Agent is executing or has pending input".into(),
            ));
        }
        if restore_files && state.agents.values().any(rewind_blocked) {
            return Err(Error::InvalidArgument(
                "cannot restore files while any Session Agent is active".into(),
            ));
        }
        let events = EventStore::replay(&self.inner.events_path)?;
        let target = events
            .iter()
            .find(|event| event.id == target_event_id)
            .ok_or_else(|| Error::InvalidArgument("rewind target is no longer available".into()))?;
        let EventPayload::MessageUser {
            conversation,
            turn,
            text,
            ..
        } = &target.payload
        else {
            return Err(Error::InvalidArgument(
                "rewind target is not a committed user message".into(),
            ));
        };
        if conversation != address
            || text != expected_text
            || !crate::context::revert::filter_rolled_back_events(&events)
                .iter()
                .any(|event| event.id == target_event_id)
        {
            return Err(Error::InvalidArgument(
                "rewind target belongs to another Agent or is no longer visible".into(),
            ));
        }
        let mut warnings = Vec::new();
        if restore_files {
            let plan = crate::context::revert::compute_message_rewind_plan(
                &events,
                address,
                target_event_id,
                &self.inner.options.workspace,
            )?;
            if !plan.unrecoverable.is_empty() {
                return Err(Error::InvalidArgument(format!(
                    "file restore preflight could not recover recorded state for {}",
                    plan.unrecoverable
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            let session_dir = self.inner.events_path.parent().ok_or_else(|| {
                Error::InvalidArgument("events path has no session directory".into())
            })?;
            let rollback_event_id = events.last().map_or(1, |event| event.id.saturating_add(1));
            warnings = crate::context::revert::apply_file_revert(
                &plan,
                &self.inner.options.workspace,
                session_dir,
                rollback_event_id,
            )?;
            warnings.push(
                "Only recorded in-workspace file changes were restored; shell and external side effects remain.".into(),
            );
            warnings.extend(
                plan.sensitive_files
                    .iter()
                    .map(|path| format!("Sensitive file restored: {}", path.display())),
            );
        }
        self.commit(
            &mut state,
            Record::MessageRewind {
                command,
                address: address.into(),
                target_event_id,
                target_turn: *turn,
                restore_files,
                warnings: warnings.clone(),
            },
        )?;
        Ok((state.commands[command_id].1.clone(), warnings))
    }

    /// Explicitly authorize advancement of pending or interrupted work.
    pub fn advance(&self, address: &str) -> Result<()> {
        self.advance_with_command(address, None).map(|_| ())
    }

    /// Authorize advancement once for a caller-selected command identity.
    pub fn advance_command(&self, command_id: &str, address: &str) -> Result<Receipt> {
        let command = command(command_id, address, &("advance", address))?;
        let receipt = self.advance_with_command(address, Some(command))?;
        Ok(receipt.expect("advancement commits its caller-selected command"))
    }

    /// Stop the selected agent, clean up its active work, and retain pending inputs.
    pub async fn cancel(&self, address: &str) -> Result<()> {
        self.stop(address, None).await.map(|_| ())
    }

    /// Stop only the observed execution and await cleanup of its owned work.
    pub async fn cancel_execution(
        &self,
        command_id: &str,
        address: &str,
        execution_id: u64,
    ) -> Result<Receipt> {
        let command = command(
            command_id,
            address,
            &("cancel_execution", address, execution_id),
        )?;
        let receipt = self.stop(address, Some((command, execution_id))).await?;
        Ok(receipt.expect("execution cancellation commits its caller-selected command"))
    }

    /// Withdraw a still-queued input or return its durable state at the time of the command.
    pub fn withdraw_input(
        &self,
        command_id: &str,
        address: &str,
        input_command_id: &str,
    ) -> Result<Withdrawal> {
        validate_key(input_command_id)?;
        let command = command(
            command_id,
            address,
            &("withdraw_input", address, input_command_id),
        )?;
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        if let Some(receipt) = state.retry(command_id, &command.fingerprint)? {
            return Ok(Withdrawal {
                receipt,
                status: state.withdrawals[command_id],
            });
        }
        let status = match state.input_status(address, input_command_id)? {
            InputStatus::Queued | InputStatus::Withdrawn => InputStatus::Withdrawn,
            InputStatus::Accepted => InputStatus::Accepted,
        };
        self.commit(
            &mut state,
            Record::InputWithdrawal {
                command,
                address: address.into(),
                input_command_id: input_command_id.into(),
                status,
            },
        )?;
        Ok(Withdrawal {
            receipt: state.commands[command_id].1.clone(),
            status,
        })
    }

    /// Replace an unadmitted message from the trusted source in its original queue position.
    /// Repeating the same command returns its committed receipt without waking the agent.
    pub fn replace_queued(
        &self,
        command_id: &str,
        address: &str,
        input_command_id: &str,
        text: &str,
        source: &super::InputSource,
    ) -> Result<Receipt> {
        validate_key(input_command_id)?;
        let command = command(
            command_id,
            address,
            &("replace_queued", address, input_command_id, text, source),
        )?;
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        if let Some(receipt) = state.retry(command_id, &command.fingerprint)? {
            return Ok(receipt);
        }
        if text.trim().is_empty() || text.len() > self.inner.options.limits.max_input_bytes {
            return Err(Error::InvalidArgument(
                "replacement text is empty or exceeds the configured byte limit".into(),
            ));
        }
        if !matches!(
            state.input_status(address, input_command_id),
            Ok(InputStatus::Queued)
        ) {
            return Err(Error::InputNotQueued(input_command_id.into()));
        }
        let old = state
            .agents
            .get(address)
            .and_then(|agent| {
                agent
                    .queued_message_position(input_command_id)
                    .map(|index| &agent.inbox[index].input)
            })
            .ok_or_else(|| Error::InputNotQueued(input_command_id.into()))?;
        if &old.source != source {
            return Err(Error::InvalidArgument(format!(
                "input '{input_command_id}' belongs to a different source"
            )));
        }
        self.commit(
            &mut state,
            Record::InputReplacement {
                command,
                address: address.into(),
                input_command_id: input_command_id.into(),
                text: text.into(),
            },
        )?;
        Ok(state.commands[command_id].1.clone())
    }

    pub(super) fn resume(
        &self,
        state: &mut State,
        address: &str,
        command: Option<Command>,
    ) -> Result<()> {
        let agent = state.agent(address)?;
        let paused = matches!(
            agent.snapshot.status,
            AgentStatus::Stopped | AgentStatus::Interrupted
        );
        if paused && agent.active_epoch.is_some() {
            return Err(Error::InvalidArgument(format!(
                "agent '{address}' cleanup is still in progress"
            )));
        }
        if paused || command.is_some() {
            self.commit(
                state,
                Record::AgentResumed {
                    address: address.into(),
                    command,
                    restore: paused,
                },
            )?;
        }
        Ok(())
    }

    pub(super) fn settle_cancelled_turn(&self, address: &str, turn: u64) -> Result<()> {
        let mut cursor = turn;
        let mut open = BTreeSet::new();
        let mut dispatched = BTreeSet::new();
        let mut started = false;
        let mut terminal = false;
        let mut stop_event_id = None;
        loop {
            let page = EventStore::read_page(self.events_path(), cursor, 128)?;
            if page.is_empty() {
                break;
            }
            for event in page {
                cursor = event.id;
                if event.payload.is_explicit_stop(address, turn) {
                    stop_event_id = Some(event.id);
                }
                match event.payload {
                    EventPayload::ToolCall {
                        turn: target,
                        conversation,
                        tool_call_id,
                        ..
                    } if target == turn
                        && conversation.as_deref().is_none_or(|agent| agent == address) =>
                    {
                        open.insert(tool_call_id);
                    }
                    EventPayload::ToolResult {
                        turn: target,
                        conversation,
                        tool_call_id,
                        ..
                    } if target == turn
                        && conversation.as_deref().is_none_or(|agent| agent == address) =>
                    {
                        open.remove(&tool_call_id);
                    }
                    EventPayload::Unknown(value)
                        if value["kind"] == "runtime.tool_dispatched"
                            && value["turn"] == turn
                            && value["agent"] == address =>
                    {
                        if let Some(id) = value["tool_call_id"].as_str() {
                            dispatched.insert(id.to_owned());
                        }
                    }
                    EventPayload::TurnStarted {
                        conversation,
                        turn: target,
                        ..
                    } if conversation == address && target == turn => started = true,
                    EventPayload::TurnCompleted {
                        conversation,
                        turn: target,
                        ..
                    }
                    | EventPayload::TurnCancelled {
                        conversation,
                        turn: target,
                        ..
                    }
                    | EventPayload::TurnInterrupted {
                        conversation,
                        turn: target,
                        ..
                    } if conversation == address && target == turn => terminal = true,
                    _ => {}
                }
            }
        }
        let mut writer = EventStore::open(self.events_path())?;
        for id in open.difference(&dispatched) {
            writer.append(cancelled_tool_result(address, turn, id)?)?;
        }
        if let Some(id) = open.intersection(&dispatched).next() {
            return Err(Error::InterruptedOpenTool(format!("agent '{address}' tool call '{id}' from turn {turn} has no confirmed result after cancellation")));
        }
        if started && !terminal {
            writer.append(EventPayload::TurnCancelled {
                turn,
                ts: timestamp()?,
                conversation: address.into(),
                reason: "user_cancelled".into(),
                stop_event_id,
            })?;
        }
        Ok(())
    }

    fn advance_with_command(
        &self,
        address: &str,
        command: Option<Command>,
    ) -> Result<Option<Receipt>> {
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        if let Some(command) = &command {
            if let Some(receipt) = state.retry(&command.command_id, &command.fingerprint)? {
                return Ok(Some(receipt));
            }
        }
        let key = command.as_ref().map(|command| command.command_id.clone());
        self.resume(&mut state, address, command)?;
        self.schedule_locked(&mut state, address)?;
        Ok(key.map(|key| state.commands[&key].1.clone()))
    }

    async fn stop(
        &self,
        address: &str,
        expected: Option<(Command, u64)>,
    ) -> Result<Option<Receipt>> {
        let (done, failure, receipt) = {
            let mut state = self.inner.state.lock().unwrap();
            ensure_open(&state)?;
            if let Some((command, execution)) = &expected {
                if let Some(receipt) = state.retry(&command.command_id, &command.fingerprint)? {
                    return Ok(Some(receipt));
                }
                let agent = state.agent(address)?;
                if agent.snapshot.execution_id != Some(*execution)
                    || agent.snapshot.status == AgentStatus::Stopped
                {
                    return Err(Error::ExecutionNotActive(*execution));
                }
            }
            let agent = state.agent(address)?;
            let epoch = agent.active_epoch;
            let waiting_turn = agent
                .snapshot
                .permission
                .as_ref()
                .filter(|_| epoch.is_none())
                .map(|request| request.turn);
            let interruption = agent
                .snapshot
                .execution_id
                .filter(|_| agent.snapshot.status != AgentStatus::Stopped)
                .map(|execution_id| {
                    let turn = agent.snapshot.turn.filter(|_| !agent.accepted.is_empty());
                    let preview = agent.reply_preview.as_ref().filter(|preview| {
                        preview.execution_id == execution_id && Some(preview.turn) == turn
                    });
                    InterruptedExecution {
                        execution_id,
                        turn,
                        text: preview.map_or_else(String::new, |preview| preview.text.clone()),
                        truncated: preview.is_some_and(|preview| preview.truncated),
                        usage: agent.pending_usage(),
                    }
                });
            let command = expected.map(|(command, _)| command);
            let key = command.as_ref().map(|command| command.command_id.clone());
            let stopped = self
                .commit(
                    &mut state,
                    Record::AgentStopped {
                        address: address.into(),
                        command,
                        cleanup_pending: epoch.is_some(),
                        interruption,
                    },
                )
                .err();
            if stopped.is_some() {
                state.agent_mut(address)?.snapshot.status = AgentStatus::Stopped;
            }
            let failed_cleanup =
                waiting_turn.and_then(|turn| self.settle_cancelled_turn(address, turn).err());
            let failure = stopped.or(failed_cleanup);
            let workers = self.inner.workers.lock().unwrap();
            let done = epoch.and_then(|epoch| workers.get(&epoch)).map(|worker| {
                let _ = worker.cancel.send(true);
                worker.done.clone()
            });
            let receipt = key
                .and_then(|key| state.commands.get(&key))
                .map(|(_, receipt)| receipt.clone());
            (done, failure, receipt)
        };
        let cleanup = self.await_cleanup(address, done).await;
        if let Some(error) = failure {
            return Err(error);
        }
        cleanup?;
        Ok(receipt)
    }

    async fn await_cleanup(
        &self,
        address: &str,
        done: Option<tokio::sync::watch::Receiver<super::CleanupOutcome>>,
    ) -> Result<()> {
        if let Some(mut done) = done {
            loop {
                if let Some(result) = done.borrow().clone() {
                    result.map_err(Error::InvalidEventStream)?;
                    break;
                }
                done.changed().await.map_err(|_| {
                    Error::InvalidEventStream(format!(
                        "agent '{address}' cleanup task ended without a result"
                    ))
                })?;
            }
        }
        if let Some(message) = self
            .inner
            .state
            .lock()
            .unwrap()
            .cleanup_failures
            .get(address)
        {
            return Err(Error::InvalidEventStream(message.clone()));
        }
        Ok(())
    }
}

fn rewind_blocked(agent: &AgentState) -> bool {
    agent.active_epoch.is_some()
        || agent.snapshot.pending_inputs > 0
        || matches!(
            agent.snapshot.status,
            AgentStatus::Running | AgentStatus::Waiting
        )
}

fn cancelled_tool_result(address: &str, turn: u64, id: &str) -> Result<EventPayload> {
    let result =
        crate::tool::ToolResultEnvelope::cancelled("pending action cancelled before dispatch");
    Ok(EventPayload::ToolResult {
        turn,
        ts: timestamp()?,
        conversation: Some(address.into()),
        tool_call_id: id.into(),
        status: result.status,
        summary: result.summary,
        model_content: result.model_content,
        truncated: result.truncated,
        files_read: Vec::new(),
        files_changed: Vec::new(),
        commands_run: Vec::new(),
        memory_changed: None,
        structured: result.structured,
    })
}

fn command(command_id: &str, address: &str, intent: &impl serde::Serialize) -> Result<Command> {
    validate_key(command_id)?;
    validate_address(address)?;
    Ok(Command {
        command_id: command_id.into(),
        fingerprint: fingerprint(intent)?,
    })
}

#[cfg(test)]
mod rewind_guard_tests {
    use super::*;

    fn idle_agent() -> AgentState {
        let mut state = State::default();
        state
            .reduce(
                1,
                Record::AgentCreated {
                    command_id: "create".into(),
                    fingerprint: "fingerprint".into(),
                    address: "main".into(),
                    config: crate::AgentConfig::default(),
                },
            )
            .unwrap();
        state.agents.remove("main").unwrap()
    }

    #[test]
    fn active_cleanup_epoch_and_pending_inputs_block_rewind() {
        let mut agent = idle_agent();
        agent.active_epoch = Some(7);
        assert!(rewind_blocked(&agent));

        agent.active_epoch = None;
        agent.snapshot.pending_inputs = 1;
        assert!(rewind_blocked(&agent));
    }
}
