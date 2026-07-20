use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tempfile::tempdir;

use kuku::event::{TaskId, TaskRevision, WorkspaceId};

use super::repository::TaskRepository;
use super::store::{
    CreateTaskCommand, ResolveInteractionCommand, StopRunCommand, SubmitRunCommand,
    TaskCommandService,
};

struct RepositoryUsage {
    repository: TaskRepository,
}

impl crate::platform::WorkspaceUsagePort for RepositoryUsage {
    fn has_durable_tasks<'a>(
        &'a self,
        workspace_id: &'a WorkspaceId,
    ) -> Pin<Box<dyn Future<Output = Result<bool, crate::api::ApiError>> + Send + 'a>> {
        Box::pin(async move {
            self.repository
                .summaries()
                .map(|summaries| {
                    summaries
                        .iter()
                        .any(|summary| &summary.workspace_id == workspace_id)
                })
                .map_err(|error| error.into_api_error("task-store-test"))
        })
    }
}

fn workspace_id() -> WorkspaceId {
    WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap()
}

fn other_workspace_id() -> WorkspaceId {
    WorkspaceId::parse("wsp_1123456789abcdef01234567").unwrap()
}

fn open_service(repository: TaskRepository) -> TaskCommandService {
    TaskCommandService::new_unchecked(repository)
}

async fn create(service: &TaskCommandService, key: &str) -> crate::api::CreateTaskResponse {
    service
        .create_task(CreateTaskCommand {
            workspace_id: workspace_id(),
            idempotency_key: key.to_owned(),
        })
        .await
        .unwrap()
}

fn submit(task_id: TaskId, revision: TaskRevision, key: &str, message: &str) -> SubmitRunCommand {
    SubmitRunCommand {
        task_id,
        expected_task_revision: revision,
        idempotency_key: key.to_owned(),
        message: message.to_owned(),
        tier_id: "tier:default".to_owned(),
        skill_ids: Vec::new(),
    }
}

#[tokio::test]
async fn every_create_result_replays_after_reopen_and_conflicts_on_changed_intent() {
    let dir = tempdir().unwrap();
    let first_service = open_service(TaskRepository::open(dir.path()).unwrap());
    let first = create(&first_service, "create-1").await;
    let replay = create(&first_service, "create-1").await;
    assert!(!first.replayed);
    assert!(replay.replayed);
    assert_eq!(first.projection, replay.projection);

    let reopened = open_service(TaskRepository::open(dir.path()).unwrap());
    let replay = create(&reopened, "create-1").await;
    assert!(replay.replayed);
    assert_eq!(
        first.projection.task.task_id,
        replay.projection.task.task_id
    );
    assert!(matches!(
        reopened
            .create_task(CreateTaskCommand {
                workspace_id: other_workspace_id(),
                idempotency_key: "create-1".to_owned(),
            })
            .await,
        Err(super::DomainError::IdempotencyConflict)
    ));
}

#[tokio::test]
async fn separate_services_share_the_create_transaction_gate() {
    let dir = tempdir().unwrap();
    let left = open_service(TaskRepository::open(dir.path()).unwrap());
    let right = open_service(TaskRepository::open(dir.path()).unwrap());
    let command = CreateTaskCommand {
        workspace_id: workspace_id(),
        idempotency_key: "shared-race".to_owned(),
    };
    let (left, right) = tokio::join!(
        left.create_task(command.clone()),
        right.create_task(command),
    );
    assert_eq!(
        left.unwrap().projection.task.task_id,
        right.unwrap().projection.task.task_id
    );
    assert_eq!(
        std::fs::read_dir(dir.path().join("tasks")).unwrap().count(),
        1
    );
}

