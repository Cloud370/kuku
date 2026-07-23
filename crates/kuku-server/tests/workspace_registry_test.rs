use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use kuku::{WorkspaceCommandRequest, WorkspaceQueryCapability};
use kuku_server::api::{
    ApiError, ApiErrorCode, RegisterWorkspaceRequest, RemoveWorkspaceRequest, WorkspaceAvailability,
};
#[cfg(unix)]
use kuku_server::platform::{ProcessChunk, ProcessChunkSink, ProcessLimits, RootCommand};
use kuku_server::platform::{
    RegistrationRootRegistry, RegistrationRootSpec, ServerRevisionCoordinator, WorkspaceRegistry,
    WorkspaceUsagePort,
};

struct UsageFixture {
    in_use: AtomicBool,
}

#[derive(Default)]
#[cfg(unix)]
struct ChunkFixture {
    chunks: Vec<ProcessChunk>,
}

#[cfg(unix)]
struct RejectingChunkFixture;

#[cfg(unix)]
impl ProcessChunkSink for ChunkFixture {
    fn push<'a>(
        &'a mut self,
        chunk: ProcessChunk,
    ) -> Pin<Box<dyn Future<Output = Result<(), ApiError>> + Send + 'a>> {
        Box::pin(async move {
            self.chunks.push(chunk);
            Ok(())
        })
    }
}

#[cfg(unix)]
impl ProcessChunkSink for RejectingChunkFixture {
    fn push<'a>(
        &'a mut self,
        _chunk: ProcessChunk,
    ) -> Pin<Box<dyn Future<Output = Result<(), ApiError>> + Send + 'a>> {
        Box::pin(async {
            Err(ApiError::new(
                ApiErrorCode::Internal,
                "stream rejected",
                "workspace-test",
            ))
        })
    }
}

impl WorkspaceUsagePort for UsageFixture {
    fn has_durable_tasks<'a>(
        &'a self,
        _id: &'a kuku_server::api::WorkspaceId,
    ) -> Pin<Box<dyn Future<Output = Result<bool, ApiError>> + Send + 'a>> {
        Box::pin(async move { Ok(self.in_use.load(Ordering::SeqCst)) })
    }
}

fn open_registry(
    home: &std::path::Path,
    allowed: &std::path::Path,
    usage: Arc<UsageFixture>,
) -> Arc<WorkspaceRegistry> {
    let roots = RegistrationRootRegistry::from_server_config(
        home,
        vec![RegistrationRootSpec {
            label: "Projects".to_owned(),
            path: allowed.to_owned(),
        }],
    )
    .unwrap();
    let revision = ServerRevisionCoordinator::open(home);
    WorkspaceRegistry::open(home, roots, usage, revision).unwrap()
}

