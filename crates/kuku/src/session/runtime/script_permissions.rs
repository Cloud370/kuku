use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::event::{EventPayload, EventStore};
use crate::execution::{WorkspaceActionReceipt, WorkspaceActionState};
use crate::query::PermissionRequest;
use crate::tool::ToolResultEnvelope;
use crate::tool::run_js::append_recovery_evidence;
use crate::{Error, Result};
use crate::{WorkspaceScriptActionFacts, WorkspaceScriptActionStatus};

use super::record::{Record, State};
use super::{Receipt, Session, ensure_open};

const MAX_PENDING_SCRIPT_APPROVALS: usize = 64;
pub(super) const RESTARTED_SCRIPT: &str =
    "The service restarted while awaiting approval. Restart the remaining script steps.";

impl Session {
    pub(super) fn recover_script_permissions(&self) -> Result<()> {
        let scripts: BTreeSet<_> = self
            .inner
            .state
            .lock()
            .unwrap()
            .script_permissions
            .values()
            .map(|request| {
                (
                    request.turn,
                    request.conversation.as_str().to_owned(),
                    request.tool_call_id.clone(),
                )
            })
            .collect();
        for (turn, agent, id) in scripts {
            self.recover_paused_script(turn, &agent, &id)?;
        }
        self.expire_script_permissions(RESTARTED_SCRIPT)
    }

    fn recover_paused_script(&self, turn: u64, agent: &str, id: &str) -> Result<()> {
        let mut settled = false;
        let mut actions = BTreeMap::new();
        EventStore::visit_after_path(self.events_path(), turn.saturating_sub(1), |event| {
            if let EventPayload::ToolResult {
                conversation,
                turn: settled_turn,
                tool_call_id,
                ..
            } = &event.payload
            {
                settled |= *settled_turn == turn
                    && tool_call_id == id
                    && conversation.as_deref() == Some(agent);
            }
            if let Some(action) = WorkspaceActionReceipt::from_event(&event)? {
                if action.agent == agent && action.turn == turn && action.tool_call_id == id {
                    let ordinal = action
                        .action_id
                        .rsplit('/')
                        .next()
                        .and_then(|value| value.parse::<usize>().ok());
                    actions.insert((ordinal, action.action_id.clone()), action);
                }
            }
            Ok(())
        })?;
        if settled {
            return Ok(());
        }
        let mut outcome =
            ToolResultEnvelope::cancelled("run_js interrupted while awaiting approval");
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
        EventStore::open(self.events_path())?.append(result)?;
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
        args: Value,
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
        self.commit(
            &mut state,
            Record::ScriptPermissionWaiting {
                address,
                request,
                args,
            },
        )?;
        let (send, receive) = oneshot::channel();
        self.inner
            .script_permission_replies
            .lock()
            .unwrap()
            .insert(request_id, send);
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
        if replies
            .get(request_id)
            .is_none_or(oneshot::Sender::is_closed)
        {
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
            let _ = reply.send(allow);
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
        result["path"] = json!(target);
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