#[tokio::test]
async fn workspace_remove_and_create_race_cannot_orphan_a_task() {
    let home = tempdir().unwrap();
    let allowed = tempdir().unwrap();
    std::fs::create_dir(allowed.path().join("project")).unwrap();
    let repository = TaskRepository::open(home.path()).unwrap();
    let roots = crate::platform::RegistrationRootRegistry::from_server_config(
        home.path(),
        vec![crate::platform::RegistrationRootSpec {
            label: "Projects".to_owned(),
            path: allowed.path().to_owned(),
        }],
    )
    .unwrap();
    let registry = crate::platform::WorkspaceRegistry::open(
        home.path(),
        roots,
        Arc::new(RepositoryUsage {
            repository: repository.clone(),
        }),
        crate::platform::ServerRevisionCoordinator::open(home.path()),
    )
    .unwrap();
    let workspace = registry
        .register(crate::api::RegisterWorkspaceRequest {
            root_id: registry.registration_roots().list()[0].root_id.clone(),
            relative_path: "project".to_owned(),
            label: "Project".to_owned(),
            expected_revision: registry.revision().await.unwrap(),
        })
        .await
        .unwrap();
    let service = TaskCommandService::new_with_test_ports(repository.clone(), registry.clone());
    let create = service.create_task(CreateTaskCommand {
        workspace_id: workspace.workspace_id.clone(),
        idempotency_key: "workspace-race".to_owned(),
    });
    let remove = registry.remove(
        &workspace.workspace_id,
        crate::api::RemoveWorkspaceRequest {
            expected_revision: registry.revision().await.unwrap(),
        },
    );
    let (created, removed) = tokio::join!(create, remove);

    assert_ne!(created.is_ok(), removed.is_ok());
    if created.is_ok() {
        assert_eq!(
            removed.unwrap_err().code(),
            crate::api::ApiErrorCode::WorkspaceInUse
        );
        assert_eq!(repository.summaries().unwrap().len(), 1);
    } else {
        assert!(matches!(
            created,
            Err(super::DomainError::WorkspaceNotFound)
        ));
        assert!(repository.summaries().unwrap().is_empty());
    }
}

#[tokio::test]
async fn separate_services_serialize_task_writers_and_submit_replays_before_revision() {
    let dir = tempdir().unwrap();
    let left = open_service(TaskRepository::open(dir.path()).unwrap());
    let right = open_service(TaskRepository::open(dir.path()).unwrap());
    let task = create(&left, "create-submit").await;
    let task_id = task.projection.task.task_id;
    let revision = task.projection.task_revision;
    let (first, second) = tokio::join!(
        left.submit(submit(task_id.clone(), revision, "submit-a", "hello")),
        right.submit(submit(task_id.clone(), revision, "submit-b", "second")),
    );
    assert!(first.is_ok() ^ second.is_ok());
    assert!(matches!(
        first.as_ref().err().or(second.as_ref().err()),
        Some(super::DomainError::StaleCommand)
    ));
    let (accepted, original_key, original_message) = if let Ok(accepted) = first {
        (accepted, "submit-a", "hello")
    } else {
        (second.unwrap(), "submit-b", "second")
    };
    let replay = left
        .submit(submit(
            task_id.clone(),
            revision,
            original_key,
            original_message,
        ))
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(accepted.run_id, replay.run_id);
    assert!(matches!(
        left.submit(submit(task_id, revision, original_key, "changed"))
            .await,
        Err(super::DomainError::IdempotencyConflict)
    ));
}

#[tokio::test]
async fn stop_and_interaction_results_replay_before_revision_and_conflict() {
    let dir = tempdir().unwrap();
    let service = open_service(TaskRepository::open(dir.path()).unwrap());
    let task = create(&service, "create-commands").await;
    let task_id = task.projection.task.task_id;
    let submitted = service
        .submit(submit(
            task_id.clone(),
            task.projection.task_revision,
            "submit-commands",
            "hello",
        ))
        .await
        .unwrap();
    let stop = StopRunCommand {
        task_id: task_id.clone(),
        expected_task_revision: submitted.task_revision,
        idempotency_key: "stop-commands".to_owned(),
    };
    let first = service.stop(stop.clone()).await.unwrap();
    let replay = service.stop(stop).await.unwrap();
    assert!(!first.replayed);
    assert!(replay.replayed);
    assert_eq!(first.task_revision, replay.task_revision);

    let second_task = create(&service, "create-interaction").await;
    let second_id = second_task.projection.task.task_id;
    let submitted = service
        .submit(submit(
            second_id.clone(),
            second_task.projection.task_revision,
            "submit-interaction",
            "choose",
        ))
        .await
        .unwrap();
    let interaction_id = kuku::event::InteractionId::parse("int_0123456789abcdef01234567").unwrap();
    service
        .append_activity(
            &second_id,
            vec![
                kuku::event::TaskEvent::RunStarted {
                    run: run_fact(
                        submitted.run_id.clone(),
                        second_id.clone(),
                        kuku::event::RunState::Running,
                    ),
                },
                kuku::event::TaskEvent::InteractionOpened {
                    interaction: kuku::event::InteractionFact {
                        interaction_id: interaction_id.clone(),
                        run_id: submitted.run_id,
                        prompt: "Choose".to_owned(),
                        choices: vec![kuku::event::InteractionChoiceFact {
                            choice_id: "yes".to_owned(),
                            label: "Yes".to_owned(),
                        }],
                        selected_choice_id: None,
                    },
                },
            ],
        )
        .await
        .unwrap();
    let resolve = ResolveInteractionCommand {
        task_id: second_id,
        interaction_id,
        choice_id: "yes".to_owned(),
        expected_task_revision: submitted.task_revision,
        idempotency_key: "resolve-commands".to_owned(),
    };
    let first = service.resolve_interaction(resolve.clone()).await.unwrap();
    let replay = service.resolve_interaction(resolve.clone()).await.unwrap();
    assert!(!first.replayed);
    assert!(replay.replayed);
    let mut changed = resolve;
    changed.choice_id = "no".to_owned();
    assert!(matches!(
        service.resolve_interaction(changed).await,
        Err(super::DomainError::IdempotencyConflict)
    ));
}