async fn register(
    registry: &Arc<WorkspaceRegistry>,
    relative_path: &str,
    label: &str,
) -> kuku_server::api::WorkspaceSummary {
    let root_id = registry.registration_roots().list()[0].root_id.clone();
    let expected_revision = registry.revision().await.unwrap();
    registry
        .register(RegisterWorkspaceRequest {
            root_id,
            relative_path: relative_path.to_owned(),
            label: label.to_owned(),
            expected_revision,
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn opaque_registry_persists_ids_without_exposing_host_roots() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    std::fs::create_dir(allowed.path().join("project")).unwrap();
    std::fs::write(
        allowed.path().join("project/readme.txt"),
        "private workspace",
    )
    .unwrap();
    let usage = Arc::new(UsageFixture {
        in_use: AtomicBool::new(false),
    });
    let registry = open_registry(home.path(), allowed.path(), usage.clone());
    let root_id = registry.registration_roots().list()[0].root_id.clone();

    let workspace = register(&registry, "project", "kuku").await;

    assert_eq!(28, workspace.workspace_id.as_str().len());
    assert!(workspace.workspace_id.as_str().starts_with("wsp_"));
    assert!(workspace.is_default);
    assert_eq!(WorkspaceAvailability::Available, workspace.availability);
    let serialized_roots = serde_json::to_string(&registry.registration_roots().list()).unwrap();
    let serialized_page = serde_json::to_string(&registry.list().await.unwrap()).unwrap();
    assert!(!serialized_roots.contains(&allowed.path().display().to_string()));
    assert!(!serialized_page.contains(&allowed.path().display().to_string()));
    assert!(
        !std::fs::read_to_string(home.path().join("workspaces.json"))
            .unwrap()
            .contains(&allowed.path().display().to_string())
    );

    #[cfg(unix)]
    for private_file in ["registration-roots.json", "workspaces.json"] {
        use std::os::unix::fs::PermissionsExt;

        let mode = std::fs::metadata(home.path().join(private_file))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(0o600, mode);
    }

    let capability = registry.capability(&workspace.workspace_id).unwrap();
    let relative = capability.resolve("readme.txt").unwrap();
    let mut file = capability.open_file(&relative).unwrap();
    let mut contents = String::new();
    std::io::Read::read_to_string(&mut file, &mut contents).unwrap();
    assert_eq!("private workspace", contents);

    let before_restart = registry.revision().await.unwrap();
    drop(registry);
    let reopened = open_registry(home.path(), allowed.path(), usage);
    assert_eq!(root_id, reopened.registration_roots().list()[0].root_id);
    assert_eq!(
        workspace.workspace_id,
        reopened.list().await.unwrap().items[0].workspace_id
    );
    assert_eq!(before_restart, reopened.revision().await.unwrap());
}

#[tokio::test]
async fn query_capability_never_uses_a_replacement_workspace_root() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    let root = allowed.path().join("project");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("identity.txt"), "original").unwrap();
    let registry = open_registry(
        home.path(),
        allowed.path(),
        Arc::new(UsageFixture {
            in_use: AtomicBool::new(false),
        }),
    );
    let workspace = register(&registry, "project", "kuku").await;
    let capability = registry.capability(&workspace.workspace_id).unwrap();

    let displaced = allowed.path().join("displaced");
    std::fs::rename(&root, &displaced).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("identity.txt"), "replacement").unwrap();

    assert_eq!(
        capability.read_file("identity.txt", 1024).unwrap(),
        b"original"
    );
    capability
        .write_file("created.txt", b"capability", 1024)
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(displaced.join("created.txt")).unwrap(),
        "capability"
    );
    assert!(!root.join("created.txt").exists());
    capability
        .write_file("identity.txt", b"updated", 1024)
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(displaced.join("identity.txt")).unwrap(),
        "updated"
    );
    assert!(capability
        .write_file("identity.txt", b"too large", 1)
        .is_err());
    assert_eq!(
        std::fs::read_to_string(displaced.join("identity.txt")).unwrap(),
        "updated"
    );
    assert!(!std::fs::read_dir(&displaced)
        .unwrap()
        .flatten()
        .any(|entry| entry
            .file_name()
            .to_string_lossy()
            .starts_with(".kuku-write-")));

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(displaced.join("identity.txt"), displaced.join("leaf-link"))
            .unwrap();
        std::os::unix::fs::symlink(&root, displaced.join("parent-link")).unwrap();
        assert!(capability
            .write_file("leaf-link", b"outside", 1024)
            .is_err());
        assert!(capability
            .write_file("parent-link/outside.txt", b"outside", 1024)
            .is_err());
        assert!(!root.join("outside.txt").exists());
    }
    let entries = capability.list_entries(".", 100).unwrap();
    assert!(entries.iter().any(|entry| entry.path == "identity.txt"));
    assert!(!entries.iter().any(|entry| entry.path == "replacement"));

    let command = capability
        .run_command(
            WorkspaceCommandRequest {
                command: "printf replacement > command-marker.txt".to_string(),
                timeout: std::time::Duration::from_secs(5),
                max_output_bytes: 4096,
            },
            None,
            kuku::WorkspaceCommandCancellation::default(),
        )
        .await;
    if command.is_ok() {
        assert!(displaced.join("command-marker.txt").exists());
    }
    assert!(!root.join("command-marker.txt").exists());

    #[cfg(not(windows))]
    {
        let (events, receiver) = tokio::sync::mpsc::channel(1);
        drop(receiver);
        let started = std::time::Instant::now();
        let disconnected = capability
            .run_command(
                WorkspaceCommandRequest {
                    command: "sleep 60 & child=$!; printf '%s %s' $$ $child > disconnect-pids; printf output; wait".to_string(),
                    timeout: std::time::Duration::from_secs(60),
                    max_output_bytes: 4096,
                },
                Some(events),
                kuku::WorkspaceCommandCancellation::default(),
            )
            .await;
        assert!(disconnected.is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        let pids = std::fs::read_to_string(displaced.join("disconnect-pids")).unwrap();
        let pids = pids
            .split_whitespace()
            .map(|pid| pid.parse::<i32>().unwrap())
            .collect::<Vec<_>>();
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        for _ in 0..100 {
            if pids.iter().all(|pid| unsafe { kill(*pid, 0) } == -1) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(pids.iter().all(|pid| unsafe { kill(*pid, 0) } == -1));
    }
}

#[tokio::test]
async fn capability_opens_and_enumerates_the_registered_workspace_root() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    let root = allowed.path().join("project");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("root-file.txt"), "root").unwrap();
    let registry = open_registry(
        home.path(),
        allowed.path(),
        Arc::new(UsageFixture {
            in_use: AtomicBool::new(false),
        }),
    );
    let workspace = register(&registry, "project", "kuku").await;
    let capability = registry.capability(&workspace.workspace_id).unwrap();

    let opened = capability.open_root().unwrap();
    let mut file = opened.open("root-file.txt").unwrap();
    let mut contents = String::new();
    std::io::Read::read_to_string(&mut file, &mut contents).unwrap();
    assert_eq!("root", contents);
    let names = capability
        .read_root()
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(vec![std::ffi::OsString::from("root-file.txt")], names);
}

