use std::path::Path;

use serde_json::Value;

use crate::conversation::address::ConversationAddress;
use crate::event::StoredEvent;
use crate::execution::{FileCommitState, FileMutationFacts, FileMutationKind};
use crate::tool::ToolResultEnvelope;

use super::common::{
    expected_version, file_read_error, file_version, remove_observed_file, require_version,
    required_path, resolve_remove_path, snapshot_stale_error, ExecutorRead, WriteEvidence,
    WriteSnapshotLookup,
};

pub(crate) fn remove_file(
    args: &Value,
    workspace: &Path,
    conversation: &ConversationAddress,
    prior_events: &[StoredEvent],
) -> ToolResultEnvelope {
    remove_file_with_evidence(
        args,
        workspace,
        WriteEvidence::Model {
            conversation,
            events: prior_events,
        },
    )
}

pub(crate) fn remove_file_for_executor(
    args: &Value,
    workspace: &Path,
    reads: &[ExecutorRead],
) -> ToolResultEnvelope {
    remove_file_with_evidence(args, workspace, WriteEvidence::Executor(reads))
}

fn remove_file_with_evidence(
    args: &Value,
    workspace: &Path,
    evidence: WriteEvidence<'_>,
) -> ToolResultEnvelope {
    let path = match required_path("remove_file", args) {
        Ok(path) => path,
        Err(result) => return result,
    };
    let expected = match expected_version(args) {
        Ok(expected) => expected,
        Err(result) => return result,
    };
    let resolved = match resolve_remove_path(workspace, path) {
        Ok(resolved) => resolved,
        Err(result) => return result,
    };
    let (before, metadata) = match file_version(&resolved.path) {
        Ok(version) => version,
        Err(error) => return file_read_error(Path::new(&resolved.relative), error),
    };
    if let Err(result) = require_version(&resolved.relative, expected, Some(&before)) {
        return result;
    }
    if expected.is_none() {
        if let Err(result) =
            require_remove_read(&resolved.path, &resolved.relative, &before, &evidence)
        {
            return result;
        }
    }
    let publication = match remove_observed_file(&resolved.path, &before) {
        Ok(publication) => publication,
        Err(error) => return error.result(&resolved.relative),
    };
    let summary = format!("removed {}", resolved.relative);
    publication.result(
        FileMutationFacts {
            kind: FileMutationKind::FileRemove,
            path: resolved.relative,
            canonical_path: resolved.path.to_string_lossy().into_owned(),
            commit_state: FileCommitState::Committed,
            content_hash_before: Some(before),
            content_hash_after: None,
            bytes_written: None,
            bytes_removed: Some(metadata.len()),
        },
        summary,
        serde_json::json!({}),
    )
}

fn require_remove_read(
    path: &Path,
    relative: &str,
    version: &str,
    evidence: &WriteEvidence<'_>,
) -> Result<(), ToolResultEnvelope> {
    match evidence.find(path, true, None, version) {
        WriteSnapshotLookup::Found(snapshot) if snapshot.content_hash == version => Ok(()),
        WriteSnapshotLookup::Found(snapshot) => Err(snapshot_stale_error(relative, snapshot.event_id)),
        WriteSnapshotLookup::Rejected(reason) => Err(ToolResultEnvelope::error_with_reason(
            format!("failed: fully read {relative} before removing"),
            format!("provide an expected_version from stat_file or a prior full read_file snapshot for {relative}"),
            reason,
        )),
    }
}

#[cfg(test)]
mod tests {
    use crate::tool::builtin::stat_file;

    use super::*;

    #[test]
    fn binary_removal_requires_and_revalidates_a_version() {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("二进制 & file.out");
        let original = b"\x00\xff\xfe";
        std::fs::write(&path, original).unwrap();
        let args = serde_json::json!({"path": "二进制 & file.out"});

        let cheap = stat_file(&args, workspace.path()).structured.unwrap();
        assert_eq!("file", cheap["entry_kind"]);
        assert_eq!(3, cheap["size_bytes"]);
        assert!(cheap.get("content_hash").is_none());
        let no_observation = remove_file_for_executor(&args, workspace.path(), &[]);
        assert_eq!(
            "snapshot_required",
            no_observation.structured.unwrap()["reason_code"]
        );

        let versioned = stat_file(
            &serde_json::json!({"path":"二进制 & file.out","version":true}),
            workspace.path(),
        )
        .structured
        .unwrap();
        let hash = versioned["content_hash"].as_str().unwrap();
        assert_eq!(crate::tool::builtin::common::content_hash(original), hash);
        let pinned = serde_json::json!({"path":"二进制 & file.out","expected_version":hash});

        std::fs::write(&path, b"\xff\x00").unwrap();
        let stale = remove_file_for_executor(&pinned, workspace.path(), &[]);
        assert_eq!("snapshot_stale", stale.structured.unwrap()["reason_code"]);
        assert_eq!(b"\xff\x00", std::fs::read(&path).unwrap().as_slice());
        std::fs::write(&path, original).unwrap();

        let removed = remove_file_for_executor(&pinned, workspace.path(), &[]);
        assert_eq!("ok", removed.status);
        let facts =
            FileMutationFacts::from_structured(removed.structured.as_ref().unwrap()).unwrap();
        assert_eq!(FileMutationKind::FileRemove, facts.kind);
        assert_eq!(FileCommitState::Committed, facts.commit_state);
        assert_eq!(Some(3), facts.bytes_removed);
        assert_eq!(Some(hash), facts.content_hash_before.as_deref());
        assert!(!path.exists());
        let absent = stat_file(&args, workspace.path()).structured.unwrap();
        assert_eq!("missing", absent["entry_kind"]);
        assert_eq!(false, absent["exists"]);
    }
}
