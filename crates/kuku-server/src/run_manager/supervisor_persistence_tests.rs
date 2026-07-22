use super::*;

fn contains_message_append(repository: &TaskRepository, task_id: &TaskId, expected: &str) -> bool {
    repository.replay(task_id).unwrap().into_iter().any(|record| {
        let kuku::event::EventPayload::TaskLedger(kuku::event::TaskLedgerRecord::Activity(batch)) =
            record.payload
        else {
            return false;
        };
        batch.events().iter().any(|event| {
            matches!(event, TaskEvent::MessagePatched { append_text, .. } if append_text == expected)
        })
    })
}

async fn assert_runtime_rejects_after_persistence_failure(
    runtime: &TaskRuntime,
    task: &crate::api::CreateTaskResponse,
    expected: DomainError,
) {
    for _ in 0..100 {
        if runtime.available_run_permits_for_test() == (1, 1) {
            let error = runtime
                .submit(submit(
                    task.projection.task.task_id.clone(),
                    task.projection.task_revision,
                    "after-persistence-failure",
                ))
                .await
                .unwrap_err();
            assert_eq!(error, expected);
            tokio::time::timeout(Duration::from_millis(100), runtime.shutdown())
                .await
                .expect("shutdown must not wait on failed persistence");
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "persistence failure retained run permits: {:?}",
        runtime.available_run_permits_for_test()
    );
}

#[tokio::test]
async fn unrecoverable_run_start_persistence_failure_is_fatal_and_releases_permits() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake, 1, 0).unwrap();
    let failing = runtime
        .create_task(create("fatal-start-create"))
        .await
        .unwrap();
    let next = runtime
        .create_task(create("fatal-start-next-create"))
        .await
        .unwrap();
    let (arrived, release) = runtime.pause_next_launch();
    runtime
        .submit(submit(
            failing.projection.task.task_id.clone(),
            failing.projection.task_revision,
            "fatal-start-submit",
        ))
        .await
        .unwrap();
    arrived.wait().await;
    std::fs::remove_dir_all(
        runtime
            .repository()
            .task_path(&failing.projection.task.task_id),
    )
    .unwrap();
    release.wait().await;

    assert_runtime_rejects_after_persistence_failure(&runtime, &next, DomainError::TaskNotFound)
        .await;
}

#[tokio::test]
async fn terminal_persistence_retry_exhaustion_is_fatal_and_releases_permits() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake.clone(), 1, 0).unwrap();
    let failing = runtime
        .create_task(create("fatal-terminal-create"))
        .await
        .unwrap();
    let next = runtime
        .create_task(create("fatal-terminal-next-create"))
        .await
        .unwrap();
    let task_id = failing.projection.task.task_id.clone();
    let accepted = runtime
        .submit(submit(
            task_id.clone(),
            failing.projection.task_revision,
            "fatal-terminal-submit",
        ))
        .await
        .unwrap();
    wait_for_state(&runtime, &task_id, RunState::Running).await;
    runtime.repository().fail_appends_for_test(100);
    fake.complete(&accepted.run_id, "done").await;

    assert_runtime_rejects_after_persistence_failure(&runtime, &next, DomainError::LedgerCorrupt)
        .await;
}

#[tokio::test]
async fn shutdown_releases_queued_permits_after_fatal_persistence_failure() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake.clone(), 1, 1).unwrap();
    let running = runtime
        .create_task(create("fatal-shutdown-running-create"))
        .await
        .unwrap();
    let queued = runtime
        .create_task(create("fatal-shutdown-queued-create"))
        .await
        .unwrap();
    let next = runtime
        .create_task(create("fatal-shutdown-next-create"))
        .await
        .unwrap();
    let running_task_id = running.projection.task.task_id.clone();
    let accepted = runtime
        .submit(submit(
            running_task_id.clone(),
            running.projection.task_revision,
            "fatal-shutdown-running-submit",
        ))
        .await
        .unwrap();
    wait_for_state(&runtime, &running_task_id, RunState::Running).await;
    runtime
        .submit(submit(
            queued.projection.task.task_id,
            queued.projection.task_revision,
            "fatal-shutdown-queued-submit",
        ))
        .await
        .unwrap();
    runtime.repository().fail_appends_for_test(100);
    fake.complete(&accepted.run_id, "done").await;

    tokio::time::timeout(Duration::from_secs(2), runtime.shutdown())
        .await
        .expect("shutdown must not wait indefinitely on failed persistence");
    assert_eq!(runtime.available_run_permits_for_test(), (1, 2));
    assert_eq!(
        runtime
            .submit(submit(
                next.projection.task.task_id,
                next.projection.task_revision,
                "fatal-shutdown-next-submit",
            ))
            .await
            .unwrap_err(),
        DomainError::LedgerCorrupt
    );
}