#[test]
fn process_limits_allow_bounded_review_streams_up_to_one_gibibyte() {
    assert!(ProcessLimits::new(std::time::Duration::from_secs(5), 1024 * 1024 * 1024).is_ok());
    assert!(
        ProcessLimits::new(std::time::Duration::from_secs(5), 1024 * 1024 * 1024 + 1,).is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn capability_reads_symlink_targets_without_following_them() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    let root = allowed.path().join("project");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("target.txt"), "target").unwrap();
    std::os::unix::fs::symlink("target.txt", root.join("link.txt")).unwrap();
    let registry = open_registry(
        home.path(),
        allowed.path(),
        Arc::new(UsageFixture {
            in_use: AtomicBool::new(false),
        }),
    );
    let workspace = register(&registry, "project", "kuku").await;
    let capability = registry.capability(&workspace.workspace_id).unwrap();
    let relative = capability.resolve("link.txt").unwrap();

    assert_eq!(
        b"target.txt",
        capability.read_link_target(&relative).unwrap().as_slice()
    );
    assert!(capability.open_file(&relative).is_err());
}

#[tokio::test]
async fn query_capability_prunes_large_generated_directories_before_budgeting_entries() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    let root = allowed.path().join("project");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("visible.txt"), "visible").unwrap();
    let excluded_directories = [
        ".git",
        "target",
        "node_modules",
        "__pycache__",
        ".venv",
        "venv",
        "dist",
        "build",
    ];
    for excluded in excluded_directories {
        let directory = root.join(excluded);
        std::fs::create_dir(&directory).unwrap();
        for index in 0..64 {
            std::fs::write(directory.join(format!("generated-{index}.txt")), "hidden").unwrap();
        }
    }
    let registry = open_registry(
        home.path(),
        allowed.path(),
        Arc::new(UsageFixture {
            in_use: AtomicBool::new(false),
        }),
    );
    let workspace = register(&registry, "project", "kuku").await;
    let capability = registry.capability(&workspace.workspace_id).unwrap();

    let entries = capability.list_entries(".", 4).unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "visible.txt");
    assert!(!entries.iter().any(
        |entry| excluded_directories
            .iter()
            .any(|excluded| entry.path == *excluded
                || entry.path.starts_with(&format!("{excluded}/")))
    ));
}

#[tokio::test]
async fn query_capability_uses_the_sdk_portable_relative_path_rules() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    std::fs::create_dir(allowed.path().join("project")).unwrap();
    let registry = open_registry(
        home.path(),
        allowed.path(),
        Arc::new(UsageFixture {
            in_use: AtomicBool::new(false),
        }),
    );
    let workspace = register(&registry, "project", "kuku").await;
    let capability = registry.capability(&workspace.workspace_id).unwrap();

    for invalid in [
        "file:stream",
        "CON",
        "con.txt",
        "nested/COM9.log",
        "nested/Lpt1",
        "file.",
        "nested /file",
    ] {
        assert!(
            kuku::event::WorkspaceRelativePath::parse(invalid).is_err(),
            "SDK accepted {invalid}"
        );
        assert!(
            capability.resolve(invalid).is_err(),
            "server accepted {invalid}"
        );
    }
}

