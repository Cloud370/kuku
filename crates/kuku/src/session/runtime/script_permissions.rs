use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::json;
use tokio::sync::oneshot;

use crate::event::{EventPayload, EventStore};
use crate::execution::{WorkspaceActionReceipt, WorkspaceActionState};
use crate::query::PermissionRequest;
use crate::tool::ToolResultEnvelope;
use crate::tool::run_js::append_recovery_evidence;
use crate::{Error, Result};
use crate::{WorkspaceScriptActionFacts, WorkspaceScriptActionStatus};

use super::record::{Record, State};
use super::{AgentStatus, Receipt, Session, ensure_open};

const MAX_PENDING_SCRIPT_APPROVALS: usize = 64;
pub(super) const RESTARTED_SCRIPT: &str =
    "The service restarted before the script settled. Restart the remaining script steps.";

#[derive(Clone)]
pub(crate) struct ScriptOwner {
    pub(crate) execution_id: u64,
    pub(crate) detached: Arc<AtomicBool>,
    pub(crate) cancelling: Arc<AtomicBool>,
}

pub(super) struct ScriptWaiter {
    owner: ScriptOwner,
    reply: oneshot::Sender<bool>,
}

impl ScriptOwner {
    fn can_dispatch(&self, state: &State, address: &str) -> Result<bool> {
        let agent = &state.agent(address)?.snapshot;
        Ok(!state.closing
            && !state.closed
            && !self.cancelling.load(Ordering::Acquire)
            && (self.detached.load(Ordering::Acquire)
                || agent.status == AgentStatus::Running
                    && agent.execution_id == Some(self.execution_id)))
    }
}

impl Session {
    pub(crate) fn mark_script_action_dispatched(
        &self,
        agent: &str,
        owner: &ScriptOwner,
        payload: EventPayload,
    ) -> Result<bool> {
        let state = self.inner.state.lock().unwrap();
        if !owner.can_dispatch(&state, agent)? {
            return Ok(false);
        }
        EventStore::open(self.events_path())?.append(payload)?;
        Ok(true)
    }
    pub(super) fn recover_script_permissions(&self) -> Result<()> {
        let scripts = self.inner.state.lock().unwrap().recoverable_scripts.clone();
        for (turn, agent, id) in scripts {
            self.recover_paused_script(turn, &agent, &id)?;
            self.commit(
                &mut self.inner.state.lock().unwrap(),
                Record::ScriptRecoveryCompleted {
                    address: agent,
                    turn,
                    tool_call_id: id,
                },
            )?;
        }
        self.expire_script_permissions(RESTARTED_SCRIPT)
    }

    fn recover_paused_script(&self, turn: u64, agent: &str, id: &str) -> Result<()> {
        let mut settled = BTreeSet::new();
        let mut dispatched = BTreeSet::new();
        let mut batches = BTreeMap::new();
        let mut actions = BTreeMap::new();
        EventStore::visit_after_path(self.events_path(), turn.saturating_sub(1), |event| {
            match &event.payload {
                EventPayload::ToolCall {
                    conversation,
                    turn: event_turn,
                    tool_call_id,
                    request_id,
                    tool,
                    ..
                } if *event_turn == turn && conversation.as_deref() == Some(agent) => {
                    batches.insert(tool_call_id.clone(), (request_id.clone(), tool.clone()));
                }
                EventPayload::ToolResult {
                    conversation,
                    turn: event_turn,
                    tool_call_id,
                    ..
                } if *event_turn == turn && conversation.as_deref() == Some(agent) => {
                    settled.insert(tool_call_id.clone());
                }
                EventPayload::Unknown(data)
                    if data["kind"] == "runtime.tool_dispatched"
                        && data["turn"] == turn
                        && data["agent"] == agent =>
                {
                    if let Some(id) = data["tool_call_id"].as_str() {
                        dispatched.insert(id.to_owned());
                    }
                }
                _ => {}
            }
            if let Some(action) = WorkspaceActionReceipt::from_event(&event)? {
                if action.agent == agent && action.turn == turn && action.tool_call_id == id {
                    let ordinal = action
                        .action_id
                        .rsplit('/')
                        .next()
                        .and_then(|value| value.strip_prefix("action_"))
                        .and_then(|value| value.parse::<usize>().ok());
                    actions.insert((ordinal, action.action_id.clone()), action);
                }
            }
            Ok(())
        })?;
        let mut writer = EventStore::open(self.events_path())?;
        if let Some((batch, _)) = batches.get(id) {
            let state = self.inner.state.lock().unwrap();
            let waiting = state
                .agent(agent)?
                .snapshot
                .permission
                .as_ref()
                .map(|request| request.tool_call_id.as_str());
            for (sibling, (request, tool)) in &batches {
                if request == batch
                    && sibling != id
                    && !settled.contains(sibling)
                    && !(tool == "run_js" && dispatched.contains(sibling))
                    && waiting != Some(sibling)
                {
                    let reason = if dispatched.contains(sibling) {
                        "script batch interrupted after dispatch; prior effects are unknown. Inspect before retrying"
                    } else {
                        "script batch interrupted before this action was dispatched"
                    };
                    let mut outcome = ToolResultEnvelope::cancelled(reason);
                    outcome.model_content = reason.into();
                    writer.append(super::controls::cancelled_tool_result(
                        agent, turn, sibling, outcome,
                    )?)?;
                }
            }
        }
        if settled.contains(id) {
            return Ok(());
        }
        let mut outcome = ToolResultEnvelope::cancelled("run_js interrupted before settlement");
        let actions: Vec<_> = actions
            .into_values()
            .map(recovered_action)
            .collect::<Result<_>>()?;
        outcome.model_content = RESTARTED_SCRIPT.into();
        if actions
            .iter()
            .any(|action| action.status == WorkspaceScriptActionStatus::Unknown)
        {
            outcome.model_content.push_str(
                "\nInspect the workspace before retrying host actions with an unknown outcome.",
            );
        }
        append_recovery_evidence(&mut outcome.model_content, &actions, false);
        let result = super::controls::cancelled_tool_result(agent, turn, id, outcome)?;
        writer.append(result)?;
        Ok(())
    }

