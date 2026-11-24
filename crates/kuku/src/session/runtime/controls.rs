use std::collections::BTreeSet;

use crate::context::revert::RewindVisibility;
use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::{Error, Result};

use super::jobs::await_job_cleanup;
use super::receipts::{validate_address, validate_key};
use super::record::{
    AgentState, Command, InterruptedExecution, Record, State, fingerprint, timestamp,
};
use super::{AgentStatus, InputStatus, Receipt, Session, Withdrawal, ensure_open};

impl Session {
    /// Hide a committed user message and the following visible history for one Agent.
    ///
    /// Optional file restoration requires verified built-in write_file and edit_file records.
    /// Use a settled Turn preview for explicit review of the file plan before applying it.
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
        let store = EventStore::open(self.events_path())?;
        let (after, controls) = store.rewind_facts(address, target_event_id)?;
        let visibility = RewindVisibility::new(&controls);
        let target = store
            .read_after(target_event_id.saturating_sub(1), 1)?
            .into_iter()
            .find(|event| event.id == target_event_id)
            .ok_or_else(|| Error::InvalidArgument("rewind target is no longer available".into()))?;
        let EventPayload::MessageUser {
            conversation,
            turn,
            text,
            job_delivery: false,
            ..
        } = &target.payload
        else {
            return Err(Error::InvalidArgument(
                "rewind target is not a committed user message".into(),
            ));
        };
        if conversation != address
            || text != expected_text
            || !visibility.includes(&target, Some(address))
        {
            return Err(Error::InvalidArgument(
                "rewind target belongs to another Agent or is no longer visible".into(),
            ));
        }

