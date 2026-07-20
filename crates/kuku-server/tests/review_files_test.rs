use std::fs::{File, FileTimes};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use kuku_server::api::{ApiError, ApiErrorCode, FileKind, RegisterWorkspaceRequest, WorkspaceId};
use kuku_server::platform::{
    RegistrationRootRegistry, RegistrationRootSpec, ServerRevisionCoordinator, WorkspaceRegistry,
    WorkspaceUsagePort,
};

mod api {
    pub use kuku_server::api::*;
}

mod platform {
    pub use kuku_server::platform::WorkspaceCapability;
}

#[path = "../src/review/files.rs"]
pub mod files;
#[path = "../src/review/mod.rs"]
pub mod review;

use files::WorkspaceReadService;
use review::{ReviewLimits, WorkspaceCapabilityProvider};

struct NoWorkspaceUsage(AtomicBool);

impl WorkspaceUsagePort for NoWorkspaceUsage {
    fn has_durable_tasks<'a>(
        &'a self,
        _id: &'a WorkspaceId,
    ) -> Pin<Box<dyn Future<Output = Result<bool, ApiError>> + Send + 'a>> {
        Box::pin(async move { Ok(self.0.load(std::sync::atomic::Ordering::SeqCst)) })
    }
}

struct RegistryProvider(Arc<WorkspaceRegistry>);

impl WorkspaceCapabilityProvider for RegistryProvider {
    fn capability(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<kuku_server::platform::WorkspaceCapability, ApiError> {
        self.0.capability(workspace_id)
    }
}

struct TestWorkspace {
    _home: tempfile::TempDir,
    _allowed: tempfile::TempDir,
    project: std::path::PathBuf,
    workspace_id: WorkspaceId,
    provider: Arc<RegistryProvider>,
}

impl TestWorkspace {
    async fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let allowed = tempfile::tempdir().unwrap();
        let project = allowed.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let roots = RegistrationRootRegistry::from_server_config(
            home.path(),
            vec![RegistrationRootSpec {
                label: "Projects".to_owned(),
                path: allowed.path().to_owned(),
            }],
        )
        .unwrap();
        let root_id = roots.list()[0].root_id.clone();
        let registry = WorkspaceRegistry::open(
            home.path(),
            roots,
            Arc::new(NoWorkspaceUsage(AtomicBool::new(false))),
            ServerRevisionCoordinator::open(home.path()),
        )
        .unwrap();
        let expected_revision = registry.revision().await.unwrap();
        let summary = registry
            .register(RegisterWorkspaceRequest {
                root_id,
                relative_path: "project".to_owned(),
                label: "Review fixture".to_owned(),
                expected_revision,
            })
            .await
            .unwrap();
        Self {
            _home: home,
            _allowed: allowed,
            project,
            workspace_id: summary.workspace_id,
            provider: Arc::new(RegistryProvider(registry)),
        }
    }

    fn service(&self) -> WorkspaceReadService {
        WorkspaceReadService::new(self.provider.clone())
    }

    fn service_with_limits(&self, limits: ReviewLimits) -> WorkspaceReadService {
        WorkspaceReadService::with_limits(self.provider.clone(), limits)
    }
}

