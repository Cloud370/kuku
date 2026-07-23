use kuku::event::{
    ActivityFact, ActivityKindFact, ActivityStatusFact, CommandIntent, CommandReceipt,
    CommandResult, RunFact, RunId, RunState, TaskEvent, TaskId, TaskLedgerRecord, TaskRevision,
    TaskTransaction, WorkspaceId,
};

use super::domain::{DomainError, TaskAggregate};

fn task_id() -> TaskId {
    TaskId::parse("tsk_0123456789abcdef01234567").unwrap()
}

fn workspace_id() -> WorkspaceId {
    WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap()
}

fn run_id(suffix: char) -> RunId {
    let mut value = "run_0123456789abcdef0123456".to_string();
    value.push(suffix);
    RunId::parse(value).unwrap()
}

fn run(id: RunId, state: RunState) -> RunFact {
    RunFact {
        run_id: id,
        task_id: task_id(),
        state,
        started_at: "2026-07-20T00:00:00Z".to_owned(),
        finished_at: (!state.is_active()).then(|| "2026-07-20T00:01:00Z".to_owned()),
        summary: (!state.is_active()).then(|| "finished".to_owned()),
        warnings: Vec::new(),
        checks: None,
        metrics: None,
        workspace_changes: None,
    }
}

fn control(events: Vec<TaskEvent>, revision: u64) -> TaskLedgerRecord {
    let receipt = CommandReceipt::new(
        format!("key-{revision}"),
        format!("digest-{revision}"),
        CommandResult::TaskCreated { task_id: task_id() },
    )
    .unwrap();
    TaskLedgerRecord::Control(
        TaskTransaction::try_new(TaskRevision::try_new(revision).unwrap(), receipt, events)
            .unwrap(),
    )
}

fn created() -> TaskLedgerRecord {
    let _ = CommandIntent::CreateTask {
        workspace_id: workspace_id(),
    };
    control(
        vec![TaskEvent::TaskCreated {
            task_id: task_id(),
            workspace_id: workspace_id(),
            title: "A task".to_owned(),
            created_at: "2026-07-20T00:00:00Z".to_owned(),
        }],
        0,
    )
}

fn activity(activity_id: &str, title: &str) -> TaskEvent {
    TaskEvent::ActivityUpserted {
        activity: ActivityFact {
            activity_id: activity_id.to_owned(),
            run_id: run_id('a'),
            title: title.to_owned(),
            kind: ActivityKindFact::System,
            status: ActivityStatusFact::Completed,
            detail: None,
            conversation_id: None,
            agent: None,
            tier: None,
            result_in_main: None,
            file_references: Vec::new(),
        },
    }
}

#[test]
fn activity_upsert_preserves_uniqueness_and_moves_updates_to_the_end() {
    let mut task = TaskAggregate::default();
    task.apply_record(kuku::event::Cursor::try_new(1).unwrap(), &created())
        .unwrap();
    task.apply_record(
        kuku::event::Cursor::try_new(2).unwrap(),
        &TaskLedgerRecord::Activity(
            kuku::event::TaskActivityBatch::try_new(vec![
                activity("activity-a", "Initial"),
                activity("activity-b", "Second"),
            ])
            .unwrap(),
        ),
    )
    .unwrap();
    task.apply_record(
        kuku::event::Cursor::try_new(3).unwrap(),
        &TaskLedgerRecord::Activity(
            kuku::event::TaskActivityBatch::try_new(vec![activity("activity-a", "Updated")])
                .unwrap(),
        ),
    )
    .unwrap();

    let activities = task
        .timeline_items()
        .iter()
        .filter_map(|item| match item {
            crate::api::TimelineItemProjection::Activity(activity) => {
                Some((activity.activity_id.as_str(), activity.title.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        vec![("activity-b", "Second"), ("activity-a", "Updated")],
        activities
    );
}

#[test]
fn task_state_is_projected_from_runs_and_needs_attention_is_active() {
    let mut task = TaskAggregate::default();
    task.apply_record(kuku::event::Cursor::try_new(1).unwrap(), &created())
        .unwrap();
    task.apply_record(
        kuku::event::Cursor::try_new(2).unwrap(),
        &control(
            vec![TaskEvent::RunQueued {
                run: run(run_id('a'), RunState::Queued),
            }],
            1,
        ),
    )
    .unwrap();
    assert_eq!(task.summary().state, kuku::event::TaskState::Queued);
    task.apply_record(
        kuku::event::Cursor::try_new(3).unwrap(),
        &TaskLedgerRecord::Activity(
            kuku::event::TaskActivityBatch::try_new(vec![TaskEvent::RunStarted {
                run: run(run_id('a'), RunState::Running),
            }])
            .unwrap(),
        ),
    )
    .unwrap();
    task.apply_record(
        kuku::event::Cursor::try_new(4).unwrap(),
        &TaskLedgerRecord::Activity(
            kuku::event::TaskActivityBatch::try_new(vec![TaskEvent::RunNeedsAttention {
                run: run(run_id('a'), RunState::NeedsAttention),
            }])
            .unwrap(),
        ),
    )
    .unwrap();
    assert_eq!(task.summary().state, kuku::event::TaskState::NeedsAttention);
    assert!(task.summary().state.is_active());
}

#[test]
fn terminal_states_reject_every_outgoing_transition() {
    let mut task = TaskAggregate::default();
    task.apply_record(kuku::event::Cursor::try_new(1).unwrap(), &created())
        .unwrap();
    task.apply_record(
        kuku::event::Cursor::try_new(2).unwrap(),
        &control(
            vec![TaskEvent::RunQueued {
                run: run(run_id('a'), RunState::Queued),
            }],
            1,
        ),
    )
    .unwrap();
    task.apply_record(
        kuku::event::Cursor::try_new(3).unwrap(),
        &TaskLedgerRecord::Activity(
            kuku::event::TaskActivityBatch::try_new(vec![TaskEvent::RunFailed {
                run: run(run_id('a'), RunState::Failed),
            }])
            .unwrap(),
        ),
    )
    .unwrap();
    let error = task.apply_record(
        kuku::event::Cursor::try_new(4).unwrap(),
        &TaskLedgerRecord::Activity(
            kuku::event::TaskActivityBatch::try_new(vec![TaskEvent::RunStarted {
                run: run(run_id('a'), RunState::Running),
            }])
            .unwrap(),
        ),
    );
    assert!(matches!(error, Err(DomainError::InvalidTransition { .. })));
}
