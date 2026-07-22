use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use kuku::event::{
    ActivityFact, ActivityKindFact, ActivityStatusFact, CommandReceipt, CommandResult,
    InteractionChoiceFact, InteractionFact, InteractionId, MessageFact, MessageRoleFact,
    ReviewAnnotationFact, ReviewSubmissionId, RunFact, RunId, RunState, SkillsChangedFact,
    TaskActivityBatch, TaskEvent, TaskId, TaskLedgerRecord, TaskRevision, TaskTransaction,
    WorkspaceId,
};
use kuku_server::api::{
    ApiError, RegisterWorkspaceRequest, TaskChange, TaskDelta, TaskProjection, TaskStreamEvent,
    TimelineItemProjection,
};
use kuku_server::platform::{
    RegistrationRootRegistry, RegistrationRootSpec, ServerRevisionCoordinator, WorkspaceRegistry,
    WorkspaceUsagePort,
};
use kuku_server::run_manager::driver::{DriverHandle, DriverStart, RunDriverFactory};
use kuku_server::run_manager::{
    projection, CreateTaskCommand, DomainError, ResolveInteractionCommand,
    ReviewSubmissionValidator, RunQueueAdmission, RunQueueReservation, SkillSelectionValidator,
    StopRunCommand, SubmitRunCommand, TaskAggregate, TaskCommandService, TaskRepository,
    TaskRuntime, ValidatedSkillSelection,
};
use kuku_server::ServerLimits;

struct NoWorkspaceUsage;

impl WorkspaceUsagePort for NoWorkspaceUsage {
    fn has_durable_tasks<'a>(
        &'a self,
        _id: &'a WorkspaceId,
    ) -> Pin<Box<dyn Future<Output = Result<bool, ApiError>> + Send + 'a>> {
        Box::pin(async { Ok(false) })
    }
}

struct AcceptSkills;

impl SkillSelectionValidator for AcceptSkills {
    fn validate(
        &self,
        _workspace_id: &WorkspaceId,
        tier_id: &str,
        skill_ids: &[String],
    ) -> Result<ValidatedSkillSelection, DomainError> {
        Ok(ValidatedSkillSelection {
            selection: SkillsChangedFact {
                tier_id: tier_id.to_owned(),
                skill_ids: skill_ids.to_vec(),
            },
            selected_skills: skill_ids
                .iter()
                .map(|skill_id| kuku::event::SkillContextFact {
                    skill_id: skill_id.clone(),
                    source: kuku::event::SourceFact {
                        scope: kuku::event::SourceScope::Project,
                        id: format!("source:{skill_id}"),
                        relative_path: None,
                    },
                    origin: kuku::event::SkillLoadOrigin::You,
                    content_hash: format!("sha256:{skill_id}"),
                })
                .collect(),
        })
    }
}

struct AcceptReviews;

impl ReviewSubmissionValidator for AcceptReviews {
    fn validate(
        &self,
        _task_id: &TaskId,
        _submission_id: &ReviewSubmissionId,
        notes: &[ReviewAnnotationFact],
    ) -> Result<Vec<ReviewAnnotationFact>, DomainError> {
        Ok(notes.to_vec())
    }
}

struct NoopQueue;
struct NoopReservation;

impl RunQueueAdmission for NoopQueue {
    fn reserve(self: Arc<Self>) -> Result<Box<dyn RunQueueReservation>, DomainError> {
        Ok(Box::new(NoopReservation))
    }

    fn ensure_admitted(&self, _task_id: &TaskId, _run_id: &RunId) -> Result<(), DomainError> {
        Ok(())
    }
}

impl RunQueueReservation for NoopReservation {
    fn commit(self: Box<Self>, _task_id: TaskId, _run_id: RunId) {}
}

struct FakeDriverFactory;