#[test]
fn reopening_rejects_duplicate_durable_receipts() {
    let dir = tempdir().unwrap();
    let repository = TaskRepository::open(dir.path()).unwrap();
    let task_id = TaskId::parse("tsk_0123456789abcdef01234567").unwrap();
    let receipt = || {
        kuku::event::CommandReceipt::new(
            "duplicate",
            "digest",
            kuku::event::CommandResult::TaskCreated {
                task_id: task_id.clone(),
            },
        )
        .unwrap()
    };
    repository
        .append_initial(
            &task_id,
            kuku::event::TaskLedgerRecord::Control(
                kuku::event::TaskTransaction::try_new(
                    TaskRevision::try_new(0).unwrap(),
                    receipt(),
                    vec![kuku::event::TaskEvent::TaskCreated {
                        task_id: task_id.clone(),
                        workspace_id: workspace_id(),
                        title: "New task".to_owned(),
                        created_at: "1".to_owned(),
                    }],
                )
                .unwrap(),
            ),
        )
        .unwrap();
    kuku::event::EventStore::open(repository.events_path(&task_id))
        .unwrap()
        .append_synced(kuku::event::EventPayload::TaskLedger(
            kuku::event::TaskLedgerRecord::Control(
                kuku::event::TaskTransaction::try_new(
                    TaskRevision::try_new(1).unwrap(),
                    receipt(),
                    vec![kuku::event::TaskEvent::TaskTitleChanged {
                        title: "changed".to_owned(),
                    }],
                )
                .unwrap(),
            ),
        ))
        .unwrap();
    assert!(matches!(
        TaskRepository::open(dir.path()),
        Err(super::DomainError::LedgerCorrupt)
    ));
}

#[tokio::test]
async fn timeline_cursor_is_immutable_across_append_patch_and_upsert() {
    let dir = tempdir().unwrap();
    let repository = TaskRepository::open(dir.path()).unwrap();
    let service = open_service(repository.clone());
    let task = create(&service, "create-timeline").await;
    let task_id = task.projection.task.task_id;
    service
        .append_activity(&task_id, vec![activity_upsert("Before")])
        .await
        .unwrap();
    repository
        .append(
            &task_id,
            control_record(1, "many", messages(&task_id, 0..505)),
        )
        .unwrap();
    let cursor = service
        .projection(&task_id)
        .await
        .unwrap()
        .timeline_next_cursor
        .unwrap();
    repository
        .append(
            &task_id,
            control_record(2, "later", messages(&task_id, 505..515)),
        )
        .unwrap();
    service
        .append_activity(
            &task_id,
            vec![
                kuku::event::TaskEvent::MessagePatched {
                    message_id: "msg-0".to_owned(),
                    append_text: "-changed".to_owned(),
                    finalized: true,
                    request_ids: None,
                },
                activity_upsert("After"),
            ],
        )
        .await
        .unwrap();
    let page = service
        .timeline(
            &task_id,
            crate::api::TimelineQuery {
                before: Some(cursor),
                limit: 500,
            },
        )
        .await
        .unwrap();
    assert_eq!(page.items.len(), 6);
    assert!(matches!(
        &page.items[0],
        crate::api::TimelineItemProjection::Activity(value) if value.title == "Before"
    ));
    assert!(matches!(
        &page.items[1],
        crate::api::TimelineItemProjection::Message(value) if value.text == "0"
    ));
}