#[tokio::test]
async fn registry_rejects_lexical_escape_duplicates_and_symlinks() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(allowed.path().join("project")).unwrap();
    let usage = Arc::new(UsageFixture {
        in_use: AtomicBool::new(false),
    });
    let registry = open_registry(home.path(), allowed.path(), usage);
    let workspace = register(&registry, "project", "kuku").await;
    let capability = registry.capability(&workspace.workspace_id).unwrap();

    assert!(capability.resolve(&"a".repeat(4096)).is_ok());
    assert_eq!(
        ApiErrorCode::InvalidRequest,
        capability.resolve(&"a".repeat(4097)).unwrap_err().code()
    );

    for invalid in [
        "",
        ".",
        "../secret",
        "/etc/passwd",
        "C:\\Windows",
        "C:/Windows",
        "\\\\server\\share",
        "nested//file",
        "nested/./file",
    ] {
        assert_eq!(
            ApiErrorCode::InvalidRequest,
            capability.resolve(invalid).unwrap_err().code(),
            "path should be rejected: {invalid}"
        );
    }

    let duplicate = RegisterWorkspaceRequest {
        root_id: registry.registration_roots().list()[0].root_id.clone(),
        relative_path: "project".to_owned(),
        label: "duplicate".to_owned(),
        expected_revision: registry.revision().await.unwrap(),
    };
    assert_eq!(
        ApiErrorCode::InvalidRequest,
        registry.register(duplicate).await.unwrap_err().code()
    );

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path(), allowed.path().join("linked")).unwrap();
        let request = RegisterWorkspaceRequest {
            root_id: registry.registration_roots().list()[0].root_id.clone(),
            relative_path: "linked".to_owned(),
            label: "linked".to_owned(),
            expected_revision: registry.revision().await.unwrap(),
        };
        assert_eq!(
            ApiErrorCode::InvalidRequest,
            registry.register(request).await.unwrap_err().code()
        );
    }
}

#[tokio::test]
async fn revision_and_usage_gate_protect_workspace_mutations() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    std::fs::create_dir(allowed.path().join("one")).unwrap();
    std::fs::create_dir(allowed.path().join("two")).unwrap();
    let usage = Arc::new(UsageFixture {
        in_use: AtomicBool::new(false),
    });
    let registry = open_registry(home.path(), allowed.path(), usage.clone());
    let stale = registry.revision().await.unwrap();
    let first = register(&registry, "one", "one").await;
    let second = register(&registry, "two", "two").await;

    assert_eq!(
        ApiErrorCode::StaleServerRevision,
        registry
            .set_default(&second.workspace_id, stale)
            .await
            .unwrap_err()
            .code()
    );

    let page = registry
        .set_default(&second.workspace_id, registry.revision().await.unwrap())
        .await
        .unwrap();
    assert!(
        page.items
            .iter()
            .find(|item| item.workspace_id == second.workspace_id)
            .unwrap()
            .is_default
    );

    usage.in_use.store(true, Ordering::SeqCst);
    assert_eq!(
        ApiErrorCode::WorkspaceInUse,
        registry
            .remove(
                &first.workspace_id,
                RemoveWorkspaceRequest {
                    expected_revision: registry.revision().await.unwrap(),
                },
            )
            .await
            .unwrap_err()
            .code()
    );
    usage.in_use.store(false, Ordering::SeqCst);
    registry
        .remove(
            &first.workspace_id,
            RemoveWorkspaceRequest {
                expected_revision: registry.revision().await.unwrap(),
            },
        )
        .await
        .unwrap();
    assert!(allowed.path().join("one").is_dir());
}

#[tokio::test]
async fn stale_revision_wins_before_registration_validation() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    std::fs::create_dir(allowed.path().join("one")).unwrap();
    let usage = Arc::new(UsageFixture {
        in_use: AtomicBool::new(false),
    });
    let registry = open_registry(home.path(), allowed.path(), usage);
    let stale = registry.revision().await.unwrap();
    register(&registry, "one", "one").await;

    let error = registry
        .register(RegisterWorkspaceRequest {
            root_id: registry.registration_roots().list()[0].root_id.clone(),
            relative_path: "../escape".to_owned(),
            label: String::new(),
            expected_revision: stale,
        })
        .await
        .unwrap_err();
    assert_eq!(ApiErrorCode::StaleServerRevision, error.code());
}

