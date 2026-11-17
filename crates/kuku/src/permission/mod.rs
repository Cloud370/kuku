pub(crate) mod gate;
pub(crate) mod policy;
mod runtime;

pub use runtime::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer, TrustedLocal};

pub use gate::{
    decide_tool_call, recover_session_grants, GateDecision, GateDecisionKind, GateScope,
    GateSource, SessionGrant,
};
pub use policy::{append_project_allow_rule, load_project_policy, parse_policy, PermissionPolicy};
