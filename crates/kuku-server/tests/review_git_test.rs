#![allow(dead_code, unused_imports)]

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use kuku_server::api::{
    ApiError, ApiErrorCode, ChangeKind, ChangesAvailability, DiffLineKind,
    RegisterWorkspaceRequest, RevisionToken,
};
use kuku_server::platform::{
    RegistrationRootRegistry, RegistrationRootSpec, ServerRevisionCoordinator, WorkspaceCapability,
    WorkspaceRegistry, WorkspaceUsagePort,
};

mod api {
    pub use kuku_server::api::*;
}

mod platform {
    pub use kuku_server::platform::*;
}

mod run_manager {
    pub use kuku_server::run_manager::*;
}

#[path = "../src/review/mod.rs"]
pub mod review;

#[path = "../src/review/git.rs"]
mod git;

use git::{parse_numstat, parse_porcelain_v2, GitReviewService};
use review::ReviewLimits;

struct UnusedUsage;

impl WorkspaceUsagePort for UnusedUsage {
    fn has_durable_tasks<'a>(
        &'a self,
        _id: &'a kuku_server::api::WorkspaceId,
    ) -> Pin<Box<dyn Future<Output = Result<bool, ApiError>> + Send + 'a>> {
        Box::pin(async { Ok(false) })
    }
}

struct Repository {
    _allowed: tempfile::TempDir,
    _home: tempfile::TempDir,
    root: PathBuf,
    capability: WorkspaceCapability,
}

impl Repository {
    async fn new() -> Self {
        let allowed = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let root = allowed.path().join("project");
        std::fs::create_dir(&root).unwrap();
        git_at(&root, &["init", "-q"]);
        git_at(&root, &["config", "user.email", "review@example.invalid"]);
        git_at(&root, &["config", "user.name", "Review Test"]);
        let roots = RegistrationRootRegistry::from_server_config(
            home.path(),
            vec![RegistrationRootSpec {
                label: "Review fixtures".to_owned(),
                path: allowed.path().to_owned(),
            }],
        )
        .unwrap();
        let revisions = ServerRevisionCoordinator::open(home.path());
        let registry =
            WorkspaceRegistry::open(home.path(), roots, Arc::new(UnusedUsage), revisions).unwrap();
        let root_id = registry.registration_roots().list()[0].root_id.clone();
        let expected_revision = registry.revision().await.unwrap();
        let workspace = registry
            .register(RegisterWorkspaceRequest {
                root_id,
                relative_path: "project".to_owned(),
                label: "Project".to_owned(),
                expected_revision,
            })
            .await
            .unwrap();
        let capability = registry.capability(&workspace.workspace_id).unwrap();
        Self {
            _allowed: allowed,
            _home: home,
            root,
            capability,
        }
    }

    fn write(&self, path: &str, contents: impl AsRef<[u8]>) {
        let path = self.root.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    fn remove(&self, path: &str) {
        std::fs::remove_file(self.root.join(path)).unwrap();
    }

    fn git(&self, args: &[&str]) {
        git_at(&self.root, args);
    }

    fn commit_all(&self, message: &str) {
        self.git(&["add", "--all"]);
        self.git(&["commit", "-qm", message]);
    }

    fn service(&self) -> GitReviewService {
        GitReviewService::new(self.capability.clone(), ReviewLimits::default())
    }
}

fn git_at(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env_remove("GIT_CONFIG_GLOBAL")
        .env_remove("GIT_DIR")
        .env_remove("GIT_EXTERNAL_DIFF")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn entry<'a>(
    snapshot: &'a kuku_server::api::ReviewSnapshot,
    path: &str,
) -> &'a kuku_server::api::ChangeEntry {
    snapshot
        .entries
        .iter()
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("missing change entry for {path}"))
}

