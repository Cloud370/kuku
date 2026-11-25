use kuku_client::{Action, AgentStatus};

use crate::contextual::{ContextControl, PendingCommandIdentity, PendingCommandKind};

use super::{App, ServiceStatus};

impl App {
    pub(crate) fn cache_warning(&self) -> Option<&kuku::session::runtime::CacheWarning> {
        let pane = self.pane();
        let warning = pane.snapshot.as_ref()?.agent.cache_warning.as_ref()?;
        (pane.dismissed_cache_warning != Some(warning.since_turn)).then_some(warning)
    }

    pub(crate) fn contextual_controls(&self) -> Vec<ContextControl> {
        let mut controls = Vec::new();
        if let Some(request_id) = self.pane().pending_request_id() {
            controls.push(ContextControl::Request {
                target: self.selection().target,
                request_id: request_id.to_owned(),
            });
        }
        if let Some(request) = self.credential_failure() {
            controls.push(ContextControl::RepairCredentials { request });
        }
        if let Some(control) = self.run_control() {
            controls.push(control);
        }
        if let Some(control) = self.pending_recovery_control() {
            controls.push(control);
        }
        if let Some(target) = self.previous_agent().cloned() {
            controls.push(ContextControl::PreviousAgent { target });
        }
        if !self.connecting
            && !self.restarting
            && matches!(
                self.service_status(),
                ServiceStatus::Offline
                    | ServiceStatus::UpdatesUnavailable
                    | ServiceStatus::Reconnecting
            )
        {
            controls.push(ContextControl::Reconnect {
                endpoint: self.endpoint.clone(),
            });
        }
        if let Some(warning) = self.cache_warning() {
            controls.push(ContextControl::CacheWarning {
                since_turn: warning.since_turn,
            });
        }
        controls.push(ContextControl::Messages);
        controls
    }

    fn run_control(&self) -> Option<ContextControl> {
        let pane = self.pane();
        let target = self.selection().target;
        if let Some(execution_id) = pane.running_execution() {
            return Some(ContextControl::Interrupt {
                target,
                execution_id,
            });
        }
        if pane.busy_or_waiting() {
            return None;
        }
        let snapshot = pane.snapshot.as_ref()?;
        let can_continue = match snapshot.agent.status {
            AgentStatus::Interrupted => true,
            AgentStatus::Idle | AgentStatus::Stopped => {
                snapshot.agent.pending_inputs > 0 && !snapshot.pending.is_empty()
            }
            AgentStatus::Running | AgentStatus::Waiting => false,
        };
        if snapshot.cleaning_up || !can_continue {
            return None;
        }
        Some(ContextControl::Continue {
            target,
            execution_id: snapshot.execution_id(),
            inputs: snapshot
                .pending
                .iter()
                .map(|input| input.receipt.command_id.clone())
                .collect(),
        })
    }

    fn pending_recovery_control(&self) -> Option<ContextControl> {
        let mut commands = self
            .pending
            .values()
            .filter(|item| item.stage.needs_recovery())
            .map(|item| PendingCommandIdentity {
                target: item.command.target.clone(),
                command_id: item.command.command_id.clone(),
                kind: if matches!(item.command.action, Action::Send { .. }) {
                    PendingCommandKind::Send
                } else {
                    PendingCommandKind::Action
                },
            })
            .collect::<Vec<_>>();
        commands.sort_by(|left, right| {
            (
                &left.target.workspace,
                &left.target.session_id,
                &left.target.agent,
                &left.command_id,
            )
                .cmp(&(
                    &right.target.workspace,
                    &right.target.session_id,
                    &right.target.agent,
                    &right.command_id,
                ))
        });
        (!commands.is_empty()).then_some(ContextControl::PendingRecovery { commands })
    }
}
