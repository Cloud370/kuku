use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::context::ToolSchema;
use crate::session::InputSource;

use super::{GateDecisionKind, GateSource, PermissionPolicy, decide_tool_call};

/// Final action and trusted provenance presented to an embedding host.
#[derive(Debug, Clone)]
pub struct ToolAuthorization {
    /// Owning session identity.
    pub session_id: String,
    /// Acting agent address.
    pub agent: String,
    /// Trusted origin of the accepted work.
    pub source: InputSource,
    /// Stable identity of this exact tool invocation.
    pub tool_call_id: String,
    /// Actual tool definition bound to the prepared step.
    pub tool: ToolSchema,
    /// Final arguments after any approved preprocessing.
    pub args: serde_json::Value,
    /// Actual directory used to perform the action.
    pub workspace: PathBuf,
}

/// Host decision about one exact tool action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationDecision {
    /// Permit this action once.
    Allow,
    /// Reject this action.
    Deny,
    /// Persist a pending request and release execution capacity.
    Defer,
}

/// Host authority consulted before a tool's side effects are dispatched.
pub trait ToolAuthorizer: std::fmt::Debug + Send + Sync {
    /// Decide on the supplied exact action or return an error that prevents dispatch.
    fn authorize(&self, action: &ToolAuthorization) -> Result<AuthorizationDecision>;
}

/// Default authority for a trusted local, single-user host.
#[derive(Debug)]
pub struct TrustedLocal;

impl ToolAuthorizer for TrustedLocal {
    fn authorize(&self, _action: &ToolAuthorization) -> Result<AuthorizationDecision> {
        Ok(AuthorizationDecision::Allow)
    }
}

pub(crate) enum ActionAuthorization {
    Allow,
    Deny {
        source: &'static str,
        reason: &'static str,
    },
    Defer,
}

pub(crate) fn authorize_action(
    policy: &PermissionPolicy,
    risk: &str,
    candidate: &str,
    authority: &dyn ToolAuthorizer,
    action: &ToolAuthorization,
) -> Result<ActionAuthorization> {
    let gate = decide_tool_call(&action.tool.name, risk, candidate, policy, &[]);
    if gate.kind == GateDecisionKind::Deny {
        let source = match gate.source {
            GateSource::HardGuard => "hard_guard",
            _ => "project_policy",
        };
        return Ok(ActionAuthorization::Deny {
            source,
            reason: gate
                .denial_reason
                .unwrap_or("authorization denied this action"),
        });
    }
    Ok(match authority.authorize(action)? {
        AuthorizationDecision::Allow => ActionAuthorization::Allow,
        AuthorizationDecision::Defer => ActionAuthorization::Defer,
        AuthorizationDecision::Deny => ActionAuthorization::Deny {
            source: "host",
            reason: "the host denied this action",
        },
    })
}
