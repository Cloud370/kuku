#[test]
fn scenario_fixture_is_provider_input_not_a_projection_script() {
    let factory = ScenarioDriverFactory::from_fixture("core_task", 7).unwrap();
    assert!(!factory.fixture_source().contains("TaskProjection"));
    assert!(!factory.fixture_source().contains("ledger"));
}

#[test]
fn feature_scenarios_are_input_fragments_not_serialized_server_state() {
    for source in [FULL_TASK_FIXTURE, HUMAN_ACCEPTANCE_FIXTURE] {
        parse_feature_scenario(source).unwrap();
        assert!(!source.contains("TaskProjection"));
        assert!(!source.contains("TaskLedgerRecord"));
        assert!(!source.contains("\"projection\""));
        assert!(!source.contains("\"ledger\""));
    }
    assert!(parse_feature_scenario(r#"{"projection":{"task_revision":1}}"#).is_err());
    assert!(parse_feature_scenario(r#"{"kind":"TaskLedgerRecord"}"#).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn feature_scenarios_drive_real_context_and_review_services() {
    let _environment = SCENARIO_ENV_LOCK.lock().await;
    for source in [FULL_TASK_FIXTURE, HUMAN_ACCEPTANCE_FIXTURE] {
        run_feature_scenario(parse_feature_scenario(source).unwrap()).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scenario_uses_commands_runtime_ledger_and_authenticated_controls() {
    let _environment = SCENARIO_ENV_LOCK.lock().await;
    std::env::set_var("KUKU_TEST_SCENARIO", "full_task");
    std::env::set_var("KUKU_TEST_SEED", "7");
    let server = common::TestServer::start_unconfigured_with_token(Some(TOKEN.to_owned())).await;
    std::env::remove_var("KUKU_TEST_SCENARIO");
    std::env::remove_var("KUKU_TEST_SEED");

    let scenario = parse_feature_scenario(FULL_TASK_FIXTURE).unwrap();
    materialize_workspace(server.workspace.path(), &scenario.workspace);
    let client = FeatureClient::new(server.base_url.clone());
    let unauthenticated = wreq::Client::new()
        .post(format!(
            "{}/api/v1/testing/barriers/after-tool/release",
            server.base_url
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(401, unauthenticated.status().as_u16());

    let workspace_id = initialize_feature_server(&client, &server, &scenario, 1).await;
    let created: CreateTaskResponse = client
        .post(
            "/api/v1/tasks",
            &json!({
                "workspace_id": workspace_id,
                "idempotency_key": "scenario-runtime-create"
            }),
            201,
        )
        .await;
    let task_id = created.projection.task.task_id;
    let _: SubmitRunResponse = client
        .post(
            &format!("/api/v1/tasks/{task_id}/runs"),
            &json!({
                "expected_task_revision": created.projection.task_revision,
                "idempotency_key": "scenario-runtime-run",
                "message": scenario.task.message,
                "tier_id": "tier:balanced",
                "skill_ids": []
            }),
            202,
        )
        .await;

    let after_tool = client
        .post_response("/api/v1/testing/barriers/after-tool/release", &json!({}))
        .await;
    assert_eq!(204, after_tool.status().as_u16());

    let mut pending = None;
    let mut last_observed = None;
    for _ in 0..200 {
        let projection: TaskProjection = client.get(&format!("/api/v1/tasks/{task_id}")).await;
        last_observed = Some((
            projection.task.state,
            projection.cursor,
            projection.timeline.len(),
        ));
        if let Some(interaction) = projection.timeline.iter().find_map(|item| match item {
            TimelineItemProjection::Interaction(interaction)
                if interaction.selected_choice_id.is_none() =>
            {
                Some(interaction.clone())
            }
            _ => None,
        }) {
            assert_eq!(TaskState::NeedsAttention, projection.task.state);
            pending = Some((projection.task_revision, interaction));
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let pending = pending.unwrap_or_else(|| {
        panic!("feature scenario exposed no pending interaction; last={last_observed:?}")
    });
    assert_eq!("Permission request", pending.1.prompt);
    let _: Value = client
        .post(
            &format!(
                "/api/v1/tasks/{task_id}/interactions/{}",
                pending.1.interaction_id
            ),
            &json!({
                "expected_task_revision": pending.0,
                "idempotency_key": "scenario-runtime-interaction",
                "choice_id": pending.1.choices[0].choice_id
            }),
            202,
        )
        .await;
    let continuity = client
        .post_response(
            "/api/v1/testing/barriers/continuity-before-finish/release",
            &json!({}),
        )
        .await;
    assert_eq!(204, continuity.status().as_u16());
    let terminal = wait_for_terminal_projection(&client, &task_id).await;
    assert_eq!(TaskState::Completed, terminal.task.state);
    assert!(terminal.cursor.get() > created.projection.cursor.get());
    assert!(terminal
        .loaded_skills
        .iter()
        .any(|skill| { skill.skill_id == "skill:project:status" && skill.loaded_by == "agent" }));

    let context: ContextSnapshot = client
        .get(&format!("/api/v1/tasks/{task_id}/context"))
        .await;
    assert_eq!(1, context.request_history.len());
    assert!(exact_request_contains(&context, &scenario.task.message));
    let expected_skill_hash = format!("sha256:{:x}", Sha256::digest(STATUS_SKILL_CONTENT));
    assert_eq!(1, context.sections.skills.len());
    assert_eq!("skill:project:status", context.sections.skills[0].skill_id);
    assert_eq!(
        kuku::event::SkillLoadOrigin::Agent,
        context.sections.skills[0].origin
    );
    assert_eq!(expected_skill_hash, context.sections.skills[0].content_hash);
    assert_eq!(
        kuku::event::SourceScope::Project,
        context.sections.skills[0].source.scope
    );
    assert_eq!(
        "skill-source:project:status",
        context.sections.skills[0].source.id
    );
    assert_eq!(
        Some(STATUS_SKILL_PATH),
        context.sections.skills[0]
            .source
            .relative_path
            .as_ref()
            .map(kuku::event::WorkspaceRelativePath::as_str)
    );
    let request_id = &context.request_history[0].request_id;
    let historical: ContextSnapshot = client
        .get(&format!("/api/v1/tasks/{task_id}/context/{request_id}"))
        .await;
    assert_eq!(context.sections.skills, historical.sections.skills);
    let expected_file_path = &scenario.workspace.working_files[0].path;
    assert_eq!(1, context.sections.observations.len());
    assert_eq!(
        Some(expected_file_path.as_str()),
        context.sections.observations[0]
            .relative_path
            .as_ref()
            .map(kuku::event::WorkspaceRelativePath::as_str)
    );

    let mut before = None;
    let mut history_ids = std::collections::BTreeSet::new();
    let mut tool_file_reference = None;
    loop {
        let path = before.as_ref().map_or_else(
            || format!("/api/v1/tasks/{task_id}/timeline?limit=500"),
            |cursor: &kuku_server::api::PageCursor| {
                format!(
                    "/api/v1/tasks/{task_id}/timeline?limit=500&before={}",
                    cursor.as_str()
                )
            },
        );
        let page: TimelinePage = client.get(&path).await;
        for item in page.items {
            if let TimelineItemProjection::Activity(activity) = item {
                if activity.activity_id.starts_with("scenario-history-7-") {
                    assert!(history_ids.insert(activity.activity_id));
                } else if activity.activity_id.starts_with("scenario-tool-7-") {
                    assert_eq!(1, activity.file_references.len());
                    tool_file_reference = Some(activity.file_references[0].relative_path.clone());
                }
            }
        }
        before = page.next_cursor;
        if before.is_none() {
            break;
        }
    }
    assert_eq!(10_000, history_ids.len());
    assert_eq!(Some(expected_file_path.clone()), tool_file_reference);

    let _: SubmitRunResponse = client
        .post(
            &format!("/api/v1/tasks/{task_id}/runs"),
            &json!({
                "expected_task_revision": terminal.task_revision,
                "idempotency_key": "scenario-runtime-follow-up",
                "message": "Capture a second immutable Request snapshot",
                "tier_id": "tier:balanced",
                "skill_ids": []
            }),
            202,
        )
        .await;
    let mut follow_up_context = None;
    for _ in 0..200 {
        let response = client
            .client
            .get(format!("{}/api/v1/tasks/{task_id}/context", client.base_url))
            .header("authorization", format!("Bearer {TOKEN}"))
            .send()
            .await
            .unwrap();
        assert_eq!(
            200,
            response.status().as_u16(),
            "follow-up Request made Context ledger unreadable"
        );
        let snapshot: ContextSnapshot = response.json().await.unwrap();
        if snapshot.request_history.len() == 2 {
            follow_up_context = Some(snapshot);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let follow_up_context = follow_up_context.expect("follow-up Request was not recorded");
    assert_ne!(
        follow_up_context.request_history[0].request_id,
        follow_up_context.request_history[1].request_id
    );

    let follow_up_projection: TaskProjection =
        client.get(&format!("/api/v1/tasks/{task_id}")).await;
    let _: Value = client
        .post(
            &format!("/api/v1/tasks/{task_id}/stop"),
            &json!({
                "expected_task_revision": follow_up_projection.task_revision,
                "idempotency_key": "scenario-runtime-follow-up-stop"
            }),
            202,
        )
        .await;
    assert_eq!(
        TaskState::Stopped,
        wait_for_terminal_projection(&client, &task_id).await.task.state
    );
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

#[tokio::test]
async fn scenario_factory_drives_the_production_driver_contract() {
    async fn initial_request_id(
        handle: &mut kuku_server::run_manager::driver::DriverHandle,
    ) -> String {
        assert_eq!(
            handle.events.recv().await,
            Some(RuntimeDriverEvent::Started)
        );
        while let Some(event) = handle.events.recv().await {
            if let RuntimeDriverEvent::Activity(events) = event {
                if let Some(request_id) = events.iter().find_map(|event| match event {
                    TaskEvent::RequestStarted(started) => {
                        Some(started.scope.request_id.as_str().to_owned())
                    }
                    _ => None,
                }) {
                    return request_id;
                }
            }
        }
        panic!("scenario driver emitted no RequestStarted event");
    }

    fn assert_driver_factory<T: RunDriverFactory>() {}
    assert_driver_factory::<ScenarioDriverFactory>();

    let directory = tempfile::tempdir().unwrap();
    let event_store = kuku::event::EventStore::open(directory.path().join("events.jsonl")).unwrap();
    let mut ids = kuku_server::testing::ScenarioIds::seeded(7);
    let start = DriverStart {
        task_id: ids.task_id(),
        run_id: ids.run_id(),
        workspace_id: ids.workspace_id(),
        prompt: "Run the deterministic scenario".to_owned(),
        tier_id: "tier:default".to_owned(),
        selected_skills: Vec::new(),
        agent_message_id: "msg_agent_scenario".to_owned(),
        execution_scope: kuku::event::ExecutionScope {
            workspace_id: ids.workspace_id(),
            task_id: ids.task_id(),
            run_id: ids.run_id(),
            turn_id: ids.turn_id(),
            conversation_id: ids.conversation_id(),
            turn_index: 1,
        },
        event_store,
    };
    let factory = ScenarioDriverFactory::from_fixture("core_task", 7).unwrap();
    let control = factory.control();
    let mut first = factory.start(start.clone()).await.unwrap();
    let first_request_id = initial_request_id(&mut first).await;
    let second_run_id = ids.run_id();
    let second = DriverStart {
        run_id: second_run_id.clone(),
        agent_message_id: "msg_agent_scenario_follow_up".to_owned(),
        execution_scope: kuku::event::ExecutionScope {
            run_id: second_run_id,
            turn_id: ids.turn_id(),
            turn_index: 2,
            ..start.execution_scope
        },
        ..start
    };
    let mut follow_up = factory.start(second).await.unwrap();
    let follow_up_request_id = initial_request_id(&mut follow_up).await;

    assert_ne!(first_request_id, follow_up_request_id);
    control.release("after-tool").await.unwrap();
}

#[test]
fn feature_scenarios_include_required_runtime_inputs() {
    for name in ["full_task", "human_acceptance"] {
        let factory = ScenarioDriverFactory::from_fixture(name, 11).unwrap();
        assert_eq!(factory.fixture().name, name);
        assert!(factory
            .fixture()
            .barriers
            .iter()
            .any(|name| name == "after-tool"));
        assert!(factory
            .fixture()
            .barriers
            .iter()
            .any(|name| name == "continuity-before-finish"));

        let tool_position = factory
            .fixture()
            .events
            .iter()
            .position(|event| matches!(event, DriverEvent::ToolCall { .. }))
            .unwrap();
        let interaction_position = factory
            .fixture()
            .events
            .iter()
            .position(|event| matches!(event, DriverEvent::Interaction { .. }))
            .unwrap();
        assert!(interaction_position > tool_position);

        let skill = factory
            .fixture()
            .events
            .iter()
            .find_map(|event| match event {
                DriverEvent::AgentSkillLoaded { skill_id, source } => Some((skill_id, source)),
                _ => None,
            })
            .unwrap();
        assert_eq!("skill:project:status", skill.0);
        assert_eq!(STATUS_SKILL_PATH, skill.1.relative_path.as_str());
        assert_eq!(STATUS_SKILL_CONTENT, skill.1.content);

        let fixture = serde_json::to_value(factory.fixture()).unwrap();
        let history = fixture["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["kind"] == "timeline_history")
            .unwrap();
        assert_eq!(history["count"], 10_000);
        assert!(history["batch_size"].as_u64().unwrap() <= 250);
    }
}

#[test]
fn compact_full_task_scenario_omits_long_history() {
    let factory = ScenarioDriverFactory::from_fixture("full_task_compact", 11).unwrap();

    assert_eq!("full_task_compact", factory.fixture().name);
    assert!(factory
        .fixture()
        .events
        .iter()
        .any(|event| matches!(event, DriverEvent::Interaction { .. })));
    assert!(!factory
        .fixture()
        .events
        .iter()
        .any(|event| matches!(event, DriverEvent::TimelineHistory { .. })));
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