        let agent = state.agent(address)?;
        let mut inputs = RewindInputs::default();
        if !agent.accepted.is_empty() {
            EventStore::visit_after_path(self.events_path(), after, |event| {
                inputs.observe(&event, address, target_event_id, &visibility);
                Ok(())
            })?;
        }
        let removed_input_ids = agent
            .accepted
            .iter()
            .map(|entry| &entry.input.command_id)
            .filter(|id| inputs.removed.contains(*id))
            .cloned()
            .collect();
        if rewind_blocked_except(agent, &removed_input_ids) {
            return Err(Error::InvalidArgument(
                "cannot rewind while this Agent is executing or has pending input".into(),
            ));
        }
        if restore_files
            && (state.jobs.has_active()
                || state.agents.iter().any(|(candidate, agent)| {
                    if candidate == address {
                        rewind_blocked_except(agent, &removed_input_ids)
                    } else {
                        rewind_blocked(agent)
                    }
                }))
        {
            return Err(Error::InvalidArgument(
                "Wait for Session Agents and jobs to settle before restoring files".into(),
            ));
        }
        let mut warnings = Vec::new();
        let mut file_revert = None;
        if restore_files {
            let workspace = &self.inner.options.workspace;
            let files = crate::context::revert::plan_file_rewind(
                self.events_path(),
                workspace,
                address,
                target_event_id,
            )?;
            let session_dir = self.inner.events_path.parent().ok_or_else(|| {
                Error::InvalidArgument("events path has no session directory".into())
            })?;
            file_revert = Some(crate::context::revert::apply_verified_file_rewind(
                &files.plan,
                workspace,
                session_dir,
                store.next_id(),
                &files.expected,
            )?);
            warnings.extend(
                files
                    .preview
                    .skipped
                    .into_iter()
                    .map(|path| format!("Skipped {path}: file version could not be verified")),
            );
        }
        commit_with_file_revert(file_revert, || {
            self.commit(
                &mut state,
                Record::MessageRewind {
                    command,
                    address: address.into(),
                    target_event_id,
                    target_turn: *turn,
                    restore_files,
                    removed_input_ids: removed_input_ids.into_iter().collect(),
                    warnings: warnings.clone(),
                },
            )
        })?;
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
                    .map(|index| &agent.inbox[index])
            })
            .ok_or_else(|| Error::InputNotQueued(input_command_id.into()))?;
        if &old.input.source != source {
            return Err(Error::InvalidArgument(format!(
                "input '{input_command_id}' belongs to a different source"
            )));
        }
        if super::boundary::input_model_text(text, &old.values).len()
            > self.inner.options.limits.max_input_bytes
        {
            return Err(Error::InvalidArgument(
                "replacement text and references exceed the configured byte limit".into(),
            ));
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
            let result = crate::tool::ToolResultEnvelope::cancelled(
                "pending action cancelled before dispatch",
            );
            writer.append(cancelled_tool_result(address, turn, id, result)?)?;
        }
        if let Some(id) = open.intersection(&dispatched).next() {
            return Err(Error::InterruptedOpenTool(format!(
                "agent '{address}' tool call '{id}' from turn {turn} has no confirmed result after cancellation"
            )));
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
        let (done, jobs, failure, receipt) = {
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
            let jobs = if key.is_none() {
                self.cancel_owned_jobs(address)
            } else {
                Vec::new()
            };
            let workers = self.inner.workers.lock().unwrap();
            let done = epoch.and_then(|epoch| workers.get(&epoch)).map(|worker| {
                let _ = worker.cancel.send(true);
                worker.done.clone()
            });
            let receipt = key
                .and_then(|key| state.commands.get(&key))
                .map(|(_, receipt)| receipt.clone());
            (done, jobs, failure, receipt)
        };
        let mut cleanup = self.await_cleanup(address, done).await;
        for job in jobs {
            cleanup = cleanup.and(await_job_cleanup(job).await);
        }
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

pub(super) fn rewind_blocked(agent: &AgentState) -> bool {
    rewind_blocked_except(agent, &BTreeSet::new())
}

#[derive(Default)]
struct RewindInputs {
    last_accepted: BTreeSet<String>,
    removed: BTreeSet<String>,
}

impl RewindInputs {
    fn observe(
        &mut self,
        event: &StoredEvent,
        address: &str,
        target: u64,
        visibility: &RewindVisibility,
    ) {
        match &event.payload {
            EventPayload::MessageUser { conversation, .. } if conversation == address => {
                if event.id >= target && visibility.includes(event, Some(address)) {
                    self.removed.append(&mut self.last_accepted);
                } else {
                    self.last_accepted.clear();
                }
            }
            EventPayload::Unknown(value) => {
                let record = &value["record"];
                if value["kind"] == "runtime.operation"
                    && record["operation"] == "inputs_accepted"
                    && record["address"] == address
                {
                    self.last_accepted = record["input_ids"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|id| id.as_str().map(str::to_owned))
                        .collect();
                }
            }
            _ => {}
        }
    }
}

fn rewind_blocked_except(agent: &AgentState, allowed_input_ids: &BTreeSet<String>) -> bool {
    agent.active_epoch.is_some()
        || matches!(
            agent.snapshot.status,
            AgentStatus::Running | AgentStatus::Waiting
        )
        || (agent.snapshot.pending_inputs > 0 && agent.inbox.is_empty())
        || agent
            .inbox
            .iter()
            .any(|delivery| !allowed_input_ids.contains(&delivery.input.command_id))
        || agent
            .accepted
            .iter()
            .any(|delivery| !allowed_input_ids.contains(&delivery.input.command_id))
}

pub(super) fn commit_with_file_revert<T>(
    transaction: Option<crate::context::revert::FileRevertTransaction>,
    commit: impl FnOnce() -> Result<T>,
) -> Result<T> {
    match commit() {
        Ok(value) => Ok(value),
        Err(error) => {
            if let Some(transaction) = transaction {
                transaction.rollback().map_err(|rollback_error| {
                    Error::InvalidEventStream(format!(
                        "rewind commit failed ({error}) and workspace rollback failed ({rollback_error})"
                    ))
                })?;
            }
            Err(error)
        }
    }
}

pub(super) fn cancelled_tool_result(
    address: &str,
    turn: u64,
    id: &str,
    result: crate::tool::ToolResultEnvelope,
) -> Result<EventPayload> {
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

pub(super) fn command(
    command_id: &str,
    address: &str,
    intent: &impl serde::Serialize,
) -> Result<Command> {
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
    use crate::session::runtime::record::QueuedDelivery;

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

    #[test]
    fn failed_rewind_commit_restores_files_and_retry_can_apply() {
        let workspace = tempfile::tempdir().unwrap();
        let session_dir = tempfile::tempdir().unwrap();
        let path = workspace.path().join("file.txt");
        std::fs::write(&path, "after").unwrap();
        let plan = crate::context::revert::RevertPlan {
            restores: vec![crate::context::revert::FileRestore {
                path: path.clone(),
                old_content: "before".into(),
                old_hash: String::new(),
                new_content_on_plan: "after".into(),
            }],
            deletes: Vec::new(),
            unrecoverable: Vec::new(),
            sensitive_files: Vec::new(),
        };
        let (transaction, _) = crate::context::revert::apply_message_rewind_files(
            &plan,
            workspace.path(),
            session_dir.path(),
            3,
        )
        .unwrap();

        let error = commit_with_file_revert(Some(transaction), || {
            Err::<(), _>(Error::InvalidArgument("injected commit failure".into()))
        })
        .unwrap_err();

        assert!(error.to_string().contains("injected commit failure"));
        assert_eq!("after", std::fs::read_to_string(&path).unwrap());
        let (transaction, _) = crate::context::revert::apply_message_rewind_files(
            &plan,
            workspace.path(),
            session_dir.path(),
            4,
        )
        .unwrap();
        commit_with_file_revert(Some(transaction), || Ok(())).unwrap();
        assert_eq!("before", std::fs::read_to_string(path).unwrap());
    }

    #[test]
    fn interrupted_resume_rewind_removes_only_affected_input() {
        let mut state = State::default();
        state
            .reduce(
                1,
                Record::AgentCreated {
                    command_id: "create".into(),
                    fingerprint: "fp".into(),
                    address: "main".into(),
                    config: crate::AgentConfig::default(),
                },
            )
            .unwrap();
        let make_delivery = |id: &str, text: &str, accepted_turn: Option<u64>| QueuedDelivery {
            queued_at: 1,
            receipt: Receipt {
                command_id: id.into(),
                event_id: 1,
                agent: "main".into(),
            },
            input: crate::session::runtime::Input::new(id, "main", text),
            values: Vec::new(),
            accepted_turn,
        };
        state.agents.get_mut("main").unwrap().accepted = vec![
            make_delivery("rewound", "rewound prompt", Some(1)),
            make_delivery("unrelated", "keep this input", Some(1)),
        ];
        state
            .reduce(
                2,
                Record::AgentResumed {
                    address: "main".into(),
                    command: None,
                    restore: true,
                },
            )
            .unwrap();
        let rewind = Record::MessageRewind {
            command: Command {
                command_id: "rewind".into(),
                fingerprint: "fp".into(),
            },
            address: "main".into(),
            target_event_id: 1,
            target_turn: 1,
            restore_files: false,
            removed_input_ids: vec!["rewound".into()],
            warnings: Vec::new(),
        };
        let rewind = Record::from_event(&StoredEvent {
            id: 3,
            payload: rewind.payload().unwrap(),
        })
        .unwrap()
        .unwrap();
        state.reduce(3, rewind).unwrap();

        let agent = state.agent("main").unwrap();
        assert_eq!(
            vec!["unrelated"],
            agent
                .inbox
                .iter()
                .map(|d| d.input.command_id.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(InputStatus::Withdrawn, state.deliveries["rewound"]);
        state
            .agents
            .get_mut("main")
            .unwrap()
            .inbox
            .push_back(make_delivery("new", "new prompt", None));
        state
            .reduce(
                4,
                Record::InputsAccepted {
                    address: "main".into(),
                    input_ids: vec!["new".into()],
                    message_input_ids: Vec::new(),
                    turn: Some(2),
                },
            )
            .unwrap();
        assert!(
            state
                .agent("main")
                .unwrap()
                .accepted
                .iter()
                .any(|d| d.input.command_id == "new")
        );
    }

    #[test]
    fn rewind_does_not_remove_an_unaccepted_duplicate_prompt() {
        let accepted = Record::InputsAccepted {
            address: "main".into(),
            input_ids: vec!["rewound".into()],
            message_input_ids: vec!["rewound".into()],
            turn: Some(1),
        };
        let events = vec![
            StoredEvent {
                id: 1,
                payload: accepted.payload().unwrap(),
            },
            StoredEvent {
                id: 2,
                payload: EventPayload::MessageUser {
                    ts: "now".into(),
                    conversation: "main".into(),
                    turn: 1,
                    text: "same prompt".into(),
                    from: None,
                    job_delivery: false,
                    images: Vec::new(),
                },
            },
        ];
        let mut inputs = RewindInputs::default();
        let visibility = RewindVisibility::new(&events);
        for event in &events {
            inputs.observe(event, "main", 2, &visibility);
        }
        let removed = inputs.removed;
        assert_eq!(BTreeSet::from(["rewound".into()]), removed);

        let make_delivery = |id: &str, accepted_turn: Option<u64>| QueuedDelivery {
            queued_at: 3,
            receipt: Receipt {
                command_id: id.into(),
                event_id: 3,
                agent: "main".into(),
            },
            input: crate::session::runtime::Input::new(id, "main", "same prompt"),
            values: Vec::new(),
            accepted_turn,
        };
        let mut agent = idle_agent();
        agent.accepted.push(make_delivery("rewound", Some(1)));
        agent.inbox.push_back(make_delivery("unrelated", None));
        assert!(rewind_blocked_except(&agent, &removed));
        assert_eq!("unrelated", agent.inbox.front().unwrap().input.command_id);
    }
}