    /// Return unresolved tool approvals for all Agents in this Session.
    pub fn pending_permissions(&self) -> Vec<PermissionRequest> {
        let state = self.inner.state.lock().unwrap();
        state
            .agents
            .values()
            .filter_map(|agent| {
                agent
                    .snapshot
                    .permission
                    .as_ref()
                    .filter(|_| agent.permission_reply.is_none())
            })
            .chain(state.script_permissions.values())
            .cloned()
            .collect()
    }

    pub(crate) fn request_script_permission(
        &self,
        request: PermissionRequest,
        owner: ScriptOwner,
    ) -> Result<oneshot::Receiver<bool>> {
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        if state.script_permissions.len() >= MAX_PENDING_SCRIPT_APPROVALS {
            return Err(Error::InvalidArgument(
                "too many script operations are awaiting approval".into(),
            ));
        }
        let request_id = request.id.clone();
        if state.script_permissions.contains_key(&request_id) {
            return Err(Error::InvalidArgument(
                "script approval request already exists".into(),
            ));
        }
        let address = request.conversation.as_str().to_owned();
        if !owner.can_dispatch(&state, &address)? {
            return Err(Error::ExecutionNotActive(owner.execution_id));
        }
        self.commit(
            &mut state,
            Record::ScriptPermissionWaiting { address, request },
        )?;
        let (send, receive) = oneshot::channel();
        self.inner
            .script_permission_replies
            .lock()
            .unwrap()
            .insert(request_id, ScriptWaiter { owner, reply: send });
        Ok(receive)
    }

    pub(super) fn reply_script_permission(
        &self,
        state: &mut State,
        command_id: &str,
        address: &str,
        request_id: &str,
        fingerprint: String,
        allow: bool,
    ) -> Result<Receipt> {
        let mut replies = self.inner.script_permission_replies.lock().unwrap();
        let Some(waiter) = replies.get(request_id) else {
            return Err(Error::PermissionRequestNotPending(request_id.into()));
        };
        if waiter.reply.is_closed() || !waiter.owner.can_dispatch(state, address)? {
            return Err(Error::PermissionRequestNotPending(request_id.into()));
        }
        self.commit(
            state,
            Record::ScriptPermissionReplied {
                address: address.into(),
                command_id: command_id.into(),
                fingerprint,
                request_id: request_id.into(),
                allow,
            },
        )?;
        if let Some(reply) = replies.remove(request_id) {
            let _ = reply.reply.send(allow);
        }
        Ok(state.commands[command_id].1.clone())
    }

    pub(crate) fn end_script_permission(&self, request_id: &str, reason: &str) -> Result<()> {
        let mut state = self.inner.state.lock().unwrap();
        if let Some(request) = state.script_permissions.get(request_id) {
            let address = request.conversation.as_str().to_owned();
            let turn = request.turn;
            self.commit(
                &mut state,
                Record::ScriptPermissionEnded {
                    address,
                    turn,
                    request_id: request_id.into(),
                    reason: reason.into(),
                },
            )?;
        }
        self.inner
            .script_permission_replies
            .lock()
            .unwrap()
            .remove(request_id);
        Ok(())
    }

    pub(super) fn expire_script_permissions(&self, reason: &str) -> Result<()> {
        let requests: Vec<_> = self
            .inner
            .state
            .lock()
            .unwrap()
            .script_permissions
            .keys()
            .cloned()
            .collect();
        for id in requests {
            self.end_script_permission(&id, reason)?;
        }
        Ok(())
    }
}

fn recovered_action(receipt: WorkspaceActionReceipt) -> Result<WorkspaceScriptActionFacts> {
    let status = match receipt.status {
        WorkspaceActionState::Dispatched => WorkspaceScriptActionStatus::Unknown,
        WorkspaceActionState::Ok => WorkspaceScriptActionStatus::Ok,
        WorkspaceActionState::Error => WorkspaceScriptActionStatus::Error,
        WorkspaceActionState::Blocked => WorkspaceScriptActionStatus::Blocked,
        WorkspaceActionState::Cancelled => WorkspaceScriptActionStatus::Cancelled,
    };
    let mut result = serde_json::to_value(receipt.facts)?;
    if let Some(target) = receipt.target {
        if receipt.tool == crate::WorkspaceScriptOperation::SaveValue {
            result["name"] = json!(target);
            result["reference"] = result["output_ref"].clone();
        } else {
            result["path"] = json!(target);
        }
    }
    Ok(WorkspaceScriptActionFacts {
        id: receipt.action_id,
        tool: receipt.tool,
        status,
        summary: String::new(),
        truncated: false,
        result,
    })
}
