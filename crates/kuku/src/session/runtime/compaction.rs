use std::sync::Arc;

use tokio::sync::watch;

use crate::{Error, Result};

use super::controls::command;
use super::driver::Work;
use super::record::{Command, ManualCompaction, Record, State};
use super::usage::{AgentUsage, UnfinishedSummary};
use super::{Activation, AgentStatus, Receipt, Session, ensure_open};

impl Session {
    /// Compact an idle or interrupted Agent's model context without submitting a user message.
    pub fn compact_command(&self, command_id: &str, address: &str) -> Result<Receipt> {
        let command = command(command_id, address, &("compact", address))?;
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        if let Some(receipt) = state.retry(command_id, &command.fingerprint)? {
            return Ok(receipt);
        }
        self.schedule_compaction(&mut state, address, command)?;
        Ok(state.commands[command_id].1.clone())
    }

    /// Restore the context before the latest active compaction while keeping all visible messages.
    pub fn restore_context_command(&self, command_id: &str, address: &str) -> Result<Receipt> {
        let command = command(command_id, address, &("restore_context", address))?;
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        if let Some(receipt) = state.retry(command_id, &command.fingerprint)? {
            return Ok(receipt);
        }
        let agent = state.agent(address)?;
        if agent.active_epoch.is_some() || agent.snapshot.status == AgentStatus::Waiting {
            return Err(Error::InvalidArgument(
                "restore context after this Agent's active work settles".into(),
            ));
        }
        let segment =
            crate::context::runtime::latest_checkpoint_segment(self.events_path(), address)?
                .ok_or_else(|| {
                    Error::InvalidArgument("this Agent has no active compaction to undo".into())
                })?;
        self.commit(
            &mut state,
            Record::CompactionReverted {
                command,
                address: address.into(),
                segment,
            },
        )?;
        Ok(state.commands[command_id].1.clone())
    }

    pub(super) fn schedule_compaction(
        &self,
        state: &mut State,
        address: &str,
        command: Command,
    ) -> Result<()> {
        let agent = state.agent(address)?;
        let status = agent.snapshot.status;
        if !matches!(status, AgentStatus::Idle | AgentStatus::Interrupted)
            || agent.active_epoch.is_some()
            || !agent.inbox.is_empty()
        {
            return Err(Error::InvalidArgument(
                "compact context after this Agent's pending work settles".into(),
            ));
        }
        let turn = agent.snapshot.turn.ok_or_else(|| {
            Error::InvalidArgument("this Agent has no completed conversation to compact".into())
        })?;
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
            Error::InvalidArgument("compacting a session requires a Tokio runtime".into())
        })?;
        let mut query =
            self.query_for_config(address, agent.snapshot.config.clone(), String::new())?;
        let epoch = self.commit(
            state,
            Record::CompactionRequested {
                command,
                address: address.into(),
                turn,
            },
        )?;
        query.runtime_execution = Some(epoch);
        query.turn_override = Some(turn);
        self.spawn_worker(
            state,
            address,
            epoch,
            runtime,
            Work::Compaction(Box::new(query), status),
        )
    }

    pub(super) async fn drive_compaction(
        &self,
        address: &str,
        epoch: u64,
        query: crate::Query,
        status: AgentStatus,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<()> {
        let notify = Arc::new(tokio::sync::Notify::new());
        let compact = query.compact_context(status == AgentStatus::Idle, notify.clone());
        tokio::pin!(compact);
        let outcome = if *cancel.borrow() {
            Ok(())
        } else {
            tokio::select! {
                biased;
                _ = cancel.changed() => { notify.notify_one(); compact.await }
                result = &mut compact => result,
            }
        };
        let mut state = self.inner.state.lock().unwrap();
        let agent = state.agent(address)?;
        if agent.active_epoch == Some(epoch) {
            let cancelled = *cancel.borrow();
            let error = outcome
                .err()
                .filter(|_| !cancelled)
                .map(|error| error.to_string());
            let summary_end = if cancelled {
                UnfinishedSummary::Cancelled
            } else {
                UnfinishedSummary::Interrupted("Execution ended before compaction settled")
            };
            self.settle_open_summary(&mut state, address, summary_end)?;
            self.commit(
                &mut state,
                Record::CompactionSettled {
                    address: address.into(),
                    execution: epoch,
                    status,
                    cancelled,
                    error,
                },
            )?;
            state.agent_mut(address)?.active_epoch = None;
            if cancelled {
                self.resume_after_stop(&mut state, address)?;
            } else if !state.closing
                && state.agents[address]
                    .inbox
                    .iter()
                    .any(|entry| entry.input.activation == Activation::Wake)
            {
                self.schedule_locked(&mut state, address)?;
            }
        }
        Ok(())
    }
}

impl State {
    pub(super) fn compaction_requested(
        &mut self,
        event_id: u64,
        command: Command,
        address: String,
    ) -> Result<()> {
        self.remember_command(command, event_id, address.clone());
        let agent = self.agent_mut(&address)?;
        agent.manual_compaction = Some(ManualCompaction {
            execution: event_id,
            previous_status: agent.snapshot.status,
            finished: false,
        });
        agent.set_active_config(Some(agent.snapshot.config.clone()));
        agent.snapshot.execution_id = Some(event_id);
        agent.snapshot.status = AgentStatus::Running;
        agent.snapshot.last_error = None;
        agent.model_usage = AgentUsage::default();
        Ok(())
    }

    pub(super) fn compaction_settled(
        &mut self,
        address: &str,
        status: AgentStatus,
        error: Option<String>,
    ) -> Result<()> {
        let agent = self.agent_mut(address)?;
        agent.manual_compaction = None;
        if agent.snapshot.status == AgentStatus::Running {
            agent.snapshot.status = status;
        }
        agent.snapshot.execution_id = None;
        agent.snapshot.last_error = error;
        agent.set_active_config(None);
        agent.model_usage.settle(agent.snapshot.turn);
        Ok(())
    }
}
