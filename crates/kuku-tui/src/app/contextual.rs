use kuku_client::{Action, AgentStatus};

use crate::contextual::{ContextControl, PendingCommandIdentity, PendingCommandKind};

use super::{Activity, App, PendingStage, ServiceStatus};

impl App {
    pub(crate) fn contextual_controls(&self) -> Vec<ContextControl> {
        let mut controls = Vec::new();
        if let Some(snapshot) = &self.pane().snapshot {
            let request = snapshot
                .agent
                .question
                .as_ref()
                .map(|request| request.id.clone())
                .or_else(|| {
                    snapshot
                        .agent
                        .permission
                        .as_ref()
                        .map(|request| request.id.clone())
                });
            if let Some(request_id) = request {
                controls.push(ContextControl::Request {
                    target: self.selection().target,
                    request_id,
                });
            }
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
        if !self.connecting && !self.restarting {
            if matches!(
                self.service_status(),
                ServiceStatus::Offline | ServiceStatus::UpdatesUnavailable
            ) {
                controls.push(ContextControl::Reconnect {
                    endpoint: self.endpoint.clone(),
                });
            }
            if !self.remote {
                controls.push(ContextControl::Restart {
                    endpoint: self.endpoint.clone(),
                });
            }
        }
        controls
    }

    fn run_control(&self) -> Option<ContextControl> {
        let snapshot = self.pane().snapshot.as_ref()?;
        let target = self.selection().target;
        if snapshot.agent.status == AgentStatus::Running
            && self.pane().activity() != Activity::Stopping
        {
            return self
                .pane()
                .execution()
                .map(|execution_id| ContextControl::Interrupt {
                    target,
                    execution_id,
                });
        }
        let can_continue = match snapshot.agent.status {
            AgentStatus::Interrupted => true,
            AgentStatus::Stopped => {
                snapshot.agent.pending_inputs > 0 && !snapshot.pending.is_empty()
            }
            _ => false,
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
            .filter(|item| !matches!(item.stage, PendingStage::Discarding))
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
