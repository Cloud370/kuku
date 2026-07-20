use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::tempdir;
use tokio::sync::mpsc;

use kuku::event::{
    ConversationId, ExecutionScope, InteractionChoiceFact, InteractionFact, InteractionId,
    ProviderFact, ProviderFailureKind, RequestCause, RequestId, RequestScope, RequestStarted,
    RunFact, RunId, RunState, TaskEvent, TaskId, TaskRevision, TurnId, WorkspaceId,
};

use super::driver::{
    drain_text_chunks, permission_choice, DriverCommand, DriverEvent, DriverHandle, DriverStart,
    RunDriverFactory, RunFailure, RunResult,
};
use super::store::{CreateTaskCommand, TaskCommandService};
use super::submission::SubmitRunCommand;
use super::{DomainError, StopRunCommand, TaskRepository, TaskRuntime};

#[derive(Clone, Default)]
struct FakeDriverFactory {
    started: Arc<Mutex<HashSet<RunId>>>,
    commands: Arc<Mutex<HashMap<RunId, mpsc::Receiver<DriverCommand>>>>,
    events: Arc<Mutex<HashMap<RunId, mpsc::Sender<DriverEvent>>>>,
}

impl FakeDriverFactory {
    async fn complete(&self, run_id: &RunId, summary: &str) {
        let sender = self.events.lock().unwrap().get(run_id).cloned().unwrap();
        sender
            .send(DriverEvent::Completed(RunResult {
                summary: summary.to_owned(),
                checks: None,
                metrics: None,
                workspace_changes: None,
            }))
            .await
            .unwrap();
    }

    fn was_started(&self, run_id: &RunId) -> bool {
        self.started.lock().unwrap().contains(run_id)
    }

    async fn send(&self, run_id: &RunId, event: DriverEvent) {
        let sender = self.events.lock().unwrap().get(run_id).cloned().unwrap();
        sender.send(event).await.unwrap();
    }

    async fn commands(&self, run_id: &RunId) -> mpsc::Receiver<DriverCommand> {
        for _ in 0..100 {
            if let Some(commands) = self.commands.lock().unwrap().remove(run_id) {
                return commands;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("driver command channel was not installed")
    }

    fn close_events(&self, run_id: &RunId) {
        self.events.lock().unwrap().remove(run_id);
    }
}

impl RunDriverFactory for FakeDriverFactory {
    fn start(
        &self,
        start: DriverStart,
    ) -> Pin<Box<dyn Future<Output = Result<DriverHandle, DomainError>> + Send>> {
        let fake = self.clone();
        Box::pin(async move {
            let (commands, command_rx) = mpsc::channel(8);
            let (event_tx, events) = mpsc::channel(32);
            fake.started.lock().unwrap().insert(start.run_id.clone());
            fake.commands
                .lock()
                .unwrap()
                .insert(start.run_id.clone(), command_rx);
            fake.events
                .lock()
                .unwrap()
                .insert(start.run_id, event_tx.clone());
            event_tx.send(DriverEvent::Started).await.unwrap();
            Ok(DriverHandle { commands, events })
        })
    }
}

fn workspace_id() -> WorkspaceId {
    WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap()
}

fn create(key: &str) -> CreateTaskCommand {
    CreateTaskCommand {
        workspace_id: workspace_id(),
        idempotency_key: key.to_owned(),
    }
}

fn submit(task_id: TaskId, revision: TaskRevision, key: &str) -> SubmitRunCommand {
    SubmitRunCommand {
        task_id,
        expected_task_revision: revision,
        idempotency_key: key.to_owned(),
        message: "Run the task".to_owned(),
        tier_id: "tier:default".to_owned(),
        skill_ids: Vec::new(),
    }
}

async fn wait_for_state(runtime: &TaskRuntime, task_id: &TaskId, expected: RunState) {
    let mut actual = None;
    for _ in 0..100 {
        let projection = runtime.projection(task_id).await.unwrap();
        actual = projection
            .active_run
            .as_ref()
            .or(projection.latest_run.as_ref())
            .map(|run| run.state);
        if actual == Some(expected) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run did not reach {expected:?}; last state was {actual:?}");
}

#[tokio::test]
async fn dropping_submission_owner_does_not_stop_the_driver() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake.clone(), 1, 64).unwrap();
    let created = runtime.create_task(create("browser-create")).await.unwrap();
    let task_id = created.projection.task.task_id.clone();
    let browser = runtime.clone();
    let accepted = browser
        .submit(submit(
            task_id.clone(),
            created.projection.task_revision,
            "browser-submit",
        ))
        .await
        .unwrap();
    drop(browser);

    wait_for_state(&runtime, &task_id, RunState::Running).await;
    fake.complete(&accepted.run_id, "done").await;
    wait_for_state(&runtime, &task_id, RunState::Completed).await;
}

#[tokio::test]
async fn capacity_one_queues_a_second_task_and_stop_cancels_it_before_start() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake.clone(), 1, 64).unwrap();
    let first = runtime
        .create_task(create("capacity-first-create"))
        .await
        .unwrap();
    let second = runtime
        .create_task(create("capacity-second-create"))
        .await
        .unwrap();
    let first_task = first.projection.task.task_id.clone();
    let second_task = second.projection.task.task_id.clone();
    let running = runtime
        .submit(submit(
            first_task.clone(),
            first.projection.task_revision,
            "capacity-first-submit",
        ))
        .await
        .unwrap();
    wait_for_state(&runtime, &first_task, RunState::Running).await;
    let queued = runtime
        .submit(submit(
            second_task.clone(),
            second.projection.task_revision,
            "capacity-second-submit",
        ))
        .await
        .unwrap();
    wait_for_state(&runtime, &second_task, RunState::Queued).await;

    runtime
        .stop(StopRunCommand {
            task_id: second_task.clone(),
            expected_task_revision: queued.task_revision,
            idempotency_key: "capacity-second-stop".to_owned(),
        })
        .await
        .unwrap();

    wait_for_state(&runtime, &second_task, RunState::Stopped).await;
    assert!(!fake.was_started(&queued.run_id));
    fake.complete(&running.run_id, "done").await;
    wait_for_state(&runtime, &first_task, RunState::Completed).await;
}

