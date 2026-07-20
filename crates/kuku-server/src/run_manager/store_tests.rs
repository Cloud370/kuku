use tempfile::tempdir;

use kuku::event::WorkspaceId;

use super::repository::TaskRepository;
use super::store::{CreateTaskCommand, TaskCommandService};

fn workspace_id() -> WorkspaceId {
    WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap()
}

#[tokio::test]
async fn create_replays_after_reopen_and_duplicate_key_returns_same_task() {
    let dir = tempdir().unwrap();
    let repository = TaskRepository::open(dir.path()).unwrap();
    let service = TaskCommandService::new(repository.clone());
    let command = CreateTaskCommand {
        workspace_id: workspace_id(),
        idempotency_key: "create-1".into(),
        title: "First task".into(),
    };
    let first = service.create_task(command.clone()).await.unwrap();
    let replay = service.create_task(command).await.unwrap();
    assert_eq!(first.task_id(), replay.task_id());
    let reopened = TaskCommandService::new(TaskRepository::open(dir.path()).unwrap());
    let projection = reopened.projection(first.task_id().unwrap()).await.unwrap();
    assert_eq!(projection.task.title, "First task");
}

#[tokio::test]
async fn concurrent_identical_creates_commit_one_ledger() {
    let dir = tempdir().unwrap();
    let service = std::sync::Arc::new(TaskCommandService::new(
        TaskRepository::open(dir.path()).unwrap(),
    ));
    let command = CreateTaskCommand {
        workspace_id: workspace_id(),
        idempotency_key: "race".into(),
        title: "Race".into(),
    };
    let (left, right) = tokio::join!(
        service.create_task(command.clone()),
        service.create_task(command),
    );
    assert_eq!(left.unwrap().task_id(), right.unwrap().task_id());
    let entries = std::fs::read_dir(dir.path().join("tasks")).unwrap().count();
    assert_eq!(entries, 1);
}