#[tokio::test]
async fn tree_search_and_content_are_bounded_capability_reads() {
    let workspace = TestWorkspace::new().await;
    std::fs::create_dir_all(workspace.project.join("root/src/nested")).unwrap();
    std::fs::write(
        workspace.project.join("root/src/lib.rs"),
        b"pub mod review;\r\nline two\n",
    )
    .unwrap();
    std::fs::write(workspace.project.join("root/src/alpha.TXT"), b"alpha\n").unwrap();
    std::fs::write(
        workspace.project.join("root/src/nested/data.bin"),
        [0, 1, 2, 3],
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        workspace.project.join("root/src/lib.rs"),
        workspace.project.join("root/src/link"),
    )
    .unwrap();
    let service = workspace.service();

    let first = service
        .tree(&workspace.workspace_id, "root", None, 2)
        .await
        .unwrap();
    assert_eq!(2, first.entries.len());
    assert!(first.next_cursor.is_some());
    let second = service
        .tree(
            &workspace.workspace_id,
            "root",
            first.next_cursor.as_ref(),
            20,
        )
        .await
        .unwrap();
    assert_eq!(first.revision, second.revision);
    let paths = first
        .entries
        .iter()
        .chain(&second.entries)
        .map(|entry| entry.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        [
            "root/src",
            "root/src/alpha.TXT",
            "root/src/lib.rs",
            "root/src/nested",
            "root/src/nested/data.bin",
        ],
        paths.as_slice()
    );
    assert_eq!(FileKind::Directory, first.entries[0].kind);
    assert_eq!(FileKind::File, second.entries[0].kind);

    let search = service
        .search(&workspace.workspace_id, "root", "LiB", None, 100)
        .await
        .unwrap();
    assert_eq!(1, search.matches.len());
    assert_eq!("root/src/lib.rs", search.matches[0].entry.path);
    assert_eq!(
        vec![kuku_server::api::TextRange { start: 9, end: 12 }],
        search.matches[0].path_match_ranges
    );
    assert_ne!(first.revision, search.revision);
    assert!(service
        .search(&workspace.workspace_id, "root", "review", None, 100)
        .await
        .unwrap()
        .matches
        .is_empty());

    let content = service
        .content(&workspace.workspace_id, "root/src/lib.rs", 1, 10)
        .await
        .unwrap();
    assert_eq!(Some("pub mod review;\nline two".to_owned()), content.text);
    assert_eq!(Some(2), content.total_lines);
    assert!(!content.truncated);
    assert_eq!(
        "4d1960b91237f11a285b40987bdff3bb5e9e99381cb3fe8063a547db030e4bca",
        content.revision.as_str()
    );
    let ranged = service
        .content(&workspace.workspace_id, "root/src/lib.rs", 1, 1)
        .await
        .unwrap();
    assert_eq!(Some("pub mod review;".to_owned()), ranged.text);
    assert_eq!(None, ranged.total_lines);
    assert!(ranged.truncated);
    assert_eq!(Some(2), ranged.next_start_line);
    assert_eq!(content.revision, ranged.revision);

    let binary = service
        .content(&workspace.workspace_id, "root/src/nested/data.bin", 1, 10)
        .await
        .unwrap();
    assert!(binary.binary);
    assert_eq!(None, binary.text);
    assert_eq!(None, binary.total_lines);
}

#[tokio::test]
async fn empty_prefix_enumerates_the_capability_root() {
    let workspace = TestWorkspace::new().await;
    std::fs::create_dir(workspace.project.join("root")).unwrap();
    std::fs::write(workspace.project.join("root/nested.txt"), b"nested").unwrap();
    std::fs::write(workspace.project.join("top.txt"), b"top").unwrap();
    let service = workspace.service();

    let page = service
        .tree(&workspace.workspace_id, "", None, 200)
        .await
        .unwrap();
    assert_eq!(
        ["root", "root/nested.txt", "top.txt"],
        page.entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>()
            .as_slice()
    );
    let search = service
        .search(&workspace.workspace_id, "", "top", None, 100)
        .await
        .unwrap();
    assert_eq!("top.txt", search.matches[0].entry.path);
}

#[tokio::test]
async fn cursors_bind_prefix_query_and_revision() {
    let workspace = TestWorkspace::new().await;
    std::fs::create_dir(workspace.project.join("root")).unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(workspace.project.join("root").join(name), name).unwrap();
    }
    let service = workspace.service();
    let first = service
        .tree(&workspace.workspace_id, "root", None, 1)
        .await
        .unwrap();
    let cursor = first.next_cursor.as_ref().unwrap();

    assert_eq!(
        ApiErrorCode::InvalidRequest,
        service
            .search(&workspace.workspace_id, "root", "a", Some(cursor), 1)
            .await
            .unwrap_err()
            .code()
    );
    std::fs::write(workspace.project.join("root/d.txt"), b"d").unwrap();
    assert_eq!(
        ApiErrorCode::Outdated,
        service
            .tree(&workspace.workspace_id, "root", Some(cursor), 1)
            .await
            .unwrap_err()
            .code()
    );
}