#[test]
fn porcelain_v2_parser_covers_every_status_and_literal_path() {
    let records = b"1 M. N... 100644 100644 100644 aaaaaaa bbbbbbb staged.txt\0\
1 .M N... 100644 100644 100644 aaaaaaa aaaaaaa modified.txt\0\
1 MM N... 100644 100644 100644 aaaaaaa bbbbbbb mixed.txt\0\
1 A. N... 000000 100644 100644 0000000 bbbbbbb added.txt\0\
1 .D N... 100644 100644 000000 aaaaaaa aaaaaaa deleted.txt\0\
1 T. N... 100644 120000 120000 aaaaaaa bbbbbbb type.txt\0\
2 R. N... 100644 100644 100644 aaaaaaa bbbbbbb R100 renamed.txt\0old.txt\0\
2 C. N... 100644 100644 100644 aaaaaaa bbbbbbb C100 copied.txt\0source.txt\0\
u UU N... 100644 100644 100644 100644 aaaaaaa bbbbbbb ccccccc conflict.txt\0\
? -option.txt\0";

    let changes = parse_porcelain_v2(records).unwrap();

    assert_eq!(10, changes.len());
    assert_eq!(ChangeKind::Modified, changes[0].kind);
    assert!(changes[0].staged);
    assert!(!changes[0].worktree);
    assert!(!changes[1].staged);
    assert!(changes[1].worktree);
    assert!(changes[2].staged && changes[2].worktree);
    assert_eq!(ChangeKind::Added, changes[3].kind);
    assert_eq!(ChangeKind::Deleted, changes[4].kind);
    assert_eq!(ChangeKind::TypeChanged, changes[5].kind);
    assert_eq!(ChangeKind::Renamed, changes[6].kind);
    assert_eq!(Some("old.txt"), changes[6].old_path.as_deref());
    assert_eq!(ChangeKind::Copied, changes[7].kind);
    assert_eq!(ChangeKind::Conflicted, changes[8].kind);
    assert_eq!(ChangeKind::Untracked, changes[9].kind);
    assert_eq!("-option.txt", changes[9].path);
}

#[test]
fn numstat_parser_handles_rename_binary_and_overflow() {
    let stats = parse_numstat(
        b"12\t3\tplain.txt\0-\t-\tbinary.bin\0\
4\t5\t\0old.txt\0new.txt\0",
    )
    .unwrap();

    assert_eq!(Some((Some(12), Some(3))), stats.get("plain.txt").copied());
    assert_eq!(Some((None, None)), stats.get("binary.bin").copied());
    assert_eq!(Some((Some(4), Some(5))), stats.get("new.txt").copied());
    assert!(parse_numstat(b"4294967296\t0\thuge.txt\0").is_err());
}