#[tokio::test]
async fn queued_stop_after_dispatch_pop_never_constructs_driver() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake.clone(), 1, 64).unwrap();
    let first = runtime
        .create_task(create("wake-first-create"))
        .await
        .unwrap();
    let second = runtime
        .create_task(create("wake-second-create"))
        .await
        .unwrap();
    let running = runtime
        .submit(submit(
            first.projection.task.task_id.clone(),
            first.projection.task_revision,
            "wake-first-submit",
        ))
        .await
        .unwrap();
    wait_for_state(&runtime, &first.projection.task.task_id, RunState::Running).await;
    let (arrived, release) = runtime.pause_next_launch();
    let queued = runtime
        .submit(submit(
            second.projection.task.task_id.clone(),
            second.projection.task_revision,
            "wake-second-submit",
        ))
        .await
        .unwrap();
    fake.complete(&running.run_id, "done").await;
    arrived.wait().await;

    runtime
        .stop(StopRunCommand {
            task_id: second.projection.task.task_id.clone(),
            expected_task_revision: queued.task_revision,
            idempotency_key: "wake-second-stop".to_owned(),
        })
        .await
        .unwrap();
    release.wait().await;

    wait_for_state(&runtime, &second.projection.task.task_id, RunState::Stopped).await;
    assert!(!fake.was_started(&queued.run_id));
}

#[tokio::test]
async fn durable_queue_starts_runs_in_fifo_order() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake.clone(), 1, 64).unwrap();
    let mut tasks = Vec::new();
    for index in 0..3 {
        tasks.push(
            runtime
                .create_task(create(&format!("fifo-create-{index}")))
                .await
                .unwrap(),
        );
    }
    let mut runs = Vec::new();
    for (index, task) in tasks.iter().enumerate() {
        runs.push(
            runtime
                .submit(submit(
                    task.projection.task.task_id.clone(),
                    task.projection.task_revision,
                    &format!("fifo-submit-{index}"),
                ))
                .await
                .unwrap(),
        );
    }
    wait_for_state(
        &runtime,
        &tasks[0].projection.task.task_id,
        RunState::Running,
    )
    .await;
    fake.complete(&runs[0].run_id, "first").await;

    wait_for_state(
        &runtime,
        &tasks[1].projection.task.task_id,
        RunState::Running,
    )
    .await;
    assert!(!fake.was_started(&runs[2].run_id));
    fake.complete(&runs[1].run_id, "second").await;
    wait_for_state(
        &runtime,
        &tasks[2].projection.task.task_id,
        RunState::Running,
    )
    .await;
    fake.complete(&runs[2].run_id, "third").await;
}