impl RunDriverFactory for FakeDriverFactory {
    fn start(
        &self,
        _start: DriverStart,
    ) -> Pin<Box<dyn Future<Output = Result<DriverHandle, DomainError>> + Send>> {
        Box::pin(async { Err(DomainError::RunNotActive) })
    }
}

#[tokio::test]
async fn a_ready_lifecycle_reconnect_revision_race_and_restart_recovery() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    std::fs::create_dir(allowed.path().join("project")).unwrap();
    let registry = open_registry(home.path(), allowed.path());
    let workspace_id = register_workspace(&registry).await;
    let repository = TaskRepository::open(home.path()).unwrap();
    let service = TaskCommandService::new(
        repository.clone(),
        registry.clone(),
        Arc::new(AcceptSkills),
        Arc::new(AcceptReviews),
        Arc::new(NoopQueue),
    );
    let mut limits = ServerLimits::with_max_concurrent_runs(1).unwrap();
    limits.max_queued_runs = 1;
    let runtime = TaskRuntime::new(
        repository.clone(),
        Arc::new(FakeDriverFactory),
        registry.clone(),
        Arc::new(AcceptSkills),
        Arc::new(AcceptReviews),
        &limits,
    )
    .unwrap();

    let created = service
        .create_task(CreateTaskCommand {
            workspace_id,
            idempotency_key: "runtime-create".to_owned(),
        })
        .await
        .unwrap();
    let task_id = created.projection.task.task_id;
    let mut disconnected = runtime.subscribe(&task_id, None).await.unwrap();
    let initial = disconnected.next().await.unwrap();
    drop(disconnected);

    let run_a = service
        .submit(submit(
            &task_id,
            created.projection.task_revision,
            "run-a",
            "Run A",
        ))
        .await
        .unwrap();
    let started_at = "2026-07-20T00:00:01Z".to_owned();
    service
        .append_activity(
            &task_id,
            vec![
                TaskEvent::RunStarted {
                    run: run_fact(&task_id, &run_a.run_id, RunState::Running, &started_at),
                },
                TaskEvent::ActivityUpserted {
                    activity: activity(&run_a.run_id, "persisted-while-disconnected"),
                },
                TaskEvent::InteractionOpened {
                    interaction: interaction(&run_a.run_id),
                },
            ],
        )
        .await
        .unwrap();

    let mut reconnected = runtime
        .subscribe(&task_id, Some(initial.cursor))
        .await
        .unwrap();
    let replacement = reconnected.next().await.unwrap();
    let TaskDelta::ProjectionReplaced { projection } = replacement.event else {
        panic!("reconnect must replace the projection");
    };
    assert_eq!(
        projection.task.state,
        kuku::event::TaskState::NeedsAttention
    );
    assert!(projection.timeline.iter().any(|item| matches!(
        item,
        kuku_server::api::TimelineItemProjection::Activity(activity)
            if activity.activity_id == "persisted-while-disconnected"
    )));
    service
        .append_activity(
            &task_id,
            vec![TaskEvent::ActivityUpserted {
                activity: activity(&run_a.run_id, "live-after-reconnect"),
            }],
        )
        .await
        .unwrap();
    let live = reconnected.next().await.unwrap();
    assert!(live.cursor > replacement.cursor);
    assert!(matches!(live.event, TaskDelta::ChangesApplied { .. }));

    let revision = service.projection(&task_id).await.unwrap().task_revision;
    let stop = service.stop(StopRunCommand {
        task_id: task_id.clone(),
        expected_task_revision: revision,
        idempotency_key: "stop-a".to_owned(),
    });
    let resolve = service.resolve_interaction(ResolveInteractionCommand {
        task_id: task_id.clone(),
        interaction_id: interaction_id(),
        choice_id: "continue".to_owned(),
        expected_task_revision: revision,
        idempotency_key: "resolve-a".to_owned(),
    });
    let (stopped, resolved) = tokio::join!(stop, resolve);
    assert_eq!(
        usize::from(stopped.is_ok()) + usize::from(resolved.is_ok()),
        1
    );
    assert!(stopped
        .as_ref()
        .err()
        .into_iter()
        .chain(resolved.as_ref().err())
        .all(|error| *error == DomainError::StaleCommand));

    let state = service.projection(&task_id).await.unwrap().task.state;
    let terminal = if state == kuku::event::TaskState::Stopping {
        RunState::Stopped
    } else {
        RunState::Completed
    };
    service
        .append_activity(
            &task_id,
            vec![terminal_event(run_fact(
                &task_id,
                &run_a.run_id,
                terminal,
                &started_at,
            ))],
        )
        .await
        .unwrap();

    let revision = service.projection(&task_id).await.unwrap().task_revision;
    let run_b = service
        .submit(submit(&task_id, revision, "run-b", "Run B"))
        .await
        .unwrap();
    assert_ne!(task_id.as_str(), run_a.run_id.as_str());
    assert_ne!(task_id.as_str(), run_b.run_id.as_str());
    assert_ne!(run_a.run_id, run_b.run_id);
    drop(reconnected);
    drop(runtime);

    let before_recovery = repository.replay(&task_id).unwrap().len();
    let replacement_runtime = TaskRuntime::new(
        repository.clone(),
        Arc::new(FakeDriverFactory),
        registry,
        Arc::new(AcceptSkills),
        Arc::new(AcceptReviews),
        &limits,
    )
    .unwrap();
    replacement_runtime.recover_after_restart().await.unwrap();
    let recovered_once = repository.replay(&task_id).unwrap().len();
    assert_eq!(recovered_once, before_recovery + 1);
    replacement_runtime.recover_after_restart().await.unwrap();
    assert_eq!(repository.replay(&task_id).unwrap().len(), recovered_once);
    let recovered = replacement_runtime.projection(&task_id).await.unwrap();
    assert_eq!(recovered.task.state, kuku::event::TaskState::Interrupted);
    assert_eq!(recovered.latest_run.unwrap().run_id, run_b.run_id);

    let run_ids = durable_run_ids(&repository, &task_id);
    assert!(run_ids.contains(&run_a.run_id));
    assert!(run_ids.contains(&run_b.run_id));

    let replayed = repository.rebuild(&task_id).unwrap().projection().unwrap();
    let mut reduced = reduce_all(&repository, &task_id);
    reduced.task.updated_at = replayed.task.updated_at.clone();
    assert_eq!(reduced, replayed);
    assert_record_deltas_match_replay(&repository, &task_id);

    let run_c = service
        .submit(submit(
            &task_id,
            recovered.task_revision,
            "run-after-restart",
            "Run after restart",
        ))
        .await
        .unwrap();
    assert_ne!(run_c.run_id, run_b.run_id);
}

