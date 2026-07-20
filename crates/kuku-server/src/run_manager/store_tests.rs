use tempfile::tempdir;

use kuku::event::WorkspaceId;

use super::repository::TaskRepository;
use super::store::{ResolveInteractionCommand, StopRunCommand, SubmitRunCommand};
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

#[tokio::test]
async fn resolve_interaction_is_revision_serialized() {
    let dir = tempdir().unwrap();
    let service = TaskCommandService::new(TaskRepository::open(dir.path()).unwrap());
    let task = service.create_task(CreateTaskCommand {
        workspace_id: workspace_id(), idempotency_key: "create-interaction".into(), title: "Task".into(),
    }).await.unwrap();
    let submitted = service.submit(SubmitRunCommand {
        task_id: task.task_id().unwrap().clone(), expected_task_revision: task.revision(),
        idempotency_key: "submit-interaction".into(), message: "hello".into(), tier_id: "tier:default".into(), skill_ids: Vec::new(),
    }).await.unwrap();
    let interaction_id: kuku::event::InteractionId = "int_0123456789abcdef01234567".parse().unwrap();
    let run_id = submitted.projection().unwrap().active_run.unwrap().run_id;
    let _ = service.append_activity(&task.task_id().unwrap().clone(), vec![
        kuku::event::TaskEvent::RunStarted { run: kuku::event::RunFact {
            run_id: run_id.clone(), task_id: task.task_id().unwrap().clone(), state: kuku::event::RunState::Running,
            started_at: "t".into(), finished_at: None, summary: None, checks: None, metrics: None, workspace_changes: None,
        }},
        kuku::event::TaskEvent::InteractionOpened {
            interaction: kuku::event::InteractionFact {
                interaction_id: interaction_id.clone(), run_id, prompt: "Choose".into(),
                choices: vec![kuku::event::InteractionChoiceFact { choice_id: "yes".into(), label: "Yes".into() }], selected_choice_id: None,
            },
        },
    ]).await.unwrap();
    let resolved = service.resolve_interaction(ResolveInteractionCommand {
        task_id: task.task_id().unwrap().clone(), interaction_id, choice_id: "yes".into(),
        expected_task_revision: submitted.revision(), idempotency_key: "resolve".into(),
    }).await.unwrap();
    assert_eq!(resolved.summary().state, kuku::event::TaskState::Running);
}
