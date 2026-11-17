use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::context::ToolSchema;
use crate::session::InputSource;
use crate::Result;

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
