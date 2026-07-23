use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::api::{ApiError, RegisterWorkspaceRequest, WorkspaceId};

use super::{
    RegistrationRootRegistry, RegistrationRootSpec, ServerRevisionCoordinator, WorkspaceRegistry,
    WorkspaceUsagePort,
};

struct UnusedUsage;

impl WorkspaceUsagePort for UnusedUsage {
    fn has_durable_tasks<'a>(
        &'a self,
        _id: &'a WorkspaceId,
    ) -> Pin<Box<dyn Future<Output = Result<bool, ApiError>> + Send + 'a>> {
        Box::pin(async { Ok(false) })
    }
}

#[tokio::test]
async fn registration_blocks_snapshots_until_revision_is_published() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    std::fs::create_dir(allowed.path().join("project")).unwrap();
    let revision = ServerRevisionCoordinator::open(home.path());
    let roots = RegistrationRootRegistry::from_server_config(
        home.path(),
        vec![RegistrationRootSpec {
            label: "Projects".to_owned(),
            path: allowed.path().to_owned(),
        }],
    )
    .unwrap();
    let registry = WorkspaceRegistry::open(
        home.path(),
        roots,
        Arc::new(UnusedUsage),
        Arc::clone(&revision),
    )
    .unwrap();
    let expected_revision = registry.revision().await.unwrap();
    let root_id = registry.registration_roots().list()[0].root_id.clone();
    let reached = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let registering = {
        let registry = Arc::clone(&registry);
        let reached = Arc::clone(&reached);
        let release = Arc::clone(&release);
        tokio::spawn(async move {
            registry
                .register_with_before_finish(
                    RegisterWorkspaceRequest {
                        root_id,
                        relative_path: "project".to_owned(),
                        label: "Project".to_owned(),
                        expected_revision,
                    },
                    || async move {
                        reached.notify_one();
                        release.notified().await;
                    },
                )
                .await
        })
    };
    reached.notified().await;
    let listed = Arc::new(tokio::sync::Notify::new());
    let listing = {
        let registry = Arc::clone(&registry);
        let listed = Arc::clone(&listed);
        tokio::spawn(async move {
            registry
                .list_with_after_gate(|| async move { listed.notify_one() })
                .await
        })
    };
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), listed.notified())
            .await
            .is_err()
    );
    release.notify_one();
    registering.await.unwrap().unwrap();
    let page = listing.await.unwrap().unwrap();
    assert_eq!(1, page.items.len());
    assert_eq!(revision.current().await.unwrap(), page.server_revision);
}

#[cfg(unix)]
#[tokio::test]
async fn task_query_rejects_a_replaced_registered_workspace_root() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    let project = allowed.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let revision = ServerRevisionCoordinator::open(home.path());
    let roots = RegistrationRootRegistry::from_server_config(
        home.path(),
        vec![RegistrationRootSpec {
            label: "Projects".to_owned(),
            path: allowed.path().to_owned(),
        }],
    )
    .unwrap();
    let registry = WorkspaceRegistry::open(
        home.path(),
        roots,
        Arc::new(UnusedUsage),
        Arc::clone(&revision),
    )
    .unwrap();
    let root_id = registry.registration_roots().list()[0].root_id.clone();
    let registered = registry
        .register(RegisterWorkspaceRequest {
            root_id,
            relative_path: "project".to_owned(),
            label: "Project".to_owned(),
            expected_revision: registry.revision().await.unwrap(),
        })
        .await
        .unwrap();
    let capability = registry.capability(&registered.workspace_id).unwrap();
    std::fs::rename(&project, allowed.path().join("original-project")).unwrap();
    std::fs::create_dir(&project).unwrap();
    let scope = kuku::ExecutionScope {
        workspace_id: registered.workspace_id,
        task_id: kuku::TaskId::try_new().unwrap(),
        run_id: kuku::RunId::try_new().unwrap(),
        turn_id: kuku::TurnId::try_new().unwrap(),
        conversation_id: kuku::ConversationId::try_new().unwrap(),
        turn_index: 1,
    };

    let error = capability
        .query(
            "hello",
            scope,
            kuku::event::EventStore::open(home.path().join("events.jsonl")).unwrap(),
            Vec::new(),
        )
        .unwrap_err();

    assert_eq!(error.code(), crate::api::ApiErrorCode::WorkspaceUnavailable);
}

#[tokio::test]
async fn registration_allows_the_active_root_and_restores_it_after_restart() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    let revision = ServerRevisionCoordinator::open(home.path());
    let roots = RegistrationRootRegistry::from_server_config(
        home.path(),
        vec![RegistrationRootSpec {
            label: "Projects".to_owned(),
            path: allowed.path().to_owned(),
        }],
    )
    .unwrap();
    let registry = WorkspaceRegistry::open(
        home.path(),
        roots,
        Arc::new(UnusedUsage),
        Arc::clone(&revision),
    )
    .unwrap();
    let root_id = registry.registration_roots().list()[0].root_id.clone();
    let registered = registry
        .register(RegisterWorkspaceRequest {
            root_id,
            relative_path: ".".to_owned(),
            label: "Workspace".to_owned(),
            expected_revision: registry.revision().await.unwrap(),
        })
        .await
        .unwrap();

    assert!(registry.capability(&registered.workspace_id).is_ok());
    assert!(registry
        .capability(&registered.workspace_id)
        .unwrap()
        .resolve(".")
        .is_err());

    let restored_roots = RegistrationRootRegistry::from_server_config(
        home.path(),
        vec![RegistrationRootSpec {
            label: "Projects".to_owned(),
            path: allowed.path().to_owned(),
        }],
    )
    .unwrap();
    let restored =
        WorkspaceRegistry::open(home.path(), restored_roots, Arc::new(UnusedUsage), revision)
            .unwrap();

    assert!(restored.capability(&registered.workspace_id).is_ok());
}
