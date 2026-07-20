use kuku::event::{
    ChangeEntryFact, ChangeKindFact, ChangesAvailabilityFact, CommandIntent, CommandReceipt,
    CommandResult, EventPayload, FiniteMetricValue, InteractionChoiceFact, InteractionFact,
    ReviewSubmissionId, ReviewSubmissionReference, RevisionToken, RunFact, RunState,
    TaskActivityBatch, TaskEvent, TaskId, TaskLedgerRecord, TaskRecordClass, TaskRevision,
    TaskTransaction, WorkspaceChangesFact, WorkspaceId, WorkspaceRelativePath,
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
        state: RunState::Running,
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

#[test]
fn run_variant_state_and_metric_values_are_checked() {
    let mut value = run();
    value.state = RunState::Queued;
    assert!(TaskActivityBatch::try_new(vec![TaskEvent::RunStarted { run: value }]).is_err());
    assert!(FiniteMetricValue::try_new(f64::NAN).is_err());
    assert!(FiniteMetricValue::try_new(f64::INFINITY).is_err());
    let mut active_with_completion = run();
    active_with_completion.checks = Some(Vec::new());
    assert!(matches!(
        TaskActivityBatch::try_new(vec![TaskEvent::RunStarted {
            run: active_with_completion
        }]),
        Err(kuku::event::TaskLedgerError::InvalidRunCompletion)
    ));
}

#[test]
fn task_ledger_json_and_terminal_workspace_changes_are_stable() {
    let transaction = TaskTransaction::try_new(
        TaskRevision::try_new(0).unwrap(),
        receipt(),
        vec![TaskEvent::TaskCreated {
            task_id: task_id(),
            workspace_id: workspace_id(),
            title: "A task".to_owned(),
            created_at: "2026-07-20T00:00:00Z".to_owned(),
        }],
    )
    .unwrap();
    let json = serde_json::to_string(&EventPayload::TaskLedger(TaskLedgerRecord::Control(
        transaction,
    )))
    .unwrap();
    let expected = serde_json::json!({"kind":"task.ledger","record_type":"control","record":{"task_revision":0,"command":{"idempotency_key":"create-1","intent_digest":"digest-1","result":{"kind":"task_created","task_id":"tsk_0123456789abcdef01234567"}},"events":[{"event_type":"task_created","event":{"task_id":"tsk_0123456789abcdef01234567","workspace_id":"wsp_0123456789abcdef01234567","title":"A task","created_at":"2026-07-20T00:00:00Z"}}]}});
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&json).unwrap(),
        expected
    );
    let invalid =
        serde_json::json!({"kind":"task.ledger","record_type":"activity","record":{"events":[]}});
    assert!(serde_json::from_value::<EventPayload>(invalid).is_err());
    let wrong_class = serde_json::json!({"kind":"task.ledger","record_type":"activity","record":{"events":[{"event_type":"task_created","event":{"task_id":"tsk_0123456789abcdef01234567","workspace_id":"wsp_0123456789abcdef01234567","title":"x","created_at":"t"}}]}});
    assert!(serde_json::from_value::<EventPayload>(wrong_class).is_err());
    let contradictory = serde_json::json!({"kind":"task.ledger","record_type":"activity","record":{"events":[{"event_type":"run_started","event":{"run":{"run_id":"run_0123456789abcdef01234567","task_id":"tsk_0123456789abcdef01234567","state":"queued","started_at":"x","finished_at":null,"summary":null,"checks":null,"metrics":null,"workspace_changes":null}}}]}});
    assert!(serde_json::from_value::<EventPayload>(contradictory).is_err());

    let revision =
        RevisionToken::parse("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
            .unwrap();
    let snapshot = WorkspaceChangesFact {
        workspace_id: workspace_id(),
        revision: revision.clone(),
        availability: ChangesAvailabilityFact::Available,
        entries: vec![ChangeEntryFact {
            path: WorkspaceRelativePath::parse("src/lib.rs").unwrap(),
            old_path: None,
            kind: ChangeKindFact::Modified,
            staged: false,
            worktree: true,
            binary: false,
            additions: Some(1),
            deletions: Some(0),
            revision,
        }],
        next_cursor: None,
    };
    let mut terminal = run();
    terminal.state = RunState::Completed;
    terminal.summary = Some("done".to_owned());
    terminal.workspace_changes = Some(snapshot.clone());
    let round_trip: RunFact =
        serde_json::from_value(serde_json::to_value(&terminal).unwrap()).unwrap();
    assert_eq!(round_trip.workspace_changes, Some(snapshot));
    assert_eq!(
        TaskEvent::TaskCreated {
            task_id: task_id(),
            workspace_id: workspace_id(),
            title: "x".into(),
            created_at: "t".into()
        }
        .record_class(),
        TaskRecordClass::Control
    );
    assert_eq!(
        TaskEvent::RunStarted { run: run() }.record_class(),
        TaskRecordClass::Activity
    );
}