#[tokio::test]
async fn accepted_stop_wins_over_late_driver_failure() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake.clone(), 1, 64).unwrap();
    let created = runtime
        .create_task(create("stop-failure-create"))
        .await
        .unwrap();
    let task_id = created.projection.task.task_id.clone();
    let accepted = runtime
        .submit(submit(
            task_id.clone(),
            created.projection.task_revision,
            "stop-failure-submit",
        ))
        .await
        .unwrap();
    wait_for_state(&runtime, &task_id, RunState::Running).await;
    let mut commands = fake.commands(&accepted.run_id).await;
    runtime
        .stop(StopRunCommand {
            task_id: task_id.clone(),
            expected_task_revision: accepted.task_revision,
            idempotency_key: "stop-failure".to_owned(),
        })
        .await
        .unwrap();
    assert!(matches!(commands.recv().await, Some(DriverCommand::Stop)));

    fake.send(
        &accepted.run_id,
        DriverEvent::Failed(RunFailure {
            summary: "late failure".to_owned(),
        }),
    )
    .await;

    wait_for_state(&runtime, &task_id, RunState::Stopped).await;
}

#[tokio::test]
async fn restart_interrupts_run_and_open_work_in_one_idempotent_batch() {
    let dir = tempdir().unwrap();
    let repository = TaskRepository::open(dir.path()).unwrap();
    let setup = TaskCommandService::new_unchecked(repository.clone());
    let created = setup.create_task(create("restart-create")).await.unwrap();
    let task_id = created.projection.task.task_id.clone();
    let accepted = setup
        .submit(submit(
            task_id.clone(),
            created.projection.task_revision,
            "restart-submit",
        ))
        .await
        .unwrap();
    let run_id = accepted.run_id.clone();
    let interaction_id = InteractionId::parse("int_0123456789abcdef01234567").unwrap();
    let request_id = RequestId::parse("req_0123456789abcdef01234567").unwrap();
    let execution = ExecutionScope {
        workspace_id: workspace_id(),
        task_id: task_id.clone(),
        run_id: run_id.clone(),
        turn_id: TurnId::parse("trn_0123456789abcdef01234567").unwrap(),
        conversation_id: ConversationId::parse("con_0123456789abcdef01234567").unwrap(),
        turn_index: 1,
    };
    setup
        .append_activity(
            &task_id,
            vec![
                TaskEvent::RunStarted {
                    run: active_run(&task_id, &run_id, RunState::Running),
                },
                TaskEvent::InteractionOpened {
                    interaction: InteractionFact {
                        interaction_id: interaction_id.clone(),
                        run_id: run_id.clone(),
                        prompt: "Continue?".to_owned(),
                        choices: vec![InteractionChoiceFact {
                            choice_id: "continue".to_owned(),
                            label: "Continue".to_owned(),
                        }],
                        selected_choice_id: None,
                    },
                },
                TaskEvent::RequestStarted(RequestStarted {
                    scope: RequestScope {
                        execution,
                        request_id: request_id.clone(),
                    },
                    cause: RequestCause::UserSubmission,
                    provider: ProviderFact::OpenAiResponses,
                    model: "fixture-model".to_owned(),
                    started_at: "2026-07-20T00:00:00Z".to_owned(),
                }),
            ],
        )
        .await
        .unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::from_repository_unchecked(repository.clone(), fake, 1, 64).unwrap();

    runtime.recover_after_restart().await.unwrap();

    let projection = runtime.projection(&task_id).await.unwrap();
    assert_eq!(projection.task.state, kuku::event::TaskState::Interrupted);
    let records = repository.replay(&task_id).unwrap();
    let last = records.last().unwrap();
    let kuku::event::EventPayload::TaskLedger(kuku::event::TaskLedgerRecord::Activity(batch)) =
        &last.payload
    else {
        panic!("restart recovery must append one activity batch")
    };
    assert!(batch.events().iter().any(|event| matches!(
        event,
        TaskEvent::RunInterrupted { run }
            if run.run_id == run_id && run.summary.as_deref() == Some("server_restarted")
    )));
    assert!(batch.events().iter().any(|event| matches!(
        event,
        TaskEvent::InteractionCancelled { interaction_id: cancelled }
            if cancelled == &interaction_id
    )));
    assert!(batch.events().iter().any(|event| matches!(
        event,
        TaskEvent::RequestFailed(failed)
            if failed.scope.request_id == request_id
                && failed.failure.kind == ProviderFailureKind::ServerRestarted
                && failed.elapsed_ms.is_none()
                && failed.usage.is_none()
                && failed.provider_request_id.is_none()
                && failed.cost.is_none()
    )));
    let record_count = records.len();
    runtime.recover_after_restart().await.unwrap();
    assert_eq!(repository.replay(&task_id).unwrap().len(), record_count);
    assert_eq!(
        request_terminal_count(&repository, &task_id, &request_id),
        1
    );

    let next = runtime
        .submit(submit(
            task_id,
            projection.task_revision,
            "restart-next-submit",
        ))
        .await
        .unwrap();
    assert_ne!(next.run_id, run_id);
}