#[tokio::test]
async fn snapshot_reports_aggregate_states_paths_and_exact_statistics() {
    let repo = Repository::new().await;
    repo.write("modified.txt", "one\ntwo\n");
    repo.write("staged.txt", "old\n");
    repo.write("mixed.txt", "base\n");
    repo.write("deleted.txt", "gone\n");
    repo.write("rename-old.txt", "rename\n");
    repo.write("binary.bin", [0, 1, 2, 3]);
    repo.commit_all("base");
    repo.write("modified.txt", "one\nchanged\nextra\n");
    repo.write("staged.txt", "new\nline\n");
    repo.git(&["add", "--", "staged.txt"]);
    repo.write("mixed.txt", "index\n");
    repo.git(&["add", "--", "mixed.txt"]);
    repo.write("mixed.txt", "worktree\nextra\n");
    repo.remove("deleted.txt");
    repo.git(&["mv", "--", "rename-old.txt", "rename-new.txt"]);
    repo.write("added.txt", "a\nb\nc\n");
    repo.git(&["add", "--", "added.txt"]);
    repo.write("untracked.txt", "u1\nu2\n");
    repo.write("untracked-binary.bin", [0, 9, 8]);
    repo.write("-option.txt", "literal\n");
    repo.write("binary.bin", [0, 4, 5, 6]);
    #[cfg(unix)]
    std::os::unix::fs::symlink("modified.txt", repo.root.join("link.txt")).unwrap();

    let snapshot = repo.service().snapshot(None, 100).await.unwrap();

    assert_eq!(ChangesAvailability::Available, snapshot.availability);
    assert_eq!(ChangeKind::Modified, entry(&snapshot, "modified.txt").kind);
    assert!(entry(&snapshot, "staged.txt").staged);
    assert!(!entry(&snapshot, "staged.txt").worktree);
    assert!(entry(&snapshot, "mixed.txt").staged);
    assert!(entry(&snapshot, "mixed.txt").worktree);
    assert_eq!(Some(3), entry(&snapshot, "added.txt").additions);
    assert_eq!(Some(1), entry(&snapshot, "deleted.txt").deletions);
    assert_eq!(
        Some("rename-old.txt"),
        entry(&snapshot, "rename-new.txt").old_path.as_deref()
    );
    assert_eq!(
        ChangeKind::Untracked,
        entry(&snapshot, "untracked.txt").kind
    );
    assert_eq!(Some(2), entry(&snapshot, "untracked.txt").additions);
    assert_eq!(None, entry(&snapshot, "untracked-binary.bin").additions);
    assert_eq!(None, entry(&snapshot, "binary.bin").deletions);
    assert_eq!(ChangeKind::Untracked, entry(&snapshot, "-option.txt").kind);
    #[cfg(unix)]
    assert_eq!(ChangeKind::Untracked, entry(&snapshot, "link.txt").kind);
}

#[tokio::test]
async fn revisions_cover_head_index_worktree_untracked_and_rename_dimensions() {
    let repo = Repository::new().await;
    repo.write("a.txt", "aaa\n");
    repo.write("b.txt", "bbb\n");
    repo.commit_all("base");
    repo.write("a.txt", "one\n");
    repo.write("u.txt", "uuu\n");
    let service = repo.service();
    let first = service.snapshot(None, 100).await.unwrap();
    let a_first = entry(&first, "a.txt").revision.clone();
    let u_first = entry(&first, "u.txt").revision.clone();

    repo.write("a.txt", "two\n");
    let second = service.snapshot(None, 100).await.unwrap();
    assert_ne!(first.revision, second.revision);
    assert_ne!(a_first, entry(&second, "a.txt").revision);
    assert_eq!(u_first, entry(&second, "u.txt").revision);

    repo.git(&["add", "--", "a.txt"]);
    let indexed = service.snapshot(None, 100).await.unwrap();
    assert_ne!(second.revision, indexed.revision);
    assert_ne!(
        entry(&second, "a.txt").revision,
        entry(&indexed, "a.txt").revision
    );

    let metadata = std::fs::metadata(repo.root.join("u.txt")).unwrap();
    let modified = metadata.modified().unwrap();
    repo.write("u.txt", "vvv\n");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(repo.root.join("u.txt"))
        .unwrap();
    file.set_modified(modified).unwrap();
    let replaced = service.snapshot(None, 100).await.unwrap();
    assert_ne!(indexed.revision, replaced.revision);
    assert_ne!(
        entry(&indexed, "u.txt").revision,
        entry(&replaced, "u.txt").revision
    );

    repo.git(&["mv", "--", "b.txt", "renamed.txt"]);
    let renamed = service.snapshot(None, 100).await.unwrap();
    assert_ne!(replaced.revision, renamed.revision);
    assert_eq!(
        Some("b.txt"),
        entry(&renamed, "renamed.txt").old_path.as_deref()
    );

    repo.commit_all("move head");
    repo.write("a.txt", "after-head\n");
    let moved_head = service.snapshot(None, 100).await.unwrap();
    assert_ne!(renamed.revision, moved_head.revision);
}