#[tokio::test]
async fn overlapping_roots_cannot_register_the_same_directory_twice() {
    let home = tempfile::tempdir().unwrap();
    let outer = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(outer.path().join("inner/project")).unwrap();
    let roots = RegistrationRootRegistry::from_server_config(
        home.path(),
        vec![
            RegistrationRootSpec {
                label: "Outer".to_owned(),
                path: outer.path().to_owned(),
            },
            RegistrationRootSpec {
                label: "Inner".to_owned(),
                path: outer.path().join("inner"),
            },
        ],
    )
    .unwrap();
    let root_page = roots.list();
    let registry = WorkspaceRegistry::open(
        home.path(),
        roots,
        Arc::new(UsageFixture {
            in_use: AtomicBool::new(false),
        }),
        ServerRevisionCoordinator::open(home.path()),
    )
    .unwrap();
    registry
        .register(RegisterWorkspaceRequest {
            root_id: root_page[0].root_id.clone(),
            relative_path: "inner/project".to_owned(),
            label: "first".to_owned(),
            expected_revision: registry.revision().await.unwrap(),
        })
        .await
        .unwrap();

    let error = registry
        .register(RegisterWorkspaceRequest {
            root_id: root_page[1].root_id.clone(),
            relative_path: "project".to_owned(),
            label: "duplicate".to_owned(),
            expected_revision: registry.revision().await.unwrap(),
        })
        .await
        .unwrap_err();
    assert_eq!(ApiErrorCode::InvalidRequest, error.code());
}

#[cfg(unix)]
#[tokio::test]
async fn process_boundary_is_identity_bound_and_projects_git_branch() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    let project = allowed.path().join("project");
    std::fs::create_dir(&project).unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(&project)
        .status()
        .unwrap()
        .success());
    let usage = Arc::new(UsageFixture {
        in_use: AtomicBool::new(false),
    });
    let registry = open_registry(home.path(), allowed.path(), usage);
    let workspace = register(&registry, "project", "kuku").await;
    let capability = registry.capability(&workspace.workspace_id).unwrap();
    let limits = ProcessLimits::new(std::time::Duration::from_secs(2), 4 * 1024).unwrap();
    let root = capability
        .run_at_root(
            RootCommand::new("git").args(["rev-parse", "--show-toplevel"]),
            limits.clone(),
        )
        .await
        .unwrap();
    assert!(root.status().success());
    assert!(capability.reported_root_is_self(&root));

    let mut sink = ChunkFixture::default();
    let status = capability
        .stream_at_root(
            RootCommand::new("git").args(["symbolic-ref", "--short", "HEAD"]),
            limits,
            &mut sink,
        )
        .await
        .unwrap();
    assert!(status.success());
    assert!(!sink.chunks.is_empty());
    assert_eq!(
        Some("main"),
        registry.list().await.unwrap().items[0].branch.as_deref()
    );

    assert!(std::process::Command::new("git")
        .args(["checkout", "-b", "feature"])
        .current_dir(&project)
        .status()
        .unwrap()
        .success());
    assert_eq!(
        Some("feature"),
        registry.list().await.unwrap().items[0].branch.as_deref()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn process_timeout_and_stream_cancellation_reap_descendants() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    std::fs::create_dir(allowed.path().join("project")).unwrap();
    let usage = Arc::new(UsageFixture {
        in_use: AtomicBool::new(false),
    });
    let registry = open_registry(home.path(), allowed.path(), usage);
    let workspace = register(&registry, "project", "kuku").await;
    let capability = registry.capability(&workspace.workspace_id).unwrap();
    let command = || RootCommand::new("sh").args(["-c", "sleep 30 & printf ready; wait"]);

    let started = std::time::Instant::now();
    let output = capability
        .run_at_root(
            RootCommand::new("sh").args(["-c", "sleep 30 & printf ready"]),
            ProcessLimits::new(std::time::Duration::from_secs(1), 4096).unwrap(),
        )
        .await
        .unwrap();
    assert!(output.status().success());
    assert_eq!(b"ready", output.stdout());
    assert!(started.elapsed() < std::time::Duration::from_secs(2));

    let started = std::time::Instant::now();
    let output = capability
        .run_at_root(
            command(),
            ProcessLimits::new(std::time::Duration::from_millis(100), 4096).unwrap(),
        )
        .await
        .unwrap();
    assert!(output.status().timed_out());
    assert!(started.elapsed() < std::time::Duration::from_secs(2));

    let mut sink = RejectingChunkFixture;
    let started = std::time::Instant::now();
    let error = capability
        .stream_at_root(
            command(),
            ProcessLimits::new(std::time::Duration::from_secs(10), 4096).unwrap(),
            &mut sink,
        )
        .await
        .unwrap_err();
    assert_eq!(ApiErrorCode::Internal, error.code());
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

#[tokio::test]
async fn task_lease_blocks_removal_until_task_creation_finishes() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    std::fs::create_dir(allowed.path().join("project")).unwrap();
    let usage = Arc::new(UsageFixture {
        in_use: AtomicBool::new(false),
    });
    let registry = open_registry(home.path(), allowed.path(), usage);
    let workspace = register(&registry, "project", "kuku").await;
    let lease = registry
        .lease_for_task(&workspace.workspace_id)
        .await
        .unwrap();
    let expected_revision = registry.revision().await.unwrap();
    let removing = {
        let registry = registry.clone();
        let workspace_id = workspace.workspace_id.clone();
        tokio::spawn(async move {
            registry
                .remove(&workspace_id, RemoveWorkspaceRequest { expected_revision })
                .await
        })
    };

    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    assert!(!removing.is_finished());
    drop(lease);
    removing.await.unwrap().unwrap();
}

