use super::super::ResolveInteractionCommand;
use super::*;

#[test]
fn oversized_text_delta_is_split_on_utf8_boundaries() {
    let expected = "ab🙂".repeat(1_500);
    let mut text = expected.clone();

    let chunks = drain_text_chunks(&mut text, true);

    assert!(text.is_empty());
    assert!(chunks.iter().all(|(chunk, _)| chunk.len() <= 4 * 1024));
    assert!(chunks
        .iter()
        .all(|(chunk, _)| chunk.is_char_boundary(chunk.len())));
    assert_eq!(
        chunks
            .iter()
            .map(|(chunk, _)| chunk.as_str())
            .collect::<String>(),
        expected
    );
    assert!(chunks.last().unwrap().1);
    assert!(chunks[..chunks.len() - 1]
        .iter()
        .all(|(_, finalized)| !finalized));
}

#[test]
fn unknown_interaction_choice_is_not_coerced_to_deny() {
    assert_eq!(permission_choice("unknown"), None);
    assert_eq!(
        permission_choice("deny"),
        Some(kuku::PermissionChoice::Deny)
    );
}

#[tokio::test(start_paused = true)]
async fn sustained_small_deltas_keep_the_first_byte_deadline() {
    let mut buffer = TextBuffer::default();
    let started_at = tokio::time::Instant::now();
    assert!(buffer.push(started_at, "a").is_empty());
    let deadline = buffer.deadline().unwrap();

    tokio::time::advance(Duration::from_millis(40)).await;
    assert!(buffer.push(tokio::time::Instant::now(), "b").is_empty());
    assert_eq!(buffer.deadline(), Some(deadline));

    tokio::time::advance(Duration::from_millis(10)).await;
    assert!(buffer.is_due(tokio::time::Instant::now()));
    let chunks = buffer.flush(false);
    assert_eq!(chunks, vec![("ab".to_owned(), false)]);
}

#[tokio::test]
async fn invalid_interaction_choice_has_no_durable_or_driver_effect() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake.clone(), 1, 64).unwrap();
    let created = runtime
        .create_task(create("invalid-choice-create"))
        .await
        .unwrap();
    let task_id = created.projection.task.task_id.clone();
    let accepted = runtime
        .submit(submit(
            task_id.clone(),
            created.projection.task_revision,
            "invalid-choice-submit",
        ))
        .await
        .unwrap();
    wait_for_state(&runtime, &task_id, RunState::Running).await;
    let interaction_id = InteractionId::try_new().unwrap();
    fake.send(
        &accepted.run_id,
        DriverEvent::InteractionOpened(InteractionFact {
            interaction_id: interaction_id.clone(),
            run_id: accepted.run_id.clone(),
            prompt: "Continue?".to_owned(),
            choices: vec![InteractionChoiceFact {
                choice_id: "continue".to_owned(),
                label: "Continue".to_owned(),
            }],
            selected_choice_id: None,
        }),
    )
    .await;
    wait_for_state(&runtime, &task_id, RunState::NeedsAttention).await;
    let mut commands = fake.commands(&accepted.run_id).await;
    let before = runtime.projection(&task_id).await.unwrap();
    let record_count = runtime.repository().replay(&task_id).unwrap().len();
    let invalid = ResolveInteractionCommand {
        task_id: task_id.clone(),
        interaction_id: interaction_id.clone(),
        choice_id: "not-a-choice".to_owned(),
        expected_task_revision: accepted.task_revision,
        idempotency_key: "invalid-choice-key".to_owned(),
    };

    assert_eq!(
        runtime
            .resolve_interaction(invalid.clone())
            .await
            .unwrap_err(),
        DomainError::InvalidRequest
    );
    assert_eq!(
        runtime.resolve_interaction(invalid).await.unwrap_err(),
        DomainError::InvalidRequest
    );
    assert_eq!(
        runtime.projection(&task_id).await.unwrap().task_revision,
        before.task_revision
    );
    assert_eq!(
        runtime.repository().replay(&task_id).unwrap().len(),
        record_count
    );
    assert!(matches!(
        commands.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    runtime
        .resolve_interaction(ResolveInteractionCommand {
            task_id,
            interaction_id: interaction_id.clone(),
            choice_id: "continue".to_owned(),
            expected_task_revision: accepted.task_revision,
            idempotency_key: "invalid-choice-key".to_owned(),
        })
        .await
        .unwrap();
    assert!(matches!(
        commands.recv().await,
        Some(DriverCommand::Resolve {
            interaction_id: received,
            choice_id,
        }) if received == interaction_id && choice_id == "continue"
    ));
}

#[tokio::test]
async fn recovery_interrupts_every_unfinished_run_state() {
    let dir = tempdir().unwrap();
    let repository = TaskRepository::open(dir.path()).unwrap();
    let setup = TaskCommandService::new_unchecked(repository.clone());
    let states = [
        RunState::Queued,
        RunState::Running,
        RunState::NeedsAttention,
        RunState::Stopping,
    ];
    let mut tasks = Vec::new();
    for (index, state) in states.into_iter().enumerate() {
        let created = setup
            .create_task(create(&format!("recover-state-create-{index}")))
            .await
            .unwrap();
        let task_id = created.projection.task.task_id.clone();
        let accepted = setup
            .submit(submit(
                task_id.clone(),
                created.projection.task_revision,
                &format!("recover-state-submit-{index}"),
            ))
            .await
            .unwrap();
        if matches!(
            state,
            RunState::Running | RunState::NeedsAttention | RunState::Stopping
        ) {
            setup
                .append_activity(
                    &task_id,
                    vec![TaskEvent::RunStarted {
                        run: active_run(&task_id, &accepted.run_id, RunState::Running),
                    }],
                )
                .await
                .unwrap();
        }
        if state == RunState::NeedsAttention {
            setup
                .append_activity(
                    &task_id,
                    vec![TaskEvent::InteractionOpened {
                        interaction: InteractionFact {
                            interaction_id: InteractionId::try_new().unwrap(),
                            run_id: accepted.run_id.clone(),
                            prompt: "Continue?".to_owned(),
                            choices: vec![InteractionChoiceFact {
                                choice_id: "continue".to_owned(),
                                label: "Continue".to_owned(),
                            }],
                            selected_choice_id: None,
                        },
                    }],
                )
                .await
                .unwrap();
        }
        if state == RunState::Stopping {
            setup
                .stop(StopRunCommand {
                    task_id: task_id.clone(),
                    expected_task_revision: accepted.task_revision,
                    idempotency_key: format!("recover-state-stop-{index}"),
                })
                .await
                .unwrap();
        }
        tasks.push(task_id);
    }
    let runtime = TaskRuntime::from_repository_unchecked(
        repository,
        Arc::new(FakeDriverFactory::default()),
        1,
        64,
    )
    .unwrap();

    runtime.recover_after_restart().await.unwrap();

    for task_id in tasks {
        assert_eq!(
            runtime
                .projection(&task_id)
                .await
                .unwrap()
                .latest_run
                .unwrap()
                .state,
            RunState::Interrupted
        );
    }
}

#[tokio::test]
async fn driver_event_eof_durably_fails_the_run() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake.clone(), 1, 64).unwrap();
    let created = runtime.create_task(create("eof-create")).await.unwrap();
    let task_id = created.projection.task.task_id.clone();
    let accepted = runtime
        .submit(submit(
            task_id.clone(),
            created.projection.task_revision,
            "eof-submit",
        ))
        .await
        .unwrap();
    wait_for_state(&runtime, &task_id, RunState::Running).await;

    fake.close_events(&accepted.run_id);

    wait_for_state(&runtime, &task_id, RunState::Failed).await;
    assert_terminal_finalizes_agent(runtime.repository(), &task_id, &accepted.run_id);
}

