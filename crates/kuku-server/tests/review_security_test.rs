#![allow(dead_code, unused_imports)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use kuku_server::api::{ApiError, ApiErrorCode, RegisterWorkspaceRequest, WorkspaceId};
use kuku_server::platform::{
    RegistrationRootRegistry, RegistrationRootSpec, ServerRevisionCoordinator, WorkspaceRegistry,
    WorkspaceUsagePort,
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

#[path = "common/review_modules.rs"]
mod review;
#[path = "../src/review/mod.rs"]
mod review_contract;

use review::{ReviewAdmission, ReviewLimits, WorkspaceCapabilityProvider};

struct UnusedUsage;

impl WorkspaceUsagePort for UnusedUsage {
    fn has_durable_tasks<'a>(
        &'a self,
        _id: &'a WorkspaceId,
    ) -> Pin<Box<dyn Future<Output = Result<bool, ApiError>> + Send + 'a>> {
        Box::pin(async { Ok(false) })
    }
}

struct Provider(Arc<WorkspaceRegistry>);

impl WorkspaceCapabilityProvider for Provider {
    fn capability(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<kuku_server::platform::WorkspaceCapability, ApiError> {
        self.0.capability(workspace_id)
    }
}

#[tokio::test]
async fn capability_paths_reject_escape_forms_without_host_disclosure() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    let project = allowed.path().join("project");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(project.join("safe.txt"), "safe\n").unwrap();
    let roots = RegistrationRootRegistry::from_server_config(
        home.path(),
        vec![RegistrationRootSpec {
            label: "Security".to_owned(),
            path: allowed.path().to_owned(),
        }],
    )
    .unwrap();
    let registry = WorkspaceRegistry::open(
        home.path(),
        roots,
        Arc::new(UnusedUsage),
        ServerRevisionCoordinator::open(home.path()),
    )
    .unwrap();
    let root_id = registry.registration_roots().list()[0].root_id.clone();
    let workspace = registry
        .register(RegisterWorkspaceRequest {
            root_id,
            relative_path: "project".to_owned(),
            label: "Project".to_owned(),
            expected_revision: registry.revision().await.unwrap(),
        })
        .await
        .unwrap();
    let provider = Arc::new(Provider(registry));
    let service = review::files::WorkspaceReadService::new(provider);
    for path in [
        "/etc/passwd",
        "../safe.txt",
        "..\\safe.txt",
        "C:\\Windows\\system32",
        "\\\\server\\share\\secret",
        "safe.txt\0suffix",
    ] {
        let error = service
            .current_revision(&workspace.workspace_id, path)
            .await
            .unwrap_err();
        assert_eq!(ApiErrorCode::InvalidRequest, error.code(), "{path:?}");
        assert!(!error.message.contains("/etc"));
    }
}

#[test]
fn review_admission_releases_global_and_workspace_capacity() {
    let limits = ReviewLimits {
        global_scan_permits: 2,
        workspace_scan_permits: 1,
        global_git_permits: 2,
        workspace_git_permits: 1,
        ..ReviewLimits::default()
    };
    let admission = ReviewAdmission::new(&limits);
    let workspace = WorkspaceId::parse("wsp_000000000000000000000001").unwrap();
    let first = admission.try_acquire_scan(&workspace).unwrap();
    assert_eq!(
        ApiErrorCode::ServerBusy,
        admission.try_acquire_scan(&workspace).unwrap_err().code()
    );
    drop(first);
    let _reused = admission.try_acquire_scan(&workspace).unwrap();

    let git = admission.try_acquire_git(&workspace).unwrap();
    assert_eq!(
        ApiErrorCode::ServerBusy,
        admission.try_acquire_git(&workspace).unwrap_err().code()
    );
    drop(git);
    let _reused_git = admission.try_acquire_git(&workspace).unwrap();
}
