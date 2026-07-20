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
    let replay_after_reopen = reopened.create_task(CreateTaskCommand {
        workspace_id: workspace_id(), idempotency_key: "create-1".into(), title: "First task".into(),
    }).await.unwrap();
    assert_eq!(replay_after_reopen.task_id(), first.task_id());
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

#[tokio::test]
async fn timeline_cursor_keeps_an_immutable_snapshot_across_appends() {
    let dir = tempdir().unwrap();
    let repository = TaskRepository::open(dir.path()).unwrap();
    let service = TaskCommandService::new(repository.clone());
    let task = service.create_task(CreateTaskCommand {
        workspace_id: workspace_id(), idempotency_key: "create-timeline".into(), title: "Timeline".into(),
    }).await.unwrap();
    let task_id = task.task_id().unwrap().clone();
    let messages = |range: std::ops::Range<usize>| range.map(|index| kuku::event::TaskEvent::MessageAppended {
        message: kuku::event::MessageFact {
            message_id: format!("msg-{index}"), task_id: task_id.clone(), run_id: None,
            role: kuku::event::MessageRoleFact::Agent, text: index.to_string(), finalized: true,
            request_ids: Vec::new(), file_references: Vec::new(),
        },
    }).collect::<Vec<_>>();
    let receipt = |key: &str| kuku::event::CommandReceipt::new(
        key, key, kuku::event::CommandResult::Stopped,
    ).unwrap();
    repository.append(&task_id, kuku::event::TaskLedgerRecord::Control(
        kuku::event::TaskTransaction::try_new(kuku::event::TaskRevision::try_new(1).unwrap(), receipt("many"), messages(0..505)).unwrap(),
    )).unwrap();
    let projection = service.projection(&task_id).await.unwrap();
    let cursor = projection.timeline_next_cursor.unwrap();
    repository.append(&task_id, kuku::event::TaskLedgerRecord::Control(
        kuku::event::TaskTransaction::try_new(kuku::event::TaskRevision::try_new(2).unwrap(), receipt("later"), messages(505..515)).unwrap(),
    )).unwrap();
    let page = service.timeline(&task_id, crate::api::TimelineQuery { before: Some(cursor), limit: 500 }).await.unwrap();
    assert_eq!(page.items.len(), 5);
    assert!(matches!(&page.items[0], crate::api::TimelineItemProjection::Message(value) if value.text == "0"));
    assert!(page.next_cursor.is_none());
}

#[tokio::test]
async fn task_list_cursor_is_bound_to_workspace_search_and_limit() {
    let dir = tempdir().unwrap();
    let service = TaskCommandService::new(TaskRepository::open(dir.path()).unwrap());
    for (key, title) in [("one", "Needle one"), ("two", "Needle two")] {
        service.create_task(CreateTaskCommand {
            workspace_id: workspace_id(), idempotency_key: key.into(), title: title.into(),
        }).await.unwrap();
    }
    let first = service.list_tasks_query(crate::api::ListTasksQuery {
        workspace_id: workspace_id(), search: Some(" NEEDLE ".into()), cursor: None, limit: 1,
    }).await.unwrap();
    assert_eq!(first.items.len(), 1);
    let cursor = first.next_cursor.unwrap();
    let second = service.list_tasks_query(crate::api::ListTasksQuery {
        workspace_id: workspace_id(), search: Some("needle".into()), cursor: Some(cursor.clone()), limit: 1,
    }).await.unwrap();
    assert_eq!(second.items.len(), 1);
    assert!(matches!(service.list_tasks_query(crate::api::ListTasksQuery {
        workspace_id: workspace_id(), search: Some("different".into()), cursor: Some(cursor), limit: 1,
    }).await, Err(super::DomainError::InvalidRequest)));
}