#[tokio::test]
async fn diff_revision_matches_entry_and_pages_reassemble_without_gaps() {
    let repo = Repository::new().await;
    let base = (0..120)
        .map(|line| format!("old-{line}\n"))
        .collect::<String>();
    repo.write("large.txt", base);
    repo.commit_all("base");
    let changed = (0..120)
        .map(|line| format!("new-{line}-{}\n", "x".repeat(256)))
        .collect::<String>();
    repo.write("large.txt", changed);
    let service = repo.service();
    let snapshot = service.snapshot(None, 100).await.unwrap();
    let revision = entry(&snapshot, "large.txt").revision.clone();
    let mut cursor = None;
    let mut assembled = Vec::new();
    let mut pages = 0;

    loop {
        let page = service
            .diff("large.txt", &revision, cursor.as_ref(), 17)
            .await
            .unwrap();
        assert_eq!(revision, page.revision);
        for hunk in page.hunks {
            assembled.extend(hunk.lines.into_iter().map(|line| (line.kind, line.text)));
        }
        pages += 1;
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }

    assert!(pages > 2);
    assert_eq!(240, assembled.len());
    assert_eq!(
        120,
        assembled
            .iter()
            .filter(|(_, line)| line.starts_with("old-"))
            .count()
    );
    assert_eq!(
        120,
        assembled
            .iter()
            .filter(|(_, line)| line.starts_with("new-"))
            .count()
    );
    assert!(assembled
        .iter()
        .any(|(kind, _)| *kind == DiffLineKind::Deletion));
    assert!(assembled
        .iter()
        .any(|(kind, _)| *kind == DiffLineKind::Addition));
}

#[tokio::test]
async fn binary_and_untracked_diffs_are_bounded_and_revision_addressed() {
    let repo = Repository::new().await;
    repo.write("binary.bin", [0, 1, 2]);
    repo.commit_all("base");
    repo.write("binary.bin", [0, 3, 4]);
    repo.write("untracked.txt", "first\nsecond\n");
    repo.write("untracked-no-newline.txt", "last line");
    let service = repo.service();
    let snapshot = service.snapshot(None, 100).await.unwrap();

    let binary = service
        .diff(
            "binary.bin",
            &entry(&snapshot, "binary.bin").revision,
            None,
            100,
        )
        .await
        .unwrap();
    assert!(binary.binary);
    assert!(binary.hunks.is_empty());
    let untracked = service
        .diff(
            "untracked.txt",
            &entry(&snapshot, "untracked.txt").revision,
            None,
            1,
        )
        .await
        .unwrap();
    assert!(!untracked.binary);
    assert!(untracked.truncated);
    assert!(untracked.next_cursor.is_some());
    let no_newline = service
        .diff(
            "untracked-no-newline.txt",
            &entry(&snapshot, "untracked-no-newline.txt").revision,
            None,
            100,
        )
        .await
        .unwrap();
    assert!(no_newline
        .hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .any(|line| line.kind == DiffLineKind::NoNewlineMarker));
}

#[tokio::test]
async fn stale_diff_revision_is_outdated_and_budgets_fail_closed() {
    let repo = Repository::new().await;
    repo.write("change.txt", "base\n");
    repo.commit_all("base");
    repo.write("change.txt", "changed\n");
    let service = repo.service();
    let snapshot = service.snapshot(None, 100).await.unwrap();
    let revision = entry(&snapshot, "change.txt").revision.clone();
    repo.write("change.txt", "newer!!\n");
    let error = service
        .diff("change.txt", &revision, None, 100)
        .await
        .unwrap_err();
    assert_eq!(ApiErrorCode::Outdated, error.code());

    let limits = ReviewLimits {
        revision_git_entries: 1,
        revision_git_hash_bytes: 4,
        ..ReviewLimits::default()
    };
    let exhausted = GitReviewService::new(repo.capability.clone(), limits);
    let unavailable = exhausted.snapshot(None, 100).await.unwrap();
    assert_eq!(
        ChangesAvailability::GitUnavailable,
        unavailable.availability
    );
    assert!(unavailable.entries.is_empty());
    let error = exhausted
        .diff("change.txt", &revision, None, 100)
        .await
        .unwrap_err();
    assert_eq!(ApiErrorCode::PayloadTooLarge, error.code());
}