#[test]
fn subscription_golden_replays_to_the_frozen_projection_shape() {
    let frames = include_str!("fixtures/runtime/subscription.ndjson")
        .lines()
        .map(|line| serde_json::from_str::<TaskStreamEvent>(line).unwrap())
        .collect::<Vec<_>>();
    let TaskDelta::ProjectionReplaced { projection } = &frames[0].event else {
        panic!("subscription must begin with a replacement");
    };
    let mut replayed = (**projection).clone();
    for frame in &frames[1..] {
        let TaskDelta::ChangesApplied { changes, .. } = &frame.event else {
            panic!("incremental frames must not replace the projection");
        };
        apply_changes(&mut replayed, changes);
        replayed.cursor = frame.cursor;
        replayed.task_revision = frame.task_revision;
    }
    assert_eq!(replayed.cursor, frames.last().unwrap().cursor);
    assert!(replayed.timeline.iter().any(|item| matches!(
        item,
        kuku_server::api::TimelineItemProjection::Message(message)
            if message.finalized && !message.request_ids.is_empty()
    )));
}

#[test]
fn timeline_windows_are_exact_at_five_hundred_and_for_one_large_batch() {
    let task_id = fixed_task_id();
    let mut aggregate = TaskAggregate::default();
    projection::reduce_record(&mut aggregate, cursor(1), &created_record(&task_id)).unwrap();
    projection::reduce_record(
        &mut aggregate,
        cursor(2),
        &control_record(1, "first-window", messages(&task_id, 0..500)),
    )
    .unwrap();
    let boundary = projection::reduce_record(
        &mut aggregate,
        cursor(3),
        &control_record(2, "boundary-window", messages(&task_id, 500..501)),
    )
    .unwrap();
    assert!(matches!(boundary, TaskDelta::ChangesApplied {
        timeline_window: Some(ref window), ..
    } if window.evicted_items.len() == 1 && window.next_cursor.is_some()));

    let prior_projection = aggregate.projection().unwrap();
    let prior_history_end = aggregate
        .timeline_items()
        .len()
        .saturating_sub(prior_projection.timeline.len());
    let prior_history = aggregate.timeline_items()[..prior_history_end].to_vec();
    let large = projection::reduce_record(
        &mut aggregate,
        cursor(4),
        &control_record(3, "large-window", messages(&task_id, 501..1101)),
    )
    .unwrap();
    let TaskDelta::ChangesApplied {
        changes,
        timeline_window: Some(window),
    } = large
    else {
        panic!("large batch must carry window metadata");
    };
    assert_eq!(changes.len(), 600);
    assert!(changes
        .iter()
        .all(|change| matches!(change, TaskChange::MessageAppended { .. })));
    let history_and_window = prior_history
        .iter()
        .chain(
            window
                .evicted_items
                .iter()
                .chain(aggregate.projection().unwrap().timeline.iter()),
        )
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(history_and_window, aggregate.timeline_items());
}