#[tokio::test]
async fn exact_bytes_change_revision_despite_equal_size_and_mtime() {
    let workspace = TestWorkspace::new().await;
    std::fs::create_dir(workspace.project.join("root")).unwrap();
    let path = workspace.project.join("root/value.txt");
    std::fs::write(&path, b"same").unwrap();
    let original_modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let service = workspace.service();
    let first = service
        .current_revision(&workspace.workspace_id, "root/value.txt")
        .await
        .unwrap();
    assert_eq!(
        "a1f1878e3176e78a05f0c25e4481025e8b4c3b469a0419de59a12ccb47ed83e0",
        first.as_str()
    );

    std::fs::write(&path, b"diff").unwrap();
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(original_modified))
        .unwrap();
    let second = service
        .current_revision(&workspace.workspace_id, "root/value.txt")
        .await
        .unwrap();

    assert_ne!(first, second);
    assert_eq!(4, std::fs::metadata(path).unwrap().len());
}

#[tokio::test]
async fn utf8_character_crossing_read_buffer_remains_text() {
    let workspace = TestWorkspace::new().await;
    std::fs::create_dir(workspace.project.join("root")).unwrap();
    let mut text = "a".repeat(65_535);
    text.push('é');
    text.push('\n');
    std::fs::write(workspace.project.join("root/utf8.txt"), text.as_bytes()).unwrap();

    let content = workspace
        .service()
        .content(&workspace.workspace_id, "root/utf8.txt", 1, 1)
        .await
        .unwrap();

    assert!(!content.binary);
    assert_eq!(Some(text.trim_end().to_owned()), content.text);
}

#[tokio::test]
async fn response_limits_and_paths_fail_without_host_disclosure() {
    let workspace = TestWorkspace::new().await;
    std::fs::create_dir(workspace.project.join("root")).unwrap();
    std::fs::write(
        workspace.project.join("root/lines.txt"),
        b"one\ntwo\nthree\n",
    )
    .unwrap();
    let limits = ReviewLimits {
        file_lines: 2,
        file_bytes: 7,
        ..ReviewLimits::default()
    };
    let service = workspace.service_with_limits(limits);
    let content = service
        .content(&workspace.workspace_id, "root/lines.txt", 1, 20)
        .await
        .unwrap();
    assert_eq!(Some("one\ntwo".to_owned()), content.text);
    assert!(content.truncated);
    assert_eq!(Some(3), content.next_start_line);
    assert_eq!(None, content.total_lines);

    for path in ["../secret", "/tmp/secret", "root/../secret", "root\\secret"] {
        let error = service
            .content(&workspace.workspace_id, path, 1, 1)
            .await
            .unwrap_err();
        let encoded = serde_json::to_string(&error).unwrap();
        assert_eq!(ApiErrorCode::InvalidRequest, error.code());
        assert!(!encoded.contains(&workspace.project.display().to_string()));
        assert!(!encoded.contains(path));
    }
}

#[tokio::test]
async fn file_revision_overflow_returns_no_partial_content() {
    let workspace = TestWorkspace::new().await;
    std::fs::create_dir(workspace.project.join("root")).unwrap();
    File::create(workspace.project.join("root/too-large.bin"))
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();

    let error = workspace
        .service()
        .content(&workspace.workspace_id, "root/too-large.bin", 1, 1)
        .await
        .unwrap_err();
    assert_eq!(ApiErrorCode::PayloadTooLarge, error.code());
    assert!(!serde_json::to_string(&error)
        .unwrap()
        .contains(&workspace.project.display().to_string()));
}

#[tokio::test]
async fn listing_entry_overflow_returns_no_partial_page() {
    let workspace = TestWorkspace::new().await;
    let root = workspace.project.join("root");
    std::fs::create_dir(&root).unwrap();
    for index in 0..=20_000 {
        File::create(root.join(format!("entry-{index:05}"))).unwrap();
    }

    let error = workspace
        .service()
        .tree(&workspace.workspace_id, "root", None, 200)
        .await
        .unwrap_err();
    assert_eq!(ApiErrorCode::PayloadTooLarge, error.code());
}

#[tokio::test]
async fn listing_hash_byte_overflow_returns_no_partial_page() {
    let workspace = TestWorkspace::new().await;
    std::fs::create_dir(workspace.project.join("root")).unwrap();
    File::create(workspace.project.join("root/too-large.bin"))
        .unwrap()
        .set_len(256 * 1024 * 1024 + 1)
        .unwrap();

    let error = workspace
        .service()
        .tree(&workspace.workspace_id, "root", None, 200)
        .await
        .unwrap_err();
    assert_eq!(ApiErrorCode::PayloadTooLarge, error.code());
}
