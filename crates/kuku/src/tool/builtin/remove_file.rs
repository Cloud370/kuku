use std::path::Path;

use serde_json::Value;

use crate::conversation::address::ConversationAddress;
use crate::event::StoredEvent;
use crate::execution::{FileCommitState, FileMutationFacts, FileMutationKind};
use crate::tool::ToolResultEnvelope;

use super::common::{
    ExecutorObservation, FileCancellation, FileScope, WorkspaceCommitError, WriteEvidence,
    WriteSnapshotLookup, expected_version, file_read_error, file_version, remove_observed_file,
    required_path, resolve_remove_path, snapshot_stale_error,
};

pub(crate) fn remove_file(
    args: &Value,
    scope: FileScope<'_>,
    conversation: &ConversationAddress,
    prior_events: &[StoredEvent],
    scan_bytes: u64,
    cancelled: &FileCancellation<'_>,
) -> ToolResultEnvelope {
    remove_file_with_evidence(
        args,
        scope,
        WriteEvidence::Model {
            conversation,
            events: prior_events,
        },
        scan_bytes,
        cancelled,
    )
}

pub(crate) fn remove_file_for_executor(
    args: &Value,
    scope: FileScope<'_>,
    reads: &[ExecutorObservation],
    scan_bytes: u64,
    cancelled: &FileCancellation<'_>,
) -> ToolResultEnvelope {
    remove_file_with_evidence(
        args,
        scope,
        WriteEvidence::Executor(reads),
        scan_bytes,
        cancelled,
    )
}