#[test]
fn latest_run_follows_submission_order_instead_of_random_id_sorting() {
    let task_id = fixed_task_id();
    let run_a = RunId::parse("run_ffffffffffffffffffffffff").unwrap();
    let run_b = RunId::parse("run_000000000000000000000001").unwrap();
    let mut aggregate = TaskAggregate::default();
    projection::reduce_record(&mut aggregate, cursor(1), &created_record(&task_id)).unwrap();
    projection::reduce_record(
        &mut aggregate,
        cursor(2),
        &control_record(
            1,
            "ordered-run-a",
            vec![TaskEvent::RunQueued {
                run: run_fact(&task_id, &run_a, RunState::Queued, "2026-07-20T00:00:01Z"),
            }],
        ),
    )
    .unwrap();
    projection::reduce_record(
        &mut aggregate,
        cursor(3),
        &TaskLedgerRecord::Activity(
            TaskActivityBatch::try_new(vec![
                TaskEvent::RunStarted {
                    run: run_fact(&task_id, &run_a, RunState::Running, "2026-07-20T00:00:01Z"),
                },
                TaskEvent::RunCompleted {
                    run: run_fact(
                        &task_id,
                        &run_a,
                        RunState::Completed,
                        "2026-07-20T00:00:01Z",
                    ),
                },
            ])
            .unwrap(),
        ),
    )
    .unwrap();
    projection::reduce_record(
        &mut aggregate,
        cursor(4),
        &control_record(
            2,
            "ordered-run-b",
            vec![TaskEvent::RunQueued {
                run: run_fact(&task_id, &run_b, RunState::Queued, "2026-07-20T00:00:02Z"),
            }],
        ),
    )
    .unwrap();

    let projection = aggregate.projection().unwrap();
    assert_eq!(projection.latest_run.unwrap().run_id, run_b);
}

