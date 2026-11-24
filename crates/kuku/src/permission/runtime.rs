use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::context::ToolSchema;
use crate::session::InputSource;

/// Default authorization for the Agents in an open Session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    /// Permit tools subject to the embedding host's policy.
    #[default]
    Full,
    /// Defer writes and execution for approval.
    Ask,
    /// Permit reads and reject writes and execution.
    ReadOnly,
}

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
    Defer {
        source: &'static str,
    },
}

impl ActionAuthorization {
    pub(crate) fn rejection_after_approval(self, approved_source: &str) -> Option<&'static str> {
        match self {
            Self::Allow => None,
            Self::Defer { source } if source == approved_source => None,
            Self::Defer { .. } => Some("the active policy requires a different approval"),
            Self::Deny { reason, .. } => Some(reason),
        }
    }
}

pub(crate) fn authorize_action(
    mode: PermissionMode,
    read_only: bool,
    authority: &dyn ToolAuthorizer,
    action: &ToolAuthorization,
) -> Result<ActionAuthorization> {
    if action.tool.name == "run_command" {
        if let Some(reason) =
            crate::tool::builtin::run_command::prepared_command_guard(&action.args)
        {
            return Ok(ActionAuthorization::Deny {
                source: "hard_guard",
                reason,
            });
        }
    }
    if mode == PermissionMode::ReadOnly && !read_only {
        return Ok(ActionAuthorization::Deny {
            source: "permission_mode",
            reason: "read-only mode permits reads only",
        });
    }
    Ok(match authority.authorize(action)? {
        AuthorizationDecision::Allow if mode == PermissionMode::Ask && !read_only => {
            ActionAuthorization::Defer {
                source: "permission_mode",
            }
        }
        AuthorizationDecision::Allow => ActionAuthorization::Allow,
        AuthorizationDecision::Defer => ActionAuthorization::Defer { source: "host" },
        AuthorizationDecision::Deny => ActionAuthorization::Deny {
            source: "host",
            reason: "the host denied this action",
        },
    })
}
