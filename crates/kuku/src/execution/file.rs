use serde::{Deserialize, Serialize};

/// An observed mutation of one workspace file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMutationFacts {
    /// Stable structured result discriminator.
    pub kind: FileMutationKind,
    /// Display path relative to the workspace.
    pub path: String,
    /// Target path resolved before the operation.
    pub canonical_path: String,
    /// Observed publication and durability state.
    pub commit_state: FileCommitState,
    /// SHA-256 version of the previous file, when present.
    pub content_hash_before: Option<String>,
    /// SHA-256 version of the written file, when present.
    pub content_hash_after: Option<String>,
    /// Length in bytes of the published file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_written: Option<usize>,
    /// Length in bytes of the removed file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_removed: Option<u64>,
}

impl FileMutationFacts {
    /// Decode a structured file mutation result produced by the SDK.
    pub fn from_structured(value: &serde_json::Value) -> serde_json::Result<Self> {
        Self::deserialize(value)
    }
}

/// Workspace operation that produced the file mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileMutationKind {
    /// An existing file was edited by exact replacement.
    FileEdit,
    /// A file was created or overwritten.
    FileWrite,
    /// An existing file was removed.
    FileRemove,
}

/// Publication status of a workspace file modification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileCommitState {
    /// The filesystem acknowledged the requested change.
    Committed,
    /// The filesystem applied the change and a subsequent durability check failed.
    PublishedUnconfirmed,
}