fn open_registry(home: &std::path::Path, allowed: &std::path::Path) -> Arc<WorkspaceRegistry> {
    let roots = RegistrationRootRegistry::from_server_config(
        home,
        vec![RegistrationRootSpec {
            label: "Projects".to_owned(),
            path: allowed.to_owned(),
        }],
    )
    .unwrap();
    WorkspaceRegistry::open(
        home,
        roots,
        Arc::new(NoWorkspaceUsage),
        ServerRevisionCoordinator::open(home),
    )
    .unwrap()
}

async fn register_workspace(registry: &Arc<WorkspaceRegistry>) -> WorkspaceId {
    let expected_revision = registry.revision().await.unwrap();
    let root_id = registry.registration_roots().list()[0].root_id.clone();
    registry
        .register(RegisterWorkspaceRequest {
            root_id,
            relative_path: "project".to_owned(),
            label: "Runtime".to_owned(),
            expected_revision,
        })
        .await
        .unwrap()
        .workspace_id
}

fn submit(task_id: &TaskId, revision: TaskRevision, key: &str, message: &str) -> SubmitRunCommand {
    SubmitRunCommand {
        task_id: task_id.clone(),
        expected_task_revision: revision,
        idempotency_key: key.to_owned(),
        message: message.to_owned(),
        tier_id: "balanced".to_owned(),
        skill_ids: vec!["runtime-skill".to_owned()],
    }
}

fn run_fact(task_id: &TaskId, run_id: &RunId, state: RunState, started_at: &str) -> RunFact {
    RunFact {
        run_id: run_id.clone(),
        task_id: task_id.clone(),
        state,
        started_at: started_at.to_owned(),
        finished_at: (!state.is_active()).then(|| "2026-07-20T00:01:00Z".to_owned()),
        summary: (!state.is_active()).then(|| format!("{state:?}").to_lowercase()),
        warnings: Vec::new(),
        checks: None,
        metrics: None,
        workspace_changes: None,
    }
}

fn terminal_event(run: RunFact) -> TaskEvent {
    match run.state {
        RunState::Completed => TaskEvent::RunCompleted { run },
        RunState::Stopped => TaskEvent::RunStopped { run },
        _ => panic!("expected terminal run"),
    }
}

fn activity(run_id: &RunId, id: &str) -> ActivityFact {
    ActivityFact {
        activity_id: id.to_owned(),
        run_id: run_id.clone(),
        title: id.to_owned(),
        kind: ActivityKindFact::System,
        status: ActivityStatusFact::Completed,
        detail: None,
        conversation_id: None,
        agent: None,
        tier: None,
        result_in_main: None,
        file_references: Vec::new(),
    }
}

fn interaction_id() -> InteractionId {
    InteractionId::parse("int_000000000000000000000001").unwrap()
}

fn interaction(run_id: &RunId) -> InteractionFact {
    InteractionFact {
        interaction_id: interaction_id(),
        run_id: run_id.clone(),
        prompt: "Continue?".to_owned(),
        choices: vec![InteractionChoiceFact {
            choice_id: "continue".to_owned(),
            label: "Continue".to_owned(),
        }],
        selected_choice_id: None,
    }
}

fn durable_run_ids(repository: &TaskRepository, task_id: &TaskId) -> BTreeSet<RunId> {
    repository
        .replay(task_id)
        .unwrap()
        .into_iter()
        .filter_map(|stored| match stored.payload {
            kuku::event::EventPayload::TaskLedger(record) => Some(record),
            _ => None,
        })
        .flat_map(|record| match record {
            TaskLedgerRecord::Control(transaction) => transaction.events().to_vec(),
            TaskLedgerRecord::Activity(batch) => batch.events().to_vec(),
        })
        .filter_map(|event| match event {
            TaskEvent::RunQueued { run }
            | TaskEvent::RunStarted { run }
            | TaskEvent::RunNeedsAttention { run }
            | TaskEvent::RunStopping { run }
            | TaskEvent::RunCompleted { run }
            | TaskEvent::RunStopped { run }
            | TaskEvent::RunFailed { run }
            | TaskEvent::RunInterrupted { run } => Some(run.run_id),
            _ => None,
        })
        .collect()
}

