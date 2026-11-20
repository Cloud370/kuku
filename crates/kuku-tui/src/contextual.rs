use kuku_client::Target;
use ratatui::layout::Rect;

use crate::app::Address;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingCommandIdentity {
    pub(crate) target: Target,
    pub(crate) command_id: String,
    pub(crate) kind: PendingCommandKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingCommandKind {
    Send,
    Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContextControl {
    Request {
        target: Target,
        request_id: String,
    },
    RepairCredentials {
        request: crate::setup::CredentialRepairRequest,
    },
    Continue {
        target: Target,
        execution_id: Option<u64>,
        inputs: Vec<String>,
    },
    Interrupt {
        target: Target,
        execution_id: u64,
    },
    PendingRecovery {
        commands: Vec<PendingCommandIdentity>,
    },
    PreviousAgent {
        target: Address,
    },
    Reconnect {
        endpoint: Option<String>,
    },
    Restart {
        endpoint: Option<String>,
    },
    Messages,
}

impl ContextControl {
    pub(crate) fn is_send_recovery(&self) -> bool {
        matches!(self, Self::PendingRecovery { commands }
            if !commands.is_empty() && commands.iter().all(|command| command.kind == PendingCommandKind::Send))
    }

    pub(crate) fn is_service(&self) -> bool {
        matches!(self, Self::Reconnect { .. } | Self::Restart { .. })
    }
}

#[derive(Debug, Default)]
pub(crate) struct ContextFocus {
    pub(crate) cursor: Option<ContextControl>,
    pub(crate) active: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ContextHit {
    pub(crate) area: Rect,
    pub(crate) control: ContextControl,
}
