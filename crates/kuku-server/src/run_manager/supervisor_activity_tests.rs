use super::*;

#[tokio::test]
async fn consecutive_activity_events_share_bounded_ledger_batches() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake.clone(), 1, 0).unwrap();
    let created = runtime
        .create_task(create("activity-batch-create"))
        .await
        .unwrap();
    let task_id = created.projection.task.task_id.clone();
    let accepted = runtime
        .submit(submit(
            task_id.clone(),
            created.projection.task_revision,
            "activity-batch-submit",
        ))
        .await
        .unwrap();
    wait_for_state(&runtime, &task_id, RunState::Running).await;
    let before = runtime.repository().replay(&task_id).unwrap().len();
    for index in 0..4 {
        fake.enqueue(
            &accepted.run_id,
            DriverEvent::Activity(vec![TaskEvent::MessagePatched {
                message_id: format!("msg_agent_{}", accepted.run_id.as_str()),
                append_text: format!("chunk-{index}"),
                finalized: false,
                request_ids: None,
            }]),
        );
    }
    fake.enqueue(
        &accepted.run_id,
        DriverEvent::Completed(RunResult {
            summary: "done".to_owned(),
            warnings: Vec::new(),
            checks: None,
            metrics: None,
            workspace_changes: None,
        }),
    );

    wait_for_state(&runtime, &task_id, RunState::Completed).await;
    let appended = runtime.repository().replay(&task_id).unwrap().len() - before;
    assert_eq!(
        appended, 2,
        "activity queue was persisted one event at a time"
    );
}