fn reduce_all(repository: &TaskRepository, task_id: &TaskId) -> TaskProjection {
    let mut aggregate = TaskAggregate::default();
    for stored in repository.replay(task_id).unwrap() {
        let kuku::event::EventPayload::TaskLedger(record) = stored.payload else {
            continue;
        };
        projection::reduce_record(&mut aggregate, cursor(stored.id), &record).unwrap();
    }
    aggregate.projection().unwrap()
}

fn assert_record_deltas_match_replay(repository: &TaskRepository, task_id: &TaskId) {
    let records = repository.replay(task_id).unwrap();
    let mut aggregate = TaskAggregate::default();
    let mut wire_projection = None;
    let mut full_timeline = Vec::new();

    for stored in records {
        let kuku::event::EventPayload::TaskLedger(record) = stored.payload else {
            continue;
        };
        let delta = projection::reduce_record(&mut aggregate, cursor(stored.id), &record).unwrap();
        let expected = aggregate.projection().unwrap();
        let Some(mut prior) = wire_projection.take() else {
            full_timeline = aggregate.timeline_items().to_vec();
            wire_projection = Some(expected);
            continue;
        };
        let TaskDelta::ChangesApplied {
            changes,
            timeline_window,
        } = delta
        else {
            panic!("each durable record must emit an atomic changes frame");
        };
        apply_changes(&mut prior, &changes);
        apply_full_timeline(&mut full_timeline, &changes);
        if let Some(window) = timeline_window {
            let candidate = prior.timeline.clone();
            assert_eq!(
                window.evicted_items,
                candidate[..window.evicted_items.len()]
            );
            prior.timeline = candidate[window.evicted_items.len()..].to_vec();
            prior.timeline_next_cursor = window.next_cursor;
        }
        assert_eq!(full_timeline, aggregate.timeline_items());
        prior.cursor = cursor(stored.id);
        prior.task_revision = expected.task_revision;
        prior.task.updated_at = expected.task.updated_at.clone();
        assert_eq!(prior, expected);
        wire_projection = Some(prior);
    }
}

fn apply_full_timeline(full: &mut Vec<TimelineItemProjection>, changes: &[TaskChange]) {
    for change in changes {
        match change {
            TaskChange::MessageAppended { item } => full.push(item.clone()),
            TaskChange::ActivityUpserted { activity } => {
                upsert_timeline_item(full, TimelineItemProjection::Activity(activity.clone()))
            }
            TaskChange::InteractionUpserted { interaction } => upsert_timeline_item(
                full,
                TimelineItemProjection::Interaction(interaction.clone()),
            ),
            TaskChange::MessagePatched {
                message_id,
                append_text,
                finalized,
                request_ids,
            } => {
                let Some(TimelineItemProjection::Message(message)) = full.iter_mut().find(|item| {
                    matches!(item, TimelineItemProjection::Message(message) if message.message_id == *message_id)
                }) else {
                    panic!("patched message must exist in the full replay timeline");
                };
                message.text.push_str(append_text);
                message.finalized = *finalized;
                if let Some(request_ids) = request_ids {
                    message.request_ids = request_ids.clone();
                }
            }
            TaskChange::RunStateChanged { .. }
            | TaskChange::SkillsChanged { .. }
            | TaskChange::ContextSummaryChanged { .. }
            | TaskChange::ReviewSubmissionsChanged { .. } => {}
        }
    }
}

fn upsert_timeline_item(items: &mut Vec<TimelineItemProjection>, item: TimelineItemProjection) {
    let same_item = |existing: &TimelineItemProjection| match (&existing, &item) {
        (TimelineItemProjection::Activity(left), TimelineItemProjection::Activity(right)) => {
            left.activity_id == right.activity_id
        }
        (TimelineItemProjection::Interaction(left), TimelineItemProjection::Interaction(right)) => {
            left.interaction_id == right.interaction_id
        }
        (TimelineItemProjection::Message(left), TimelineItemProjection::Message(right)) => {
            left.message_id == right.message_id
        }
        _ => false,
    };
    if let Some(index) = items.iter().position(same_item) {
        items[index] = item;
    } else {
        items.push(item);
    }
}