fn remove_file_with_evidence(
    args: &Value,
    scope: FileScope<'_>,
    evidence: WriteEvidence<'_>,
    scan_bytes: u64,
    cancelled: &FileCancellation<'_>,
) -> ToolResultEnvelope {
    let path = match required_path("remove_file", args) {
        Ok(path) => path,
        Err(result) => return result,
    };
    let expected = match expected_version(args) {
        Ok(expected) => expected,
        Err(result) => return result,
    };
    let resolved = match scope.resolve(path, resolve_remove_path) {
        Ok(resolved) => resolved,
        Err(result) => return result,
    };
    let before = match expected {
        Some(expected) => expected.to_owned(),
        None => {
            let (before, _) = match file_version(&resolved.path, scan_bytes, cancelled) {
                Ok(version) => version,
                Err(error) => return file_read_error(Path::new(&resolved.relative), error),
            };
            if let Err(result) =
                require_remove_read(&resolved.path, &resolved.relative, &before, &evidence)
            {
                return result;
            }
            before
        }
    };
    let (publication, before, bytes_removed) =
        match remove_observed_file(&resolved.path, &before, scan_bytes, cancelled) {
            Ok(outcome) => outcome,
            Err(err @ WorkspaceCommitError::Conflict) => {
                return stale_remove(err.result(&resolved.relative), &evidence);
            }
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
            bytes_removed: Some(bytes_removed),
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
    match evidence.find_full_read(path, version) {
        WriteSnapshotLookup::Found(snapshot) if snapshot.content_hash == version => Ok(()),
        WriteSnapshotLookup::Found(snapshot) => Err(stale_remove(
            snapshot_stale_error(relative, snapshot.event_id),
            evidence,
        )),
        WriteSnapshotLookup::Rejected(reason) => Err(ToolResultEnvelope::error_with_reason(
            format!("failed: current version of {relative} required for removal"),
            format!(
                "{} A full read of the current file also supplies version evidence.",
                remove_version_hint(evidence)
            ),
            reason,
        )),
    }
}

fn stale_remove(
    mut result: ToolResultEnvelope,
    evidence: &WriteEvidence<'_>,
) -> ToolResultEnvelope {
    result.model_content = format!(
        "The target no longer matches the observed version. Confirm removal is still intended before retrying. {}",
        remove_version_hint(evidence),
    );
    result
}

fn remove_version_hint(evidence: &WriteEvidence<'_>) -> &'static str {
    if evidence.is_model() {
        "Use stat_file({path, version: true}) on the same path. Pass the returned content_hash as expected_version to remove_file."
    } else {
        "Use tools.stat_file({path, version: true}) on the same path. If result.content_hash is present, pass it as expected_version to tools.remove_file."
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

        let cheap = stat_file(
            &args,
            FileScope::Unprepared(workspace.path()),
            u64::MAX,
            &|| false,
        )
        .structured
        .unwrap();
        assert_eq!("file", cheap["entry_kind"]);
        assert_eq!(3, cheap["size_bytes"]);
        assert!(cheap.get("content_hash").is_none());
        let no_observation = remove_file_for_executor(
            &args,
            FileScope::Unprepared(workspace.path()),
            &[],
            u64::MAX,
            &|| false,
        );
        assert!(
            no_observation
                .model_content
                .contains("tools.stat_file({path, version: true})")
        );
        assert!(
            no_observation
                .model_content
                .contains("pass it as expected_version to tools.remove_file")
        );
        assert_eq!(
            "snapshot_required",
            no_observation.structured.unwrap()["reason_code"]
        );

        let versioned = stat_file(
            &serde_json::json!({"path":"二进制 & file.out","version":true}),
            FileScope::Unprepared(workspace.path()),
            u64::MAX,
            &|| false,
        )
        .structured
        .unwrap();
        let hash = versioned["content_hash"].as_str().unwrap();
        assert_eq!(crate::tool::builtin::common::content_hash(original), hash);
        let pinned = serde_json::json!({"path":"二进制 & file.out","expected_version":hash});
        let digest = hash.strip_prefix("sha256:").unwrap();
        let bare = serde_json::json!({"path":"二进制 & file.out","expected_version":digest.to_ascii_uppercase()});

        std::fs::write(&path, b"\xff\x00").unwrap();
        let stale = remove_file_for_executor(
            &pinned,
            FileScope::Unprepared(workspace.path()),
            &[],
            u64::MAX,
            &|| false,
        );
        assert!(
            stale
                .model_content
                .contains("Confirm removal is still intended before retrying.")
        );
        assert_eq!("snapshot_stale", stale.structured.unwrap()["reason_code"]);
        let stale_bare = remove_file_for_executor(
            &bare,
            FileScope::Unprepared(workspace.path()),
            &[],
            u64::MAX,
            &|| false,
        );
        assert_eq!(
            "snapshot_stale",
            stale_bare.structured.unwrap()["reason_code"]
        );
        assert_eq!(b"\xff\x00", std::fs::read(&path).unwrap().as_slice());
        std::fs::write(&path, original).unwrap();

        let removed = remove_file_for_executor(
            &bare,
            FileScope::Unprepared(workspace.path()),
            &[],
            u64::MAX,
            &|| false,
        );
        assert_eq!("ok", removed.status);
        let facts =
            FileMutationFacts::from_structured(removed.structured.as_ref().unwrap()).unwrap();
        assert_eq!(FileMutationKind::FileRemove, facts.kind);
        assert_eq!(FileCommitState::Committed, facts.commit_state);
        assert_eq!(Some(3), facts.bytes_removed);
        assert_eq!(Some(hash), facts.content_hash_before.as_deref());
        assert!(!path.exists());
        let absent = stat_file(
            &args,
            FileScope::Unprepared(workspace.path()),
            u64::MAX,
            &|| false,
        )
        .structured
        .unwrap();
        assert_eq!("missing", absent["entry_kind"]);
        assert_eq!(false, absent["exists"]);
    }

    #[test]
    fn direct_removal_error_points_to_the_versioned_stat_call() {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("scratch.bin");
        std::fs::write(&path, b"\xff\0").unwrap();
        let result = remove_file(
            &serde_json::json!({"path":"scratch.bin"}),
            FileScope::Unprepared(workspace.path()),
            &ConversationAddress::parse("main").unwrap(),
            &[],
            u64::MAX,
            &|| false,
        );
        assert!(
            result
                .model_content
                .starts_with("Use stat_file({path, version: true})")
        );
        assert!(
            result
                .model_content
                .contains("content_hash as expected_version")
        );
        assert_eq!(b"\xff\0", std::fs::read(path).unwrap().as_slice());
    }
}