#[tokio::test]
async fn list_index_derives_title_binds_tuple_cursor_and_rejects_long_search() {
    let dir = tempdir().unwrap();
    let service = open_service(TaskRepository::open(dir.path()).unwrap());
    for index in 0..105 {
        let task = create(&service, &format!("create-{index}")).await;
        let message = if index == 0 {
            format!("\n\nNeedle {}", "x".repeat(100))
        } else {
            format!("Task {index:03}")
        };
        service
            .submit(submit(
                task.projection.task.task_id,
                task.projection.task_revision,
                &format!("submit-{index}"),
                &message,
            ))
            .await
            .unwrap();
    }
    let first = service
        .list_tasks_query(crate::api::ListTasksQuery {
            workspace_id: workspace_id(),
            search: None,
            cursor: None,
            limit: 100,
        })
        .await
        .unwrap();
    let cursor = first.next_cursor.clone().unwrap();
    let reopened = open_service(TaskRepository::open(dir.path()).unwrap());
    let second = reopened
        .list_tasks_query(crate::api::ListTasksQuery {
            workspace_id: workspace_id(),
            search: None,
            cursor: Some(cursor.clone()),
            limit: 100,
        })
        .await
        .unwrap();
    assert_eq!((first.items.len(), second.items.len()), (100, 5));
    assert!(first.items.iter().all(|left| {
        second
            .items
            .iter()
            .all(|right| left.task_id != right.task_id)
    }));
    let search = reopened
        .list_tasks_query(crate::api::ListTasksQuery {
            workspace_id: workspace_id(),
            search: Some("needle".to_owned()),
            cursor: None,
            limit: 20,
        })
        .await
        .unwrap();
    assert_eq!(search.items.len(), 1);
    assert_eq!(search.items[0].title.chars().count(), 80);
    assert!(matches!(
        reopened
            .list_tasks_query(crate::api::ListTasksQuery {
                workspace_id: workspace_id(),
                search: Some("x".repeat(257)),
                cursor: None,
                limit: 20,
            })
            .await,
        Err(super::DomainError::InvalidRequest)
    ));
    assert!(matches!(
        reopened
            .list_tasks_query(crate::api::ListTasksQuery {
                workspace_id: workspace_id(),
                search: Some("different".to_owned()),
                cursor: Some(cursor),
                limit: 100,
            })
            .await,
        Err(super::DomainError::InvalidRequest)
    ));
}

#[tokio::test]
async fn timeline_pages_are_gap_free_and_choose_the_largest_bounded_suffix() {
    let dir = tempdir().unwrap();
    let repository = TaskRepository::open(dir.path()).unwrap();
    let service = open_service(repository.clone());
    let task = create(&service, "create-long").await;
    let task_id = task.projection.task.task_id;
    repository
        .append(
            &task_id,
            control_record(1, "long", messages(&task_id, 0..1203)),
        )
        .unwrap();
    let projection = service.projection(&task_id).await.unwrap();
    assert_eq!(projection.timeline.len(), 500);
    let first = service
        .timeline(
            &task_id,
            crate::api::TimelineQuery {
                before: projection.timeline_next_cursor,
                limit: 500,
            },
        )
        .await
        .unwrap();
    let second = service
        .timeline(
            &task_id,
            crate::api::TimelineQuery {
                before: first.next_cursor,
                limit: 500,
            },
        )
        .await
        .unwrap();
    assert_eq!((first.items.len(), second.items.len()), (500, 203));

    let large = "x".repeat(9 * 1024 * 1024);
    kuku::event::EventStore::open(repository.events_path(&task_id))
        .unwrap()
        .append_synced(kuku::event::EventPayload::TaskLedger(control_record(
            2,
            "large",
            vec![
                message(&task_id, "large-a", &large),
                message(&task_id, "large-b", &large),
            ],
        )))
        .unwrap();
    let projection = service.projection(&task_id).await.unwrap();
    assert_eq!(projection.timeline.len(), 1);
    assert!(serde_json::to_vec(&projection).unwrap().len() <= 16 * 1024 * 1024);
    let page = service
        .timeline(
            &task_id,
            crate::api::TimelineQuery {
                before: projection.timeline_next_cursor,
                limit: 500,
            },
        )
        .await
        .unwrap();
    assert!(serde_json::to_vec(&page).unwrap().len() <= 16 * 1024 * 1024);

    assert!(matches!(
        repository.append(
            &task_id,
            control_record(
                3,
                "oversized",
                vec![message(
                    &task_id,
                    "oversized",
                    &"x".repeat(16 * 1024 * 1024 + 1),
                )],
            ),
        ),
        Err(super::DomainError::PayloadTooLarge)
    ));
    let projection = service.projection(&task_id).await.unwrap();
    assert_eq!(projection.task_revision, TaskRevision::try_new(2).unwrap());
    assert_eq!(projection.timeline.len(), 1);
    assert_eq!(repository.replay(&task_id).unwrap().len(), 3);
}