fn cursor(value: u64) -> kuku::event::Cursor {
    kuku::event::Cursor::try_new(value).unwrap()
}

fn fixed_task_id() -> TaskId {
    TaskId::parse("tsk_000000000000000000000099").unwrap()
}

fn created_record(task_id: &TaskId) -> TaskLedgerRecord {
    control_record(
        0,
        "window-create",
        vec![TaskEvent::TaskCreated {
            task_id: task_id.clone(),
            workspace_id: WorkspaceId::parse("wsp_000000000000000000000099").unwrap(),
            title: "Window task".to_owned(),
            created_at: "2026-07-20T00:00:00Z".to_owned(),
        }],
    )
}

fn control_record(revision: u64, key: &str, events: Vec<TaskEvent>) -> TaskLedgerRecord {
    TaskLedgerRecord::Control(
        TaskTransaction::try_new(
            TaskRevision::try_new(revision).unwrap(),
            CommandReceipt::new(key, key, CommandResult::Stopped).unwrap(),
            events,
        )
        .unwrap(),
    )
}

fn messages(task_id: &TaskId, range: std::ops::Range<usize>) -> Vec<TaskEvent> {
    range
        .map(|index| TaskEvent::MessageAppended {
            message: MessageFact {
                message_id: format!("window-message-{index}"),
                task_id: task_id.clone(),
                run_id: None,
                role: MessageRoleFact::Agent,
                text: index.to_string(),
                finalized: true,
                request_ids: Vec::new(),
                file_references: Vec::new(),
            },
        })
        .collect()
}

fn apply_changes(projection: &mut TaskProjection, changes: &[TaskChange]) {
    for change in changes {
        match change {
            TaskChange::MessageAppended { item } => projection.timeline.push(item.clone()),
            TaskChange::MessagePatched {
                message_id,
                append_text,
                finalized,
                request_ids,
            } => {
                let message = projection.timeline.iter_mut().find_map(|item| match item {
                    kuku_server::api::TimelineItemProjection::Message(message)
                        if message.message_id == *message_id =>
                    {
                        Some(message)
                    }
                    _ => None,
                });
                let message = message.expect("patched message must already exist");
                message.text.push_str(append_text);
                message.finalized = *finalized;
                if let Some(request_ids) = request_ids {
                    message.request_ids = request_ids.clone();
                }
            }
            TaskChange::ActivityUpserted { activity } => upsert_timeline_item(
                &mut projection.timeline,
                kuku_server::api::TimelineItemProjection::Activity(activity.clone()),
            ),
            TaskChange::InteractionUpserted { interaction } => upsert_timeline_item(
                &mut projection.timeline,
                kuku_server::api::TimelineItemProjection::Interaction(interaction.clone()),
            ),
            TaskChange::RunStateChanged {
                task,
                active_run,
                latest_run,
            } => {
                projection.task = task.clone();
                projection.active_run = active_run.as_deref().cloned();
                projection.latest_run = latest_run.as_deref().cloned();
            }
            TaskChange::SkillsChanged {
                selected_tier_id,
                loaded_skills,
            } => {
                projection.selected_tier_id = selected_tier_id.clone();
                projection.loaded_skills = loaded_skills.clone();
            }
            TaskChange::ContextSummaryChanged { context_summary } => {
                projection.context_summary = context_summary.clone();
            }
            TaskChange::ReviewSubmissionsChanged { change } => {
                projection.review_summary.total_submissions = change.total_submissions;
                projection.review_summary.latest_submission_id =
                    Some(change.submission.submission_id.clone());
            }
        }
    }
}
