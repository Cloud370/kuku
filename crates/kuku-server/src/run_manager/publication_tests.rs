use std::sync::{Arc, Barrier, Mutex};

use tempfile::tempdir;

use kuku::event::{
    ActivityFact, ActivityKindFact, ActivityStatusFact, EventPayload, TaskActivityBatch, TaskEvent,
    TaskLedgerRecord, WorkspaceId,
};

use super::repository::TaskRepository;
use super::store::{CreateTaskCommand, TaskCommandService};

async fn task(repository: &TaskRepository) -> kuku::event::TaskId {
    TaskCommandService::new_unchecked(repository.clone())
        .create_task(CreateTaskCommand {
            workspace_id: WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap(),
            idempotency_key: "publication-create".to_owned(),
        })
        .await
        .unwrap()
        .projection
        .task
        .task_id
}

fn activity(id: &str) -> TaskLedgerRecord {
    activity_with_detail(id, None)
}

fn activity_with_detail(id: &str, detail: Option<String>) -> TaskLedgerRecord {
    TaskLedgerRecord::Activity(
        TaskActivityBatch::try_new(vec![TaskEvent::ActivityUpserted {
            activity: ActivityFact {
                activity_id: id.to_owned(),
                run_id: kuku::event::RunId::parse("run_0123456789abcdef01234567").unwrap(),
                title: id.to_owned(),
                kind: ActivityKindFact::System,
                status: ActivityStatusFact::Completed,
                detail,
                conversation_id: None,
                agent: None,
                tier: None,
                result_in_main: None,
                file_references: Vec::new(),
            },
        }])
        .unwrap(),
    )
}