#[tokio::test]
async fn queue_overflow_is_server_busy_without_durable_acceptance() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake, 1, 64).unwrap();
    let mut created = Vec::new();
    for index in 0..66 {
        created.push(
            runtime
                .create_task(create(&format!("overflow-create-{index}")))
                .await
                .unwrap(),
        );
    }
    for (index, task) in created.iter().take(65).enumerate() {
        runtime
            .submit(submit(
                task.projection.task.task_id.clone(),
                task.projection.task_revision,
                &format!("overflow-submit-{index}"),
            ))
            .await
            .unwrap();
    }
    let rejected = &created[65];
    let task_id = rejected.projection.task.task_id.clone();
    let error = runtime
        .submit(submit(
            task_id.clone(),
            rejected.projection.task_revision,
            "overflow-submit-rejected",
        ))
        .await
        .unwrap_err();

    assert_eq!(error, DomainError::ServerBusy);
    let projection = runtime.projection(&task_id).await.unwrap();
    assert!(projection.latest_run.is_none());
    assert_eq!(projection.task_revision, rejected.projection.task_revision);
}

#[tokio::test]
async fn stop_and_resolution_are_durable_before_driver_commands() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake.clone(), 2, 64).unwrap();

    let stopped_task = runtime
        .create_task(create("ordered-stop-create"))
        .await
        .unwrap();
    let stopped_task_id = stopped_task.projection.task.task_id.clone();
    let stopped = runtime
        .submit(submit(
            stopped_task_id.clone(),
            stopped_task.projection.task_revision,
            "ordered-stop-submit",
        ))
        .await
        .unwrap();
    assert_ne!(stopped_task_id.as_str(), stopped.run_id.as_str());
    wait_for_state(&runtime, &stopped_task_id, RunState::Running).await;
    let mut stop_commands = fake.commands(&stopped.run_id).await;
    let stop_command = StopRunCommand {
        task_id: stopped_task_id.clone(),
        expected_task_revision: stopped.task_revision,
        idempotency_key: "ordered-stop".to_owned(),
    };
    let stop_accepted = runtime.stop(stop_command.clone()).await.unwrap();
    assert!(matches!(
        stop_commands.recv().await,
        Some(DriverCommand::Stop)
    ));
    assert!(last_control_contains(
        &runtime,
        &stopped_task_id,
        |event| matches!(
            event,
            TaskEvent::RunStopping { run } if run.run_id == stopped.run_id
        )
    ));
    runtime.repository().fail_next_append_for_test();
    fake.send(&stopped.run_id, DriverEvent::Stopped).await;
    wait_for_state(&runtime, &stopped_task_id, RunState::Stopped).await;
    let next = runtime
        .submit(submit(
            stopped_task_id.clone(),
            stop_accepted.task_revision,
            "ordered-next-submit",
        ))
        .await
        .unwrap();
    wait_for_state(&runtime, &stopped_task_id, RunState::Running).await;
    assert!(runtime.stop(stop_command).await.unwrap().replayed);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        runtime
            .projection(&stopped_task_id)
            .await
            .unwrap()
            .active_run
            .unwrap()
            .run_id,
        next.run_id
    );
    fake.complete(&next.run_id, "next done").await;

    let resolved_task = runtime
        .create_task(create("ordered-resolve-create"))
        .await
        .unwrap();
    let resolved_task_id = resolved_task.projection.task.task_id.clone();
    let resolved = runtime
        .submit(submit(
            resolved_task_id.clone(),
            resolved_task.projection.task_revision,
            "ordered-resolve-submit",
        ))
        .await
        .unwrap();
    wait_for_state(&runtime, &resolved_task_id, RunState::Running).await;
    let interaction_id = InteractionId::try_new().unwrap();
    fake.send(
        &resolved.run_id,
        DriverEvent::InteractionOpened(InteractionFact {
            interaction_id: interaction_id.clone(),
            run_id: resolved.run_id.clone(),
            prompt: "Continue?".to_owned(),
            choices: vec![InteractionChoiceFact {
                choice_id: "continue".to_owned(),
                label: "Continue".to_owned(),
            }],
            selected_choice_id: None,
        }),
    )
    .await;
    wait_for_state(&runtime, &resolved_task_id, RunState::NeedsAttention).await;
    let mut resolve_commands = fake.commands(&resolved.run_id).await;
    let resolve_command = super::ResolveInteractionCommand {
        task_id: resolved_task_id.clone(),
        interaction_id: interaction_id.clone(),
        choice_id: "continue".to_owned(),
        expected_task_revision: resolved.task_revision,
        idempotency_key: "ordered-resolve".to_owned(),
    };
    runtime
        .resolve_interaction(resolve_command.clone())
        .await
        .unwrap();
    assert!(matches!(
        resolve_commands.recv().await,
        Some(DriverCommand::Resolve {
            interaction_id: received,
            choice_id,
        }) if received == interaction_id && choice_id == "continue"
    ));
    assert!(last_control_contains(
        &runtime,
        &resolved_task_id,
        |event| matches!(
            event,
            TaskEvent::InteractionResolved { interaction_id: received, choice_id }
                if received == &interaction_id && choice_id == "continue"
        )
    ));
    fake.complete(&resolved.run_id, "done").await;
    wait_for_state(&runtime, &resolved_task_id, RunState::Completed).await;
    assert!(
        runtime
            .resolve_interaction(resolve_command)
            .await
            .unwrap()
            .replayed
    );
}

