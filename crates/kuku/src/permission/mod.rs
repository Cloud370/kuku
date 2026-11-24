pub(crate) mod gate;
pub(crate) mod policy;
mod runtime;

pub(crate) use runtime::{ActionAuthorization, authorize_action};
pub use runtime::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer, TrustedLocal};

pub use gate::{
    GateDecision, GateDecisionKind, GateScope, GateSource, SessionGrant, decide_tool_call,
    recover_session_grants,
};
pub use policy::{PermissionPolicy, append_project_allow_rule, load_project_policy, parse_policy};