#[tokio::test]
async fn races_retry_once_then_fail_closed() {
    let repo = Repository::new().await;
    repo.write("race.txt", "base\n");
    repo.commit_all("base");
    repo.write("race.txt", "first\n");
    let root = repo.root.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let hook_calls = Arc::clone(&calls);
    let hook = Arc::new(move || {
        let call = hook_calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            std::fs::write(root.join("race.txt"), "second\n").unwrap();
        }
    });
    let service = GitReviewService::new_with_capture_hook(
        repo.capability.clone(),
        ReviewLimits::default(),
        hook,
    );
    let snapshot = service.snapshot(None, 100).await.unwrap();
    assert_eq!(ChangesAvailability::Available, snapshot.availability);
    assert!(calls.load(Ordering::SeqCst) >= 2);

    let root = repo.root.clone();
    let sequence = Arc::new(AtomicUsize::new(0));
    let hook_sequence = Arc::clone(&sequence);
    let always_mutate = Arc::new(move || {
        let next = hook_sequence.fetch_add(1, Ordering::SeqCst);
        std::fs::write(root.join("race.txt"), format!("mutation-{next}\n")).unwrap();
    });
    let unstable = GitReviewService::new_with_capture_hook(
        repo.capability.clone(),
        ReviewLimits::default(),
        always_mutate,
    );
    let unavailable = unstable.snapshot(None, 100).await.unwrap();
    assert_eq!(
        ChangesAvailability::GitUnavailable,
        unavailable.availability
    );
    assert!(unavailable.entries.is_empty());
}

#[tokio::test]
async fn non_git_and_mismatched_roots_never_publish_changes() {
    let non_git = Repository::new().await;
    std::fs::remove_dir_all(non_git.root.join(".git")).unwrap();
    let snapshot = non_git.service().snapshot(None, 100).await.unwrap();
    assert_eq!(ChangesAvailability::NotGitRepository, snapshot.availability);
    assert!(snapshot.entries.is_empty());

    let nested = Repository::new().await;
    std::fs::create_dir(nested.root.join("nested")).unwrap();
    nested.git(&["init", "-q", "nested"]);
    std::fs::rename(
        nested.root.join(".git"),
        nested.root.join("nested/.parent-git"),
    )
    .unwrap();
    let snapshot = nested.service().snapshot(None, 100).await.unwrap();
    assert_eq!(ChangesAvailability::NotGitRepository, snapshot.availability);

    let enclosing = Repository::new().await;
    std::fs::rename(
        enclosing.root.join(".git"),
        enclosing.root.parent().unwrap().join(".git"),
    )
    .unwrap();
    let snapshot = enclosing.service().snapshot(None, 100).await.unwrap();
    assert_eq!(
        ChangesAvailability::WorkspaceRootMismatch,
        snapshot.availability
    );
}

