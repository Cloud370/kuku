use std::sync::{Arc, Mutex};

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
    TaskLedgerRecord::Activity(
        TaskActivityBatch::try_new(vec![TaskEvent::ActivityUpserted {
            activity: ActivityFact {
                activity_id: id.to_owned(),
                run_id: kuku::event::RunId::parse("run_0123456789abcdef01234567").unwrap(),
                title: id.to_owned(),
                kind: ActivityKindFact::System,
                status: ActivityStatusFact::Completed,
                detail: None,
                file_references: Vec::new(),
            },
        }])
        .unwrap(),
    )
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
            turn: 1,
            ts: "1".to_owned(),
            request_id: "req_external".to_owned(),
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