#[test]
fn task_event_matrix_covers_each_constructible_projection_variant_and_reverse_class() {
    use kuku::event::{
        ActivityFact, ActivityKindFact, ActivityStatusFact, MessageFact, MessageRoleFact,
        SkillsChangedFact,
    };
    let mut queued = run();
    queued.state = RunState::Queued;
    let mut terminal = run();
    terminal.state = RunState::Completed;
    terminal.summary = Some("done".into());
    let message = MessageFact {
        message_id: "msg_1".into(),
        task_id: task_id(),
        run_id: None,
        role: MessageRoleFact::User,
        text: "hi".into(),
        finalized: true,
        request_ids: vec![],
        file_references: vec![],
    };
    let activity = ActivityFact {
        activity_id: "act_1".into(),
        run_id: run().run_id,
        kind: ActivityKindFact::System,
        title: "work".into(),
        status: ActivityStatusFact::Completed,
        detail: None,
        file_references: vec![],
    };
    let controls = vec![
        TaskEvent::TaskCreated {
            task_id: task_id(),
            workspace_id: workspace_id(),
            title: "x".into(),
            created_at: "t".into(),
        },
        TaskEvent::TaskTitleChanged { title: "y".into() },
        TaskEvent::RunQueued {
            run: queued.clone(),
        },
        TaskEvent::RunStopping {
            run: {
                let mut r = queued.clone();
                r.state = RunState::Stopping;
                r
            },
        },
        TaskEvent::InteractionResolved {
            interaction_id: "int_0123456789abcdef01234567".parse().unwrap(),
            choice_id: "yes".into(),
        },
        TaskEvent::MessageAppended {
            message: message.clone(),
        },
        TaskEvent::SkillsChanged {
            selection: SkillsChangedFact {
                tier_id: "tier:default".into(),
                skill_ids: vec![],
            },
        },
    ];
    let activities = vec![
        TaskEvent::RunStarted { run: run() },
        TaskEvent::RunNeedsAttention {
            run: {
                let mut r = run();
                r.state = RunState::NeedsAttention;
                r
            },
        },
        TaskEvent::RunCompleted {
            run: terminal.clone(),
        },
        TaskEvent::MessagePatched {
            message_id: "msg_1".into(),
            append_text: "!".into(),
            finalized: true,
            request_ids: None,
        },
        TaskEvent::ActivityUpserted { activity },
        TaskEvent::InteractionCancelled {
            interaction_id: "int_0123456789abcdef01234567".parse().unwrap(),
        },
    ];
    for event in controls {
        assert_eq!(event.record_class(), TaskRecordClass::Control);
        assert!(TaskActivityBatch::try_new(vec![event]).is_err());
    }
    for event in activities {
        assert_eq!(event.record_class(), TaskRecordClass::Activity);
        assert!(TaskTransaction::try_new(
            TaskRevision::try_new(0).unwrap(),
            receipt(),
            vec![event]
        )
        .is_err());
    }
}

#[test]
fn task_event_matrix_covers_terminal_and_review_values() {
    let mut completed = run();
    completed.state = RunState::Completed;
    completed.summary = Some("done".into());
    let mut stopped = completed.clone();
    stopped.state = RunState::Stopped;
    let mut failed = completed.clone();
    failed.state = RunState::Failed;
    let mut interrupted = completed.clone();
    interrupted.state = RunState::Interrupted;
    let review = TaskEvent::ReviewSubmissionReferenced {
        submission: ReviewSubmissionReference {
            submission_id: ReviewSubmissionId::parse("rsub_0123456789abcdef01234567").unwrap(),
            task_id: task_id(),
            run_id: run().run_id,
            task_revision: TaskRevision::try_new(0).unwrap(),
            submitted_at: "t".into(),
        },
    };
    for event in [
        TaskEvent::RunStopped { run: stopped },
        TaskEvent::RunFailed { run: failed },
        TaskEvent::RunInterrupted { run: interrupted },
        review,
    ] {
        assert_eq!(
            event.record_class(),
            if matches!(event, TaskEvent::ReviewSubmissionReferenced { .. }) {
                TaskRecordClass::Control
            } else {
                TaskRecordClass::Activity
            }
        );
    }
}

#[test]
fn task_event_record_class_table_names_every_variant() {
    let control = [
        "task_created",
        "task_title_changed",
        "run_queued",
        "run_stopping",
        "interaction_resolved",
        "message_appended",
        "skills_changed",
        "review_submission_referenced",
        "review_submission_recorded",
    ];
    let activity = [
        "run_started",
        "run_needs_attention",
        "run_completed",
        "run_stopped",
        "run_failed",
        "run_interrupted",
        "interaction_opened",
        "interaction_cancelled",
        "message_patched",
        "activity_upserted",
        "skill_loaded",
        "request_snapshot",
        "request_started",
        "request_completed",
        "request_failed",
        "observation_recorded",
    ];
    assert_eq!(control.len() + activity.len(), 25);
    assert!(control.iter().all(|name| !activity.contains(name)));
}