#[tokio::test]
async fn unknown_publication_outcomes_replay_without_reopening() {
    let dir = tempdir().unwrap();
    let repository = TaskRepository::open(dir.path()).unwrap();
    let service = open_service(repository.clone());
    repository.fail_next_publication_for_test();
    let command = CreateTaskCommand {
        workspace_id: workspace_id(),
        idempotency_key: "uncertain-create".to_owned(),
    };
    assert!(service.create_task(command.clone()).await.is_err());
    let task_id = repository.task_ids().unwrap()[0].clone();
    let replay = service.create_task(command).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.projection.task.task_id, task_id);

    repository.fail_next_publication_for_test();
    let command = submit(
        task_id,
        replay.projection.task_revision,
        "uncertain-submit",
        "hello",
    );
    assert!(service.submit(command.clone()).await.is_err());
    assert!(service.submit(command).await.unwrap().replayed);
}

#[test]
fn timeline_window_uses_the_untrimmed_record_candidate() {
    let task_id = TaskId::parse("tsk_3123456789abcdef01234567").unwrap();
    let mut aggregate = super::TaskAggregate::default();
    let created = control_record(
        0,
        "window-create",
        vec![kuku::event::TaskEvent::TaskCreated {
            task_id: task_id.clone(),
            workspace_id: workspace_id(),
            title: "New task".to_owned(),
            created_at: "2026-07-20T00:00:00Z".to_owned(),
        }],
    );
    super::projection::reduce_record(
        &mut aggregate,
        kuku::event::Cursor::try_new(1).unwrap(),
        &created,
    )
    .unwrap();
    let one = control_record(1, "window-one", messages(&task_id, 0..1));
    let delta = super::projection::reduce_record(
        &mut aggregate,
        kuku::event::Cursor::try_new(2).unwrap(),
        &one,
    )
    .unwrap();
    assert!(matches!(delta, crate::api::TaskDelta::ChangesApplied {
        timeline_window: Some(crate::api::TimelineWindowDelta { ref evicted_items, next_cursor: None }), ..
    } if evicted_items.is_empty()));

    let many = control_record(2, "window-many", messages(&task_id, 1..601));
    let delta = super::projection::reduce_record(
        &mut aggregate,
        kuku::event::Cursor::try_new(3).unwrap(),
        &many,
    )
    .unwrap();
    assert!(matches!(delta, crate::api::TaskDelta::ChangesApplied {
        timeline_window: Some(crate::api::TimelineWindowDelta { ref evicted_items, .. }), ..
    } if evicted_items.len() == 101
        && matches!(&evicted_items[0], crate::api::TimelineItemProjection::Message(item) if item.message_id == "msg-0")
        && matches!(&evicted_items[100], crate::api::TimelineItemProjection::Message(item) if item.message_id == "msg-100")));

    let sdk_only = control_record(
        3,
        "window-sdk",
        vec![kuku::event::TaskEvent::ReviewSubmissionRecorded(
            kuku::event::ReviewSubmissionRecorded {
                submission_id: kuku::event::ReviewSubmissionId::parse(
                    "rsub_0123456789abcdef01234567",
                )
                .unwrap(),
                task_id: task_id.clone(),
                run_id: kuku::event::RunId::parse("run_3123456789abcdef01234567").unwrap(),
                task_revision: TaskRevision::try_new(3).unwrap(),
                submitted_at: "2026-07-20T00:00:00Z".to_owned(),
                notes: Vec::new(),
            },
        )],
    );
    let delta = super::projection::reduce_record(
        &mut aggregate,
        kuku::event::Cursor::try_new(4).unwrap(),
        &sdk_only,
    )
    .unwrap();
    assert!(matches!(delta, crate::api::TaskDelta::ChangesApplied {
        ref changes, timeline_window: None,
    } if matches!(changes.as_slice(), [crate::api::TaskChange::ReviewSubmissionsChanged { .. }])));

    let patch = kuku::event::TaskLedgerRecord::Activity(
        kuku::event::TaskActivityBatch::try_new(vec![kuku::event::TaskEvent::MessagePatched {
            message_id: "msg-600".to_owned(),
            append_text: " patched".to_owned(),
            finalized: true,
            request_ids: None,
        }])
        .unwrap(),
    );
    let delta = super::projection::reduce_record(
        &mut aggregate,
        kuku::event::Cursor::try_new(5).unwrap(),
        &patch,
    )
    .unwrap();
    assert!(matches!(
        delta,
        crate::api::TaskDelta::ChangesApplied {
            timeline_window: None,
            ..
        }
    ));

    let mut byte_bounded = super::TaskAggregate::default();
    super::projection::reduce_record(
        &mut byte_bounded,
        kuku::event::Cursor::try_new(1).unwrap(),
        &control_record(
            0,
            "byte-create",
            vec![kuku::event::TaskEvent::TaskCreated {
                task_id: task_id.clone(),
                workspace_id: workspace_id(),
                title: "New task".to_owned(),
                created_at: "2026-07-20T00:00:00Z".to_owned(),
            }],
        ),
    )
    .unwrap();
    let large = "x".repeat(9 * 1024 * 1024);
    let delta = super::projection::reduce_record(
        &mut byte_bounded,
        kuku::event::Cursor::try_new(2).unwrap(),
        &control_record(
            1,
            "byte-window",
            vec![
                message(&task_id, "byte-a", &large),
                message(&task_id, "byte-b", &large),
            ],
        ),
    )
    .unwrap();
    assert!(matches!(delta, crate::api::TaskDelta::ChangesApplied {
        timeline_window: Some(crate::api::TimelineWindowDelta { ref evicted_items, .. }), ..
    } if matches!(evicted_items.as_slice(), [crate::api::TimelineItemProjection::Message(item)] if item.message_id == "byte-a")));
}