#[tokio::test]
async fn stop_racing_completion_never_leaves_stopping() {
    let dir = tempdir().unwrap();
    let fake = Arc::new(FakeDriverFactory::default());
    let runtime = TaskRuntime::open_unchecked(dir.path(), fake.clone(), 1, 64).unwrap();
    let created = runtime
        .create_task(create("stop-race-create"))
        .await
        .unwrap();
    let task_id = created.projection.task.task_id.clone();
    let accepted = runtime
        .submit(submit(
            task_id.clone(),
            created.projection.task_revision,
            "stop-race-submit",
        ))
        .await
        .unwrap();
    wait_for_state(&runtime, &task_id, RunState::Running).await;
    let stop = StopRunCommand {
        task_id: task_id.clone(),
        expected_task_revision: accepted.task_revision,
        idempotency_key: "stop-race".to_owned(),
    };

    let (stop_result, ()) =
        tokio::join!(runtime.stop(stop), fake.complete(&accepted.run_id, "done"));

    for _ in 0..100 {
        let state = runtime
            .projection(&task_id)
            .await
            .unwrap()
            .latest_run
            .unwrap()
            .state;
        if matches!(state, RunState::Completed | RunState::Stopped) {
            if stop_result.is_ok() {
                assert_eq!(state, RunState::Stopped);
            }
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("stop/completion race did not settle to a terminal state")
}

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
}

fn last_control_contains(
    runtime: &TaskRuntime,
    task_id: &TaskId,
    predicate: impl Fn(&TaskEvent) -> bool,
) -> bool {
    let records = runtime.repository().replay(task_id).unwrap();
    let Some(last) = records.last() else {
        return false;
    };
    let kuku::event::EventPayload::TaskLedger(kuku::event::TaskLedgerRecord::Control(transaction)) =
        &last.payload
    else {
        return false;
    };
    transaction.events().iter().any(predicate)
}

fn active_run(task_id: &TaskId, run_id: &RunId, state: RunState) -> RunFact {
    RunFact {
        run_id: run_id.clone(),
        task_id: task_id.clone(),
        state,
        started_at: "2026-07-20T00:00:00Z".to_owned(),
        finished_at: None,
        summary: None,
        checks: None,
        metrics: None,
        workspace_changes: None,
    }
}

fn request_terminal_count(
    repository: &TaskRepository,
    task_id: &TaskId,
    request_id: &RequestId,
) -> usize {
    repository
        .replay(task_id)
        .unwrap()
        .into_iter()
        .filter_map(|record| match record.payload {
            kuku::event::EventPayload::TaskLedger(kuku::event::TaskLedgerRecord::Activity(
                batch,
            )) => Some(batch),
            _ => None,
        })
        .flat_map(|batch| batch.events().to_vec())
        .filter(|event| match event {
            TaskEvent::RequestCompleted(completed) => completed.scope.request_id == *request_id,
            TaskEvent::RequestFailed(failed) => failed.scope.request_id == *request_id,
            _ => false,
        })
        .count()
}
