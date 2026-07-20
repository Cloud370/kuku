use tempfile::tempdir;

use kuku::event::WorkspaceId;

use super::repository::TaskRepository;
use super::store::{StopRunCommand, SubmitRunCommand};
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

#[tokio::test]
async fn submit_is_revision_serialized_and_duplicate_replays() {
    let dir = tempdir().unwrap();
    let service = TaskCommandService::new(TaskRepository::open(dir.path()).unwrap());
    let task = service
        .create_task(CreateTaskCommand {
            workspace_id: workspace_id(),
            idempotency_key: "create".into(),
            title: "Task".into(),
        })
        .await
        .unwrap();
    let command = SubmitRunCommand {
        task_id: task.task_id().unwrap().clone(),
        expected_task_revision: task.revision(),
        idempotency_key: "submit".into(),
        message: "hello".into(),
        tier_id: "tier:default".into(),
        skill_ids: Vec::new(),
    };
    let first = service.submit(command.clone()).await.unwrap();
    let replay = service.submit(command.clone()).await.unwrap();
    assert_eq!(first.cursor(), replay.cursor());
    let busy = SubmitRunCommand {
        expected_task_revision: first.revision(),
        idempotency_key: "submit-2".into(),
        ..command
    };
    assert!(matches!(
        service.submit(busy).await,
        Err(super::DomainError::TaskBusy)
    ));
}

#[tokio::test]
async fn list_searches_titles_and_stop_advances_revision() {
    let dir = tempdir().unwrap();
    let service = TaskCommandService::new(TaskRepository::open(dir.path()).unwrap());
    let task = service.create_task(CreateTaskCommand {
        workspace_id: workspace_id(), idempotency_key: "create-list".into(), title: "Needle title".into(),
    }).await.unwrap();
    let page = service.list_tasks(&workspace_id(), Some(" needle ")).await.unwrap();
    assert_eq!(page.items.len(), 1);
    let queued = service.submit(SubmitRunCommand {
        task_id: task.task_id().unwrap().clone(), expected_task_revision: task.revision(),
        idempotency_key: "submit-list".into(), message: "hello".into(),
        tier_id: "tier:default".into(), skill_ids: Vec::new(),
    }).await.unwrap();
    let stopped = service.stop(StopRunCommand {
        task_id: task.task_id().unwrap().clone(), expected_task_revision: queued.revision(),
        idempotency_key: "stop-list".into(),
    }).await.unwrap();
    assert_eq!(stopped.summary().state, kuku::event::TaskState::Stopping);
}