#[tokio::test]
async fn failed_persistence_does_not_change_in_memory_registry() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    std::fs::create_dir(allowed.path().join("one")).unwrap();
    std::fs::create_dir(allowed.path().join("two")).unwrap();
    let usage = Arc::new(UsageFixture {
        in_use: AtomicBool::new(false),
    });
    let registry = open_registry(home.path(), allowed.path(), usage);
    let first = register(&registry, "one", "one").await;
    std::fs::remove_file(home.path().join("workspaces.json")).unwrap();
    std::fs::create_dir(home.path().join("workspaces.json")).unwrap();
    let request = RegisterWorkspaceRequest {
        root_id: registry.registration_roots().list()[0].root_id.clone(),
        relative_path: "two".to_owned(),
        label: "two".to_owned(),
        expected_revision: registry.revision().await.unwrap(),
    };

    assert_eq!(
        ApiErrorCode::Internal,
        registry.register(request).await.unwrap_err().code()
    );
    let page = registry.list().await.unwrap();
    assert_eq!(1, page.items.len());
    assert_eq!(first.workspace_id, page.items[0].workspace_id);
}

#[tokio::test]
async fn corrupted_persisted_relative_path_is_rejected_on_reopen() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    std::fs::create_dir(allowed.path().join("project")).unwrap();
    let usage = Arc::new(UsageFixture {
        in_use: AtomicBool::new(false),
    });
    let registry = open_registry(home.path(), allowed.path(), usage.clone());
    register(&registry, "project", "kuku").await;
    drop(registry);
    let path = home.path().join("workspaces.json");
    let bytes = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\"project\"", "\"../escape\"");
    std::fs::write(path, bytes).unwrap();
    let roots = RegistrationRootRegistry::from_server_config(
        home.path(),
        vec![RegistrationRootSpec {
            label: "Projects".to_owned(),
            path: allowed.path().to_owned(),
        }],
    )
    .unwrap();

    assert!(WorkspaceRegistry::open(
        home.path(),
        roots,
        usage,
        ServerRevisionCoordinator::open(home.path()),
    )
    .is_err());
}

#[tokio::test]
async fn removed_registration_root_makes_persisted_workspace_unavailable() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    std::fs::create_dir(allowed.path().join("project")).unwrap();
    let usage = Arc::new(UsageFixture {
        in_use: AtomicBool::new(false),
    });
    let registry = open_registry(home.path(), allowed.path(), usage.clone());
    let workspace = register(&registry, "project", "kuku").await;
    drop(registry);

    let roots = RegistrationRootRegistry::from_server_config(home.path(), Vec::new()).unwrap();
    let reopened = WorkspaceRegistry::open(
        home.path(),
        roots,
        usage,
        ServerRevisionCoordinator::open(home.path()),
    )
    .unwrap();

    assert_eq!(
        WorkspaceAvailability::Inaccessible,
        reopened.list().await.unwrap().items[0].availability
    );
    assert_eq!(
        ApiErrorCode::WorkspaceUnavailable,
        reopened
            .capability(&workspace.workspace_id)
            .unwrap_err()
            .code()
    );
}