fn messages(task_id: &TaskId, range: std::ops::Range<usize>) -> Vec<kuku::event::TaskEvent> {
    range
        .map(|index| message(task_id, &format!("msg-{index}"), &index.to_string()))
        .collect()
}

fn message(task_id: &TaskId, id: &str, text: &str) -> kuku::event::TaskEvent {
    kuku::event::TaskEvent::MessageAppended {
        message: kuku::event::MessageFact {
            message_id: id.to_owned(),
            task_id: task_id.clone(),
            run_id: None,
            role: kuku::event::MessageRoleFact::Agent,
            text: text.to_owned(),
            finalized: true,
            request_ids: Vec::new(),
            file_references: Vec::new(),
        },
    }
}

fn activity_upsert(title: &str) -> kuku::event::TaskEvent {
    kuku::event::TaskEvent::ActivityUpserted {
        activity: kuku::event::ActivityFact {
            activity_id: "activity-snapshot".to_owned(),
            run_id: kuku::event::RunId::parse("run_3123456789abcdef01234567").unwrap(),
            title: title.to_owned(),
            kind: kuku::event::ActivityKindFact::Tool,
            status: kuku::event::ActivityStatusFact::Running,
            detail: None,
            file_references: Vec::new(),
        },
    }
}

fn control_record(
    revision: u64,
    key: &str,
    events: Vec<kuku::event::TaskEvent>,
) -> kuku::event::TaskLedgerRecord {
    kuku::event::TaskLedgerRecord::Control(
        kuku::event::TaskTransaction::try_new(
            TaskRevision::try_new(revision).unwrap(),
            kuku::event::CommandReceipt::new(key, key, kuku::event::CommandResult::Stopped)
                .unwrap(),
            events,
        )
        .unwrap(),
    )
}

fn run_fact(
    run_id: kuku::event::RunId,
    task_id: TaskId,
    state: kuku::event::RunState,
) -> kuku::event::RunFact {
    kuku::event::RunFact {
        run_id,
        task_id,
        state,
        started_at: "1".to_owned(),
        finished_at: None,
        summary: None,
        checks: None,
        metrics: None,
        workspace_changes: None,
    }
}

