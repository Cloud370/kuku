use kuku::event::{
    CommandIntent, CommandReceipt, CommandResult, EventPayload, InteractionChoiceFact,
    InteractionFact, RunFact, RunState, TaskActivityBatch, TaskEvent, TaskId, TaskLedgerRecord,
    TaskRevision, TaskTransaction, WorkspaceId,
};

fn task_id() -> TaskId {
    TaskId::parse("tsk_0123456789abcdef01234567").unwrap()
}

fn workspace_id() -> WorkspaceId {
    WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap()
}

fn run() -> RunFact {
    RunFact {
        run_id: "run_0123456789abcdef01234567".parse().unwrap(),
        task_id: task_id(),
        state: RunState::Queued,
        started_at: "2026-07-20T00:00:00Z".to_owned(),
        finished_at: None,
        summary: None,
        checks: None,
        metrics: None,
        workspace_changes: None,
    }
}

fn receipt() -> CommandReceipt {
    CommandReceipt::new(
        "create-1",
        "digest-1",
        CommandResult::TaskCreated { task_id: task_id() },
    )
    .unwrap()
}

#[test]
fn control_transaction_round_trips_and_rejects_activity_only_events() {
    let event = TaskEvent::TaskCreated {
        task_id: task_id(),
        workspace_id: workspace_id(),
        title: "A task".to_owned(),
        created_at: "2026-07-20T00:00:00Z".to_owned(),
    };
    let transaction = TaskTransaction::try_new(
        TaskRevision::try_new(0).unwrap(),
        receipt(),
        vec![event.clone()],
    )
    .unwrap();
    let record = TaskLedgerRecord::Control(transaction);
    let value = serde_json::to_value(EventPayload::TaskLedger(record.clone())).unwrap();
    let decoded: EventPayload = serde_json::from_value(value).unwrap();
    assert_eq!(decoded, EventPayload::TaskLedger(record));

    let activity = TaskEvent::RunStarted { run: run() };
    assert!(
        TaskTransaction::try_new(TaskRevision::try_new(1).unwrap(), receipt(), vec![activity],)
            .is_err()
    );
}

#[test]
fn activity_batch_accepts_runtime_facts_without_a_receipt() {
    let batch = TaskActivityBatch::try_new(vec![TaskEvent::RunStarted { run: run() }]).unwrap();
    let record = TaskLedgerRecord::Activity(batch);
    let value = serde_json::to_value(EventPayload::TaskLedger(record.clone())).unwrap();
    let decoded: EventPayload = serde_json::from_value(value).unwrap();
    assert_eq!(decoded, EventPayload::TaskLedger(record));
}

#[test]
fn command_intents_are_tagged_and_receipts_require_non_empty_keys() {
    let intent = CommandIntent::CreateTask {
        workspace_id: workspace_id(),
    };
    assert_eq!(serde_json::to_value(intent).unwrap()["kind"], "create_task");
    assert!(CommandReceipt::new("", "digest", CommandResult::Stopped).is_err());
    assert!(CommandReceipt::new("key", "", CommandResult::Stopped).is_err());
}

#[test]
fn interaction_facts_are_activity_values() {
    let interaction = InteractionFact {
        interaction_id: "int_0123456789abcdef01234567".parse().unwrap(),
        run_id: run().run_id,
        prompt: "Choose".to_owned(),
        choices: vec![
            InteractionChoiceFact {
                choice_id: "yes".to_owned(),
                label: "Yes".to_owned(),
            },
            InteractionChoiceFact {
                choice_id: "no".to_owned(),
                label: "No".to_owned(),
            },
        ],
        selected_choice_id: None,
    };
    TaskActivityBatch::try_new(vec![TaskEvent::InteractionOpened { interaction }]).unwrap();
}
