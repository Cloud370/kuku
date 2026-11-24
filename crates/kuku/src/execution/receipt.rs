use serde::{Deserialize, Serialize};

use super::{CommandTermination, FileCommitState, FileMutationKind, OutputReference};
use crate::WorkspaceScriptOperation;
use crate::event::{EventPayload, StoredEvent};

/// A durable action boundary inside one workspace script.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceActionReceipt {
    /// Agent that requested the operation.
    pub agent: String,
    /// Owning turn.
    pub turn: u64,
    /// ID of the parent script tool call.
    pub tool_call_id: String,
    /// Full host action ID used at authorization and dispatch.
    pub action_id: String,
    /// Workspace operation that the host observed.
    pub tool: WorkspaceScriptOperation,
    /// Resolved workspace-relative path or output ID when the operation selected one.
    pub target: Option<String>,
    /// An unmatched dispatched receipt leaves the side effect unknown after interruption.
    pub status: WorkspaceActionState,
    /// Facts available at this point in the operation.
    pub facts: WorkspaceActionDetails,
}

impl WorkspaceActionReceipt {
    /// Decode a workspace action, skipping other Session events.
    pub fn from_event(event: &StoredEvent) -> serde_json::Result<Option<Self>> {
        match &event.payload {
            EventPayload::Unknown(value) if value["kind"] == "workspace.action" => {
                serde_json::from_value(value.clone()).map(Some)
            }
            _ => Ok(None),
        }
    }
}

/// The recorded transition for an individual workspace operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceActionState {
    /// The host is about to dispatch the operation.
    Dispatched,
    /// Operation completed successfully.
    Ok,
    /// Operation returned an error.
    Error,
    /// The host refused dispatch.
    Blocked,
    /// Cancellation was observed.
    Cancelled,
}

/// Small receipt fields retained independently of the JS heap and output body.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceActionDetails {
    /// Digest of the exact prepared arguments at dispatch.
    pub args_sha256: Option<String>,
    /// Terminal category for an observed native invocation.
    pub termination: Option<CommandTermination>,
    /// Observed native exit code, when supplied by the OS.
    pub exit_code: Option<i32>,
    /// Retained extent produced by a command or finalized derived output.
    pub output_ref: Option<OutputReference>,
    /// Whether all observed process output was captured.
    pub capture_complete: Option<bool>,
    /// Whether output retention discarded bytes.
    pub quota_truncated: Option<bool>,
    /// Observed publication and durability state of a file mutation.
    pub commit_state: Option<FileCommitState>,
    /// File mutation responsible for this receipt.
    pub mutation: Option<FileMutationKind>,
    /// SHA-256 before a published file change.
    pub content_hash_before: Option<String>,
    /// SHA-256 after a published file change.
    pub content_hash_after: Option<String>,
    /// Total published file bytes.
    pub bytes_written: Option<usize>,
    /// Total removed file bytes.
    pub bytes_removed: Option<u64>,
    /// Whether owned process resources failed to settle within the cleanup bound.
    pub cleanup_incomplete: Option<bool>,
}