struct FailingDriverFactory;

impl RunDriverFactory for FailingDriverFactory {
    fn start(
        &self,
        _: DriverStart,
    ) -> Pin<Box<dyn Future<Output = Result<DriverHandle, DomainError>> + Send>> {
        Box::pin(async { Err(DomainError::InvalidRequest) })
    }
}

#[tokio::test]
async fn factory_failure_finalizes_agent_in_the_terminal_batch() {
    let dir = tempdir().unwrap();
    let runtime =
        TaskRuntime::open_unchecked(dir.path(), Arc::new(FailingDriverFactory), 1, 64).unwrap();
    let created = runtime
        .create_task(create("factory-fail-create"))
        .await
        .unwrap();
    let task_id = created.projection.task.task_id.clone();
    let accepted = runtime
        .submit(submit(
            task_id.clone(),
            created.projection.task_revision,
            "factory-fail-submit",
        ))
        .await
        .unwrap();

    wait_for_state(&runtime, &task_id, RunState::Failed).await;

    assert_terminal_finalizes_agent(runtime.repository(), &task_id, &accepted.run_id);
}

#[tokio::test]
async fn terminal_batch_does_not_repeat_an_existing_agent_finalization() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake.clone(), 1, 64).unwrap();
    let created = runtime
        .create_task(create("already-final-create"))
        .await
        .unwrap();
    let task_id = created.projection.task.task_id.clone();
    let accepted = runtime
        .submit(submit(
            task_id.clone(),
            created.projection.task_revision,
            "already-final-submit",
        ))
        .await
        .unwrap();
    wait_for_state(&runtime, &task_id, RunState::Running).await;
    fake.send(
        &accepted.run_id,
        DriverEvent::Activity(vec![TaskEvent::MessagePatched {
            message_id: format!("msg_agent_{}", accepted.run_id.as_str()),
            append_text: String::new(),
            finalized: true,
            request_ids: None,
        }]),
    )
    .await;
    wait_for_agent_finalized(&runtime, &task_id).await;
    fake.complete(&accepted.run_id, "done").await;
    wait_for_state(&runtime, &task_id, RunState::Completed).await;

    let batch = terminal_batch(runtime.repository(), &task_id, &accepted.run_id);
    assert!(!batch.iter().any(|event| matches!(
        event,
        TaskEvent::MessagePatched { message_id, finalized: true, .. }
            if message_id == &format!("msg_agent_{}", accepted.run_id.as_str())
    )));
}

#[tokio::test]
async fn completed_driver_warnings_are_persisted_in_the_terminal_fact() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake.clone(), 1, 64).unwrap();
    let created = runtime.create_task(create("warning-create")).await.unwrap();
    let task_id = created.projection.task.task_id.clone();
    let accepted = runtime
        .submit(submit(
            task_id.clone(),
            created.projection.task_revision,
            "warning-submit",
        ))
        .await
        .unwrap();
    wait_for_state(&runtime, &task_id, RunState::Running).await;

    fake.complete_with_warnings(
        &accepted.run_id,
        vec!["partial result".to_owned(), "check logs".to_owned()],
    )
    .await;
    wait_for_state(&runtime, &task_id, RunState::Completed).await;

    let batch = terminal_batch(runtime.repository(), &task_id, &accepted.run_id);
    assert!(batch.iter().any(|event| matches!(
        event,
        TaskEvent::RunCompleted { run }
            if run.warnings == ["partial result", "check logs"]
    )));
}
