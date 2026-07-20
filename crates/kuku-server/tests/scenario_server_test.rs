#![cfg(feature = "test-scenarios")]

use kuku::event::{
    CommandReceipt, CommandResult, Cursor, MessageFact, MessageRoleFact, TaskActivityBatch,
    TaskEvent, TaskId, TaskLedgerRecord, TaskRevision, TaskTransaction, WorkspaceId,
};
use kuku_server::api::{TaskChange, TimelineItemProjection};
use kuku_server::run_manager::{DomainError, TaskAggregate};
use kuku_server::testing::{BarrierOutcome, DriverEvent, ScenarioControl, ScenarioDriverFactory};

fn task_id(suffix: char) -> TaskId {
    let mut value = "tsk_0123456789abcdef0123456".to_owned();
    value.push(suffix);
    TaskId::parse(value).unwrap()
}

fn workspace_id() -> WorkspaceId {
    WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap()
}

fn control_record(events: Vec<TaskEvent>, revision: u64) -> TaskLedgerRecord {
    let receipt = CommandReceipt::new(
        format!("scenario-{revision}"),
        format!("digest-{revision}"),
        CommandResult::TaskCreated {
            task_id: task_id('a'),
        },
    )
    .unwrap();
    TaskLedgerRecord::Control(
        TaskTransaction::try_new(TaskRevision::try_new(revision).unwrap(), receipt, events)
            .unwrap(),
    )
}

fn created_record() -> TaskLedgerRecord {
    control_record(
        vec![TaskEvent::TaskCreated {
            task_id: task_id('a'),
            workspace_id: workspace_id(),
            title: "Scenario task".to_owned(),
            created_at: "2026-07-20T00:00:00Z".to_owned(),
        }],
        0,
    )
}

fn message_record(task_id: TaskId, text: String) -> TaskLedgerRecord {
    control_record(
        vec![TaskEvent::MessageAppended {
            message: MessageFact {
                message_id: "message-1".to_owned(),
                task_id,
                run_id: None,
                role: MessageRoleFact::Agent,
                text,
                finalized: true,
                request_ids: Vec::new(),
                file_references: Vec::new(),
            },
        }],
        1,
    )
}

fn fixture_text() -> String {
    let mut fixture = ScenarioDriverFactory::from_fixture("core_task", 7).unwrap();
    std::iter::from_fn(|| fixture.next_event())
        .find_map(|(_, event)| match event {
            DriverEvent::ProviderResponse { text } => Some(text),
            _ => None,
        })
        .unwrap()
}

#[test]
fn scenario_fixture_is_provider_input_not_a_projection_script() {
    let factory = ScenarioDriverFactory::from_fixture("core_task", 7).unwrap();
    assert!(!factory.fixture_source().contains("TaskProjection"));
    assert!(!factory.fixture_source().contains("ledger"));
}

#[test]
fn equal_seed_produces_equal_deterministic_driver_events() {
    let mut left = ScenarioDriverFactory::from_fixture("core_task", 7).unwrap();
    let mut right = ScenarioDriverFactory::from_fixture("core_task", 7).unwrap();
    let left_events: Vec<_> = std::iter::from_fn(|| left.next_event()).collect();
    let right_events: Vec<_> = std::iter::from_fn(|| right.next_event()).collect();
    assert_eq!(left_events, right_events);
}

#[tokio::test]
async fn control_releases_only_declared_barriers() {
    let control = ScenarioControl::new(["after-tool"]);
    control.release("after-tool").await.unwrap();
    assert_eq!(
        control.wait("after-tool").await.unwrap(),
        BarrierOutcome::Released
    );
}

#[test]
fn ledger_replay_rebuilds_projection_before_newer_record() {
    let records = [
        (Cursor::try_new(1).unwrap(), created_record()),
        (
            Cursor::try_new(3).unwrap(),
            message_record(task_id('a'), fixture_text()),
        ),
    ];
    let mut before_replay = TaskAggregate::default();
    for (cursor, record) in &records {
        before_replay.apply_record(*cursor, record).unwrap();
    }
    let projection_before_replay = before_replay.projection().unwrap();

    let mut replayed = TaskAggregate::default();
    for (cursor, record) in &records {
        replayed.apply_record(*cursor, record).unwrap();
    }
    assert_eq!(replayed.projection().unwrap(), projection_before_replay);

    let next_cursor = Cursor::try_new(7).unwrap();
    let changes = replayed
        .apply_record(
            next_cursor,
            &TaskLedgerRecord::Activity(
                TaskActivityBatch::try_new(vec![TaskEvent::MessagePatched {
                    message_id: "message-1".to_owned(),
                    append_text: " Done.".to_owned(),
                    finalized: true,
                    request_ids: None,
                }])
                .unwrap(),
            ),
        )
        .unwrap();
    assert!(replayed.cursor().get() > projection_before_replay.cursor.get());
    assert_eq!(replayed.cursor(), next_cursor);
    assert!(matches!(
        changes.as_slice(),
        [TaskChange::MessagePatched { .. }]
    ));
}

#[test]
fn reducer_uses_record_cursor_for_new_timeline_items() {
    let mut aggregate = TaskAggregate::default();
    aggregate
        .apply_record(Cursor::try_new(1).unwrap(), &created_record())
        .unwrap();
    let cursor = Cursor::try_new(3).unwrap();
    let changes = aggregate
        .apply_record(cursor, &message_record(task_id('a'), fixture_text()))
        .unwrap();
    assert!(matches!(
        changes.as_slice(),
        [TaskChange::MessageAppended { item }]
            if matches!(item, TimelineItemProjection::Message(message) if message.order_key == cursor)
    ));
}

#[test]
fn stale_cursor_is_rejected_before_projection_mutation() {
    let mut aggregate = TaskAggregate::default();
    let cursor = Cursor::try_new(1).unwrap();
    aggregate.apply_record(cursor, &created_record()).unwrap();
    let before = aggregate.projection().unwrap();
    let result = aggregate.apply_record(cursor, &message_record(task_id('a'), fixture_text()));
    assert!(matches!(result, Err(DomainError::LedgerCorrupt)));
    assert_eq!(aggregate.projection().unwrap(), before);
}

#[test]
fn cross_task_record_is_rejected_before_cursor_advances() {
    let mut aggregate = TaskAggregate::default();
    aggregate
        .apply_record(Cursor::try_new(1).unwrap(), &created_record())
        .unwrap();
    let before = aggregate.projection().unwrap();
    let result = aggregate.apply_record(
        Cursor::try_new(2).unwrap(),
        &control_record(
            vec![
                TaskEvent::MessageAppended {
                    message: MessageFact {
                        message_id: "local-message".to_owned(),
                        task_id: task_id('a'),
                        run_id: None,
                        role: MessageRoleFact::Agent,
                        text: "must roll back".to_owned(),
                        finalized: true,
                        request_ids: Vec::new(),
                        file_references: Vec::new(),
                    },
                },
                TaskEvent::MessageAppended {
                    message: MessageFact {
                        message_id: "foreign-message".to_owned(),
                        task_id: task_id('b'),
                        run_id: None,
                        role: MessageRoleFact::Agent,
                        text: "foreign task".to_owned(),
                        finalized: true,
                        request_ids: Vec::new(),
                        file_references: Vec::new(),
                    },
                },
            ],
            1,
        ),
    );
    assert!(matches!(result, Err(DomainError::LedgerCorrupt)));
    assert_eq!(aggregate.projection().unwrap(), before);
}