#[tokio::test]
async fn external_oversized_activity_publishes_a_bounded_replacement_and_converges() {
    let dir = tempdir().unwrap();
    let repository = TaskRepository::open(dir.path()).unwrap();
    let task_id = task(&repository).await;
    let publications = Arc::new(Mutex::new(Vec::new()));
    let observed = publications.clone();
    repository.register_observer(Arc::new(move |publication| {
        observed.lock().unwrap().push(publication.clone());
    }));
    let mut store = kuku::event::EventStore::open(repository.events_path(&task_id)).unwrap();
    let oversized = store
        .append_synced(EventPayload::TaskLedger(activity_with_detail(
            "oversized-external",
            Some("x".repeat(16 * 1024 * 1024 + 1)),
        )))
        .unwrap();

    let projection = TaskCommandService::new_unchecked(repository.clone())
        .projection(&task_id)
        .await
        .unwrap();
    assert_eq!(projection.cursor.get(), oversized.id);
    assert!(projection.timeline.is_empty());
    let before = projection.timeline_next_cursor.clone().unwrap();
    assert!(serde_json::to_vec(&projection).unwrap().len() <= 16 * 1024 * 1024);
    assert!(matches!(
        TaskCommandService::new_unchecked(repository.clone())
            .timeline(
                &task_id,
                crate::api::TimelineQuery {
                    before: Some(before),
                    limit: 500
                },
            )
            .await,
        Err(super::DomainError::PayloadTooLarge)
    ));
    {
        let publications = publications.lock().unwrap();
        assert_eq!(publications.len(), 1);
        assert_eq!(publications[0].event.id, oversized.id);
        assert_eq!(publications[0].projection.cursor.get(), oversized.id);
        assert!(matches!(
            &publications[0].delta,
            crate::api::TaskDelta::ProjectionReplaced { projection }
                if projection.timeline.is_empty()
                    && projection.timeline_next_cursor.is_some()
                    && serde_json::to_vec(projection).unwrap().len() <= 16 * 1024 * 1024
        ));
    }
    drop(store);
    drop(repository);
    let reopened = TaskRepository::open(dir.path()).unwrap();
    let projection = reopened.rebuild(&task_id).unwrap().projection().unwrap();
    assert_eq!(projection.cursor.get(), oversized.id);
    assert!(projection.timeline.is_empty());
    assert!(projection.timeline_next_cursor.is_some());
    let cached: crate::api::TaskProjection = serde_json::from_slice(
        &std::fs::read(reopened.task_path(&task_id).join("projection.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(cached, projection);
    let mut store = kuku::event::EventStore::open(reopened.events_path(&task_id)).unwrap();
    let small = store
        .append_synced(EventPayload::TaskLedger(activity("small-external")))
        .unwrap();

    let publications = publications.lock().unwrap();
    assert_eq!(publications.len(), 2);
    assert!(matches!(
        &publications[1].delta,
        crate::api::TaskDelta::ChangesApplied { changes, timeline_window: Some(_) }
            if matches!(changes.as_slice(), [crate::api::TaskChange::ActivityUpserted { activity }] if activity.activity_id == "small-external")
                && serde_json::to_vec(&publications[1].delta).unwrap().len() <= 16 * 1024 * 1024
    ));
    assert_eq!(publications[1].projection.cursor.get(), small.id);
    assert_eq!(publications[1].projection.timeline.len(), 1);
}

#[tokio::test]
async fn cache_failure_repairs_with_a_full_projection_before_observer_delivery() {
    let dir = tempdir().unwrap();
    let repository = TaskRepository::open(dir.path()).unwrap();
    let task_id = task(&repository).await;
    let publications = Arc::new(Mutex::new(Vec::new()));
    let observed = publications.clone();
    let cache_path = repository.task_path(&task_id).join("projection.json");
    repository.register_observer(Arc::new(move |publication| {
        let cached: crate::api::TaskProjection =
            serde_json::from_slice(&std::fs::read(&cache_path).unwrap()).unwrap();
        assert_eq!(cached, publication.projection);
        observed.lock().unwrap().push(publication.delta.clone());
    }));

    repository.fail_next_cache_for_test();
    assert!(repository.append(&task_id, activity("missed")).is_err());
    assert!(publications.lock().unwrap().is_empty());
    repository.append(&task_id, activity("recovered")).unwrap();

    let publications = publications.lock().unwrap();
    assert_eq!(publications.len(), 1);
    assert!(matches!(
        publications[0],
        crate::api::TaskDelta::ProjectionReplaced { .. }
    ));
}

#[tokio::test]
async fn receipt_repairs_publish_one_cache_consistent_replacement() {
    let dir = tempdir().unwrap();
    let repository = TaskRepository::open(dir.path()).unwrap();
    let service = TaskCommandService::new_unchecked(repository.clone());
    let publications = Arc::new(Mutex::new(Vec::new()));
    let observed = publications.clone();
    let cache_reader = repository.clone();
    repository.register_observer(Arc::new(move |publication| {
        let cached: crate::api::TaskProjection = serde_json::from_slice(
            &std::fs::read(
                cache_reader
                    .task_path(&publication.projection.task.task_id)
                    .join("projection.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(cached, publication.projection);
        observed.lock().unwrap().push(publication.clone());
    }));

    let publication_failure = CreateTaskCommand {
        workspace_id: WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap(),
        idempotency_key: "repair-publication".to_owned(),
    };
    repository.fail_next_publication_for_test();
    assert!(service
        .create_task(publication_failure.clone())
        .await
        .is_err());
    assert!(publications.lock().unwrap().is_empty());
    repository.fail_next_cache_for_test();
    assert!(service
        .create_task(publication_failure.clone())
        .await
        .is_err());
    assert!(publications.lock().unwrap().is_empty());
    let repaired = service
        .create_task(publication_failure.clone())
        .await
        .unwrap();
    assert!(repaired.replayed);

    {
        let publications = publications.lock().unwrap();
        assert_eq!(publications.len(), 1);
        assert_eq!(publications[0].event.id, repaired.projection.cursor.get());
        assert!(matches!(
            &publications[0].delta,
            crate::api::TaskDelta::ProjectionReplaced { projection }
                if projection.as_ref() == &repaired.projection
        ));
    }

    let healthy_replay = service.create_task(publication_failure).await.unwrap();
    assert!(healthy_replay.replayed);
    assert_eq!(publications.lock().unwrap().len(), 1);

    let late_failure = CreateTaskCommand {
        workspace_id: WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap(),
        idempotency_key: "repair-late-write".to_owned(),
    };
    repository.fail_next_store_after_write_for_test();
    repository.fail_durability_confirmations_for_test(2);
    assert!(service.create_task(late_failure.clone()).await.is_err());
    assert_eq!(publications.lock().unwrap().len(), 1);
    assert!(service.create_task(late_failure.clone()).await.is_err());
    assert_eq!(publications.lock().unwrap().len(), 1);
    let late_repaired = service.create_task(late_failure).await.unwrap();
    assert!(late_repaired.replayed);

    {
        let publications = publications.lock().unwrap();
        assert_eq!(publications.len(), 2);
        assert_eq!(
            publications[1].event.id,
            late_repaired.projection.cursor.get()
        );
        assert!(matches!(
            &publications[1].delta,
            crate::api::TaskDelta::ProjectionReplaced { projection }
                if projection.as_ref() == &late_repaired.projection
        ));
    }

    let appended = repository
        .append(&repaired.projection.task.task_id, activity("after-repair"))
        .unwrap();
    let publications = publications.lock().unwrap();
    assert_eq!(publications.len(), 3);
    assert_eq!(publications[2].event.id, appended.id);
    assert!(matches!(
        publications[2].delta,
        crate::api::TaskDelta::ChangesApplied { .. }
    ));
}

#[tokio::test]
async fn repair_and_raw_append_publish_monotonic_cache_consistent_updates() {
    let dir = tempdir().unwrap();
    let repository = TaskRepository::open(dir.path()).unwrap();
    let task_id = task(&repository).await;
    let publications = Arc::new(Mutex::new(Vec::new()));
    let observed = publications.clone();
    let cache_reader = repository.clone();
    repository.register_observer(Arc::new(move |publication| {
        let cached: crate::api::TaskProjection = serde_json::from_slice(
            &std::fs::read(
                cache_reader
                    .task_path(&publication.projection.task.task_id)
                    .join("projection.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(cached, publication.projection);
        observed.lock().unwrap().push(publication.clone());
    }));

    repository.fail_next_publication_for_test();
    assert!(repository
        .append(&task_id, activity("repair-tail"))
        .is_err());
    let repair_tail = repository.replay(&task_id).unwrap().last().unwrap().id;
    let snapshot_reached = Arc::new(Barrier::new(2));
    let resume_repair = Arc::new(Barrier::new(2));
    repository
        .pause_next_repair_after_snapshot_for_test(snapshot_reached.clone(), resume_repair.clone());

    let repair_repository = repository.clone();
    let repair = std::thread::spawn(move || repair_repository.task_ids().unwrap());
    snapshot_reached.wait();

    let mut raw_store = kuku::event::EventStore::open(repository.events_path(&task_id)).unwrap();
    let (started, competing) = std::sync::mpsc::channel();
    let raw_append = std::thread::spawn(move || {
        started.send(()).unwrap();
        raw_store
            .append_synced(EventPayload::TaskLedger(activity("after-repair")))
            .unwrap()
    });
    competing.recv().unwrap();
    resume_repair.wait();

    repair.join().unwrap();
    let appended = raw_append.join().unwrap();
    let publications = publications.lock().unwrap();
    assert_eq!(publications.len(), 2);
    assert_eq!(publications[0].event.id, repair_tail);
    assert!(matches!(
        publications[0].delta,
        crate::api::TaskDelta::ProjectionReplaced { .. }
    ));
    assert_eq!(publications[1].event.id, appended.id);
    assert!(matches!(
        publications[1].delta,
        crate::api::TaskDelta::ChangesApplied { .. }
    ));
    assert!(publications[0].projection.cursor < publications[1].projection.cursor);
    let cached: crate::api::TaskProjection = serde_json::from_slice(
        &std::fs::read(repository.task_path(&task_id).join("projection.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(cached, publications[1].projection);
}

#[tokio::test]
async fn external_appends_advance_cache_without_leaking_wait_results_or_empty_ui_deltas() {
    let dir = tempdir().unwrap();
    let repository = TaskRepository::open(dir.path()).unwrap();
    let task_id = task(&repository).await;
    let publications = Arc::new(Mutex::new(Vec::new()));
    let observed = publications.clone();
    repository.register_observer(Arc::new(move |publication| {
        observed.lock().unwrap().push(publication.clone());
    }));
    let mut store = kuku::event::EventStore::open(repository.events_path(&task_id)).unwrap();

    let raw = store
        .append_synced(EventPayload::ModelError {
            conversation: None,
            request_id: "req_0123456789abcdef01234567".to_owned(),
            turn: 1,
            ts: "1".to_owned(),
            kind: "test".to_owned(),
            message: "test".to_owned(),
        })
        .unwrap();
    assert!(publications.lock().unwrap().is_empty());
    let cached: crate::api::TaskProjection = serde_json::from_slice(
        &std::fs::read(repository.task_path(&task_id).join("projection.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(cached.cursor.get(), raw.id);
    assert_eq!(repository.publication_results_len_for_test(), 0);

    let external = store
        .append_synced(EventPayload::TaskLedger(activity("external")))
        .unwrap();
    let publications = publications.lock().unwrap();
    assert_eq!(publications.len(), 1);
    assert_eq!(publications[0].event.id, external.id);
    assert_eq!(publications[0].projection.cursor.get(), external.id);
    assert_eq!(repository.publication_results_len_for_test(), 0);
}

#[test]
fn opening_existing_ledgers_registers_publication_observers() {
    let dir = tempdir().unwrap();
    let task_id = kuku::event::TaskId::parse("tsk_2123456789abcdef01234567").unwrap();
    let events_path = dir
        .path()
        .join("tasks")
        .join(task_id.as_str())
        .join("events.jsonl");
    let transaction = kuku::event::TaskTransaction::try_new(
        kuku::event::TaskRevision::try_new(0).unwrap(),
        kuku::event::CommandReceipt::new(
            "existing-create",
            "existing-create",
            kuku::event::CommandResult::TaskCreated {
                task_id: task_id.clone(),
            },
        )
        .unwrap(),
        vec![TaskEvent::TaskCreated {
            task_id: task_id.clone(),
            workspace_id: WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap(),
            title: "Existing".to_owned(),
            created_at: "2026-07-20T00:00:00Z".to_owned(),
        }],
    )
    .unwrap();
    kuku::event::EventStore::open(&events_path)
        .unwrap()
        .append_synced(EventPayload::TaskLedger(TaskLedgerRecord::Control(
            transaction,
        )))
        .unwrap();

    let repository = TaskRepository::open(dir.path()).unwrap();
    let publications = Arc::new(Mutex::new(Vec::new()));
    let observed = publications.clone();
    repository.register_observer(Arc::new(move |publication| {
        observed.lock().unwrap().push(publication.event.id);
    }));
    let appended = kuku::event::EventStore::open(&events_path)
        .unwrap()
        .append_synced(EventPayload::TaskLedger(activity("after-reopen")))
        .unwrap();

    assert_eq!(publications.lock().unwrap().as_slice(), [appended.id]);
    let cached: crate::api::TaskProjection = serde_json::from_slice(
        &std::fs::read(repository.task_path(&task_id).join("projection.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(cached.cursor.get(), appended.id);
}