#[test]
fn reducer_publishes_run_state_changes_for_interactions_and_empty_changes_for_sdk_facts() {
    let task_id = TaskId::parse("tsk_2123456789abcdef01234567").unwrap();
    let run_id = kuku::event::RunId::parse("run_0123456789abcdef01234567").unwrap();
    let interaction_id = kuku::event::InteractionId::parse("int_2123456789abcdef01234567").unwrap();
    let mut aggregate = super::TaskAggregate::default();
    let created = control_record(
        0,
        "created-reducer",
        vec![kuku::event::TaskEvent::TaskCreated {
            task_id: task_id.clone(),
            workspace_id: workspace_id(),
            title: "New task".to_owned(),
            created_at: "1970-01-01T00:00:00.000000000Z".to_owned(),
        }],
    );
    super::projection::reduce_record(
        &mut aggregate,
        kuku::event::Cursor::try_new(1).unwrap(),
        &created,
    )
    .unwrap();
    let queued = control_record(
        1,
        "queued-reducer",
        vec![kuku::event::TaskEvent::RunQueued {
            run: run_fact(
                run_id.clone(),
                task_id.clone(),
                kuku::event::RunState::Queued,
            ),
        }],
    );
    super::projection::reduce_record(
        &mut aggregate,
        kuku::event::Cursor::try_new(2).unwrap(),
        &queued,
    )
    .unwrap();
    let opened = kuku::event::TaskLedgerRecord::Activity(
        kuku::event::TaskActivityBatch::try_new(vec![
            kuku::event::TaskEvent::RunStarted {
                run: run_fact(
                    run_id.clone(),
                    task_id.clone(),
                    kuku::event::RunState::Running,
                ),
            },
            kuku::event::TaskEvent::InteractionOpened {
                interaction: kuku::event::InteractionFact {
                    interaction_id,
                    run_id,
                    prompt: "Choose".to_owned(),
                    choices: vec![kuku::event::InteractionChoiceFact {
                        choice_id: "yes".to_owned(),
                        label: "Yes".to_owned(),
                    }],
                    selected_choice_id: None,
                },
            },
        ])
        .unwrap(),
    );
    let delta = super::projection::reduce_record(
        &mut aggregate,
        kuku::event::Cursor::try_new(3).unwrap(),
        &opened,
    )
    .unwrap();
    assert!(
        matches!(delta, crate::api::TaskDelta::ChangesApplied { changes, .. }
        if changes.iter().any(|change| matches!(change, crate::api::TaskChange::RunStateChanged { task, .. }
            if task.state == kuku::event::TaskState::NeedsAttention)))
    );

    let sdk_only = kuku::event::TaskLedgerRecord::Activity(
        kuku::event::TaskActivityBatch::try_new(vec![kuku::event::TaskEvent::RequestStarted(
            request_started(&task_id),
        )])
        .unwrap(),
    );
    let delta = super::projection::reduce_record(
        &mut aggregate,
        kuku::event::Cursor::try_new(4).unwrap(),
        &sdk_only,
    )
    .unwrap();
    assert!(
        matches!(delta, crate::api::TaskDelta::ChangesApplied { changes, .. } if changes.is_empty())
    );
}

fn request_started(task_id: &TaskId) -> kuku::event::RequestStarted {
    kuku::event::RequestStarted {
        scope: kuku::event::RequestScope {
            execution: kuku::event::ExecutionScope {
                workspace_id: workspace_id(),
                task_id: task_id.clone(),
                run_id: kuku::event::RunId::parse("run_1123456789abcdef01234567").unwrap(),
                turn_id: kuku::event::TurnId::parse("trn_0123456789abcdef01234567").unwrap(),
                conversation_id: kuku::event::ConversationId::parse("con_0123456789abcdef01234567")
                    .unwrap(),
                turn_index: 0,
            },
            request_id: kuku::event::RequestId::parse("req_0123456789abcdef01234567").unwrap(),
        },
        cause: kuku::event::RequestCause::UserSubmission,
        provider: kuku::event::ProviderFact::OpenAiCompatible,
        model: "model".to_owned(),
        started_at: "1970-01-01T00:00:00Z".to_owned(),
    }
}