#[tokio::test]
async fn hostile_git_environment_attributes_and_config_cannot_run_helpers() {
    let repo = Repository::new().await;
    repo.write("tracked.txt", "old\n");
    repo.commit_all("base");
    repo.write("tracked.txt", "new\n");
    repo.write(".gitattributes", "*.txt diff=hostile\n");
    let marker = repo.root.join("helper-ran");
    let helper = repo.root.join("helper.sh");
    std::fs::write(
        &helper,
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    repo.git(&["config", "diff.hostile.command", helper.to_str().unwrap()]);
    repo.git(&["config", "core.fsmonitor", helper.to_str().unwrap()]);
    std::env::set_var("GIT_EXTERNAL_DIFF", &helper);
    std::env::set_var("GIT_DIR", repo.root.join("missing-git-dir"));
    std::env::set_var("GIT_CONFIG_GLOBAL", &helper);

    let snapshot = repo.service().snapshot(None, 100).await.unwrap();

    std::env::remove_var("GIT_EXTERNAL_DIFF");
    std::env::remove_var("GIT_DIR");
    std::env::remove_var("GIT_CONFIG_GLOBAL");
    assert_eq!(ChangesAvailability::Available, snapshot.availability);
    assert!(!marker.exists());
}

#[tokio::test]
async fn repository_local_filter_helpers_never_execute() {
    let repo = Repository::new().await;
    repo.write("filtered.txt", "base\n");
    repo.commit_all("base");
    let marker = repo.root.join("filter-ran");
    let helper = repo.root.join("filter.sh");
    std::fs::write(
        &helper,
        format!("#!/bin/sh\ntouch '{}'\ncat\n", marker.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    repo.git(&["config", "filter.hostile.clean", helper.to_str().unwrap()]);
    repo.git(&["config", "filter.hostile.smudge", helper.to_str().unwrap()]);
    repo.write(".gitattributes", "filtered.txt filter=hostile\n");
    repo.write("filtered.txt", "changed\n");

    let snapshot = repo.service().snapshot(None, 100).await.unwrap();

    assert_eq!(ChangesAvailability::Available, snapshot.availability);
    assert!(!marker.exists(), "repository-local filter helper executed");
}

#[tokio::test]
async fn head_side_identity_and_current_mode_change_revisions() {
    let repo = Repository::new().await;
    repo.write("tracked.txt", "base\n");
    repo.commit_all("base");
    repo.write("tracked.txt", "worktree\n");
    let service = repo.service();
    let first = service.snapshot(None, 100).await.unwrap();
    let first_revision = entry(&first, "tracked.txt").revision.clone();
    repo.write("tracked.txt", "head-two\n");
    repo.commit_all("head two");
    repo.write("tracked.txt", "worktree\n");
    let second = service.snapshot(None, 100).await.unwrap();
    assert_ne!(first.revision, second.revision);
    assert_ne!(first_revision, entry(&second, "tracked.txt").revision);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            repo.root.join("tracked.txt"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let third = service.snapshot(None, 100).await.unwrap();
        assert_ne!(second.revision, third.revision);
        assert_ne!(
            entry(&second, "tracked.txt").revision,
            entry(&third, "tracked.txt").revision
        );
    }
}

#[tokio::test]
async fn oversized_untracked_content_is_rejected_before_diff_allocation() {
    let repo = Repository::new().await;
    repo.write("huge.txt", "x\n".repeat(2 * 1024 * 1024 + 1));
    let limits = ReviewLimits {
        diff_bytes: 1024,
        ..ReviewLimits::default()
    };
    let service = GitReviewService::new(repo.capability.clone(), limits);
    let snapshot = service.snapshot(None, 100).await.unwrap();
    assert_eq!(ChangesAvailability::GitUnavailable, snapshot.availability);
    assert!(snapshot.entries.is_empty());
}

#[tokio::test]
async fn revision_deadline_is_shared_by_probe_and_all_git_commands() {
    let repo = Repository::new().await;
    repo.write("slow.txt", "content\n");
    let limits = ReviewLimits {
        revision_deadline: std::time::Duration::from_millis(1),
        ..ReviewLimits::default()
    };
    let service = GitReviewService::new(repo.capability.clone(), limits);
    let snapshot = service.snapshot(None, 100).await.unwrap();
    assert_eq!(ChangesAvailability::GitUnavailable, snapshot.availability);
}

#[test]
fn revision_tokens_are_lowercase_sha256_without_prefixes() {
    let token = RevisionToken::parse("0123456789abcdef".repeat(4)).unwrap();
    assert_eq!(64, token.as_str().len());
}