#[tokio::test]
async fn healthy_stop_skips_queued_activity_and_reaches_terminal_completion() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake.clone(), 1, 0).unwrap();
    let created = runtime
        .create_task(create("healthy-stop-create"))
        .await
        .unwrap();
    let task_id = created.projection.task.task_id.clone();
    let accepted = runtime
        .submit(submit(
            task_id.clone(),
            created.projection.task_revision,
            "healthy-stop-submit",
        ))
        .await
        .unwrap();
    wait_for_state(&runtime, &task_id, RunState::Running).await;
    runtime
        .stop(StopRunCommand {
            task_id: task_id.clone(),
            expected_task_revision: accepted.task_revision,
            idempotency_key: "healthy-stop".to_owned(),
        })
        .await
        .unwrap();
    fake.send(
        &accepted.run_id,
        DriverEvent::Activity(vec![TaskEvent::MessagePatched {
            message_id: format!("msg_agent_{}", accepted.run_id.as_str()),
            append_text: "late queued activity".to_owned(),
            finalized: false,
            request_ids: None,
        }]),
    )
    .await;
    fake.complete(&accepted.run_id, "done").await;

    wait_for_state(&runtime, &task_id, RunState::Stopped).await;
    assert!(!contains_message_append(
        runtime.repository(),
        &task_id,
        "late queued activity"
    ));
}

#[tokio::test]
async fn fatal_persistence_stops_other_active_drivers_and_releases_their_permits() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake.clone(), 2, 0).unwrap();
    let first = runtime
        .create_task(create("fatal-active-first-create"))
        .await
        .unwrap();
    let second = runtime
        .create_task(create("fatal-active-second-create"))
        .await
        .unwrap();
    let first_task_id = first.projection.task.task_id.clone();
    let second_task_id = second.projection.task.task_id.clone();
    let first_run = runtime
        .submit(submit(
            first_task_id.clone(),
            first.projection.task_revision,
            "fatal-active-first-submit",
        ))
        .await
        .unwrap();
    let second_run = runtime
        .submit(submit(
            second_task_id.clone(),
            second.projection.task_revision,
            "fatal-active-second-submit",
        ))
        .await
        .unwrap();
    wait_for_state(&runtime, &first_task_id, RunState::Running).await;
    wait_for_state(&runtime, &second_task_id, RunState::Running).await;
    let mut cancellation = fake.cancellation(&second_run.run_id).await;
    runtime.repository().fail_appends_for_test(100);
    fake.complete(&first_run.run_id, "done").await;

    tokio::time::timeout(Duration::from_secs(2), cancellation.changed())
        .await
        .expect("fatal persistence must cancel other active drivers")
        .unwrap();
    assert!(*cancellation.borrow());
    fake.send(&second_run.run_id, DriverEvent::Stopped).await;
    for _ in 0..100 {
        if runtime.available_run_permits_for_test() == (2, 2) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "fatal active drivers retained permits: {:?}",
        runtime.available_run_permits_for_test()
    );
}

#[tokio::test]
async fn fatal_persistence_cancels_an_active_driver_when_its_command_channel_is_full() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake.clone(), 2, 0).unwrap();
    let first = runtime
        .create_task(create("fatal-full-first-create"))
        .await
        .unwrap();
    let second = runtime
        .create_task(create("fatal-full-second-create"))
        .await
        .unwrap();
    let first_task_id = first.projection.task.task_id.clone();
    let second_task_id = second.projection.task.task_id.clone();
    let first_run = runtime
        .submit(submit(
            first_task_id.clone(),
            first.projection.task_revision,
            "fatal-full-first-submit",
        ))
        .await
        .unwrap();
    let second_run = runtime
        .submit(submit(
            second_task_id.clone(),
            second.projection.task_revision,
            "fatal-full-second-submit",
        ))
        .await
        .unwrap();
    wait_for_state(&runtime, &first_task_id, RunState::Running).await;
    wait_for_state(&runtime, &second_task_id, RunState::Running).await;
    let mut cancellation = fake.cancellation(&second_run.run_id).await;
    fake.fill_commands(&second_run.run_id).await;
    runtime.repository().fail_appends_for_test(100);
    fake.complete(&first_run.run_id, "done").await;

    tokio::time::timeout(Duration::from_secs(2), cancellation.changed())
        .await
        .expect("fatal persistence must cancel a driver with a full command channel")
        .unwrap();
    assert!(*cancellation.borrow());
    fake.send(&second_run.run_id, DriverEvent::Stopped).await;
    for _ in 0..100 {
        if runtime.available_run_permits_for_test() == (2, 2) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "fatal cancellation retained permits: {:?}",
        runtime.available_run_permits_for_test()
    );
}
