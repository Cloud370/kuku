use kuku::event::{
    EventPayload, ExecutionScope, SkillLoadFact, SkillLoadOrigin, SourceFact, SourceScope,
    TaskActivityBatch, TaskEvent, TaskId, TaskLedgerRecord, WorkspaceRelativePath,
};
use tempfile::tempdir;

use super::{selected_skill_facts, DriverCommand, RunDriverFactory};

fn scope(task_id: TaskId, run_id: &str) -> ExecutionScope {
    ExecutionScope {
        workspace_id: kuku::WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap(),
        task_id,
        run_id: kuku::RunId::parse(run_id).unwrap(),
        turn_id: kuku::TurnId::parse("trn_0123456789abcdef01234567").unwrap(),
        conversation_id: kuku::ConversationId::parse("con_0123456789abcdef01234567").unwrap(),
        turn_index: 1,
    }
}

fn append_skill(
    store: &mut kuku::event::EventStore,
    execution: ExecutionScope,
    skill_id: &str,
    relative_path: &str,
    content_hash: &str,
) {
    let batch = TaskActivityBatch::try_new(vec![TaskEvent::SkillLoaded(SkillLoadFact {
        execution,
        caused_by_request_id: None,
        skill_id: skill_id.to_owned(),
        source: SourceFact {
            scope: SourceScope::Project,
            id: format!("source:{skill_id}"),
            relative_path: Some(WorkspaceRelativePath::parse(relative_path).unwrap()),
        },
        origin: SkillLoadOrigin::You,
        content_hash: content_hash.to_owned(),
    })])
    .unwrap();
    store
        .append_synced(EventPayload::TaskLedger(TaskLedgerRecord::Activity(batch)))
        .unwrap();
}

#[test]
fn selected_skill_facts_are_exact_task_scoped_and_selection_ordered() {
    let directory = tempdir().unwrap();
    let mut store = kuku::event::EventStore::open(directory.path().join("events.jsonl")).unwrap();
    let task_id = TaskId::parse("tsk_0123456789abcdef01234567").unwrap();
    let other_task = TaskId::parse("tsk_1123456789abcdef01234567").unwrap();
    let current = scope(task_id.clone(), "run_0123456789abcdef01234567");
    append_skill(
        &mut store,
        scope(other_task, "run_0123456789abcdef01234567"),
        "skill:project:second",
        ".agents/skills/wrong/SKILL.md",
        "sha256:wrong-task",
    );
    append_skill(
        &mut store,
        scope(task_id.clone(), "run_1123456789abcdef01234567"),
        "skill:project:second",
        ".agents/skills/historical/SKILL.md",
        "sha256:historical",
    );
    append_skill(
        &mut store,
        current.clone(),
        "skill:project:first",
        ".agents/skills/first/SKILL.md",
        "sha256:first",
    );
    append_skill(
        &mut store,
        current.clone(),
        "skill:project:second",
        ".agents/skills/second/SKILL.md",
        "sha256:second",
    );

    let selected = selected_skill_facts(
        &store,
        &current,
        &[
            "skill:project:second".to_owned(),
            "skill:project:first".to_owned(),
        ],
    )
    .unwrap();
    assert_eq!(selected[0].skill_id, "skill:project:second");
    assert_eq!(selected[1].skill_id, "skill:project:first");
    assert_eq!(
        selected[0].source.relative_path.as_ref().unwrap().as_str(),
        ".agents/skills/second/SKILL.md"
    );
}

#[test]
fn selected_skill_facts_reject_missing_duplicate_and_repeated_selection() {
    let directory = tempdir().unwrap();
    let mut store = kuku::event::EventStore::open(directory.path().join("events.jsonl")).unwrap();
    let task_id = TaskId::parse("tsk_0123456789abcdef01234567").unwrap();
    let current = scope(task_id, "run_0123456789abcdef01234567");
    assert!(selected_skill_facts(&store, &current, &["skill:project:missing".to_owned()]).is_err());

    append_skill(
        &mut store,
        current.clone(),
        "skill:project:duplicate",
        ".agents/skills/duplicate/SKILL.md",
        "sha256:duplicate",
    );
    append_skill(
        &mut store,
        current.clone(),
        "skill:project:duplicate",
        ".agents/skills/duplicate/SKILL.md",
        "sha256:duplicate",
    );
    assert!(
        selected_skill_facts(&store, &current, &["skill:project:duplicate".to_owned()]).is_err()
    );
    assert!(selected_skill_facts(
        &store,
        &current,
        &[
            "skill:project:duplicate".to_owned(),
            "skill:project:duplicate".to_owned(),
        ]
    )
    .is_err());
}

fn skill_markdown(name: &str, instructions: &str) -> String {
    format!("---\nname: {name}\ndescription: Test skill\n---\n{instructions}\n")
}

fn skill_hash(name: &str, instructions: &str) -> String {
    kuku::skill::definition::SkillDefinition {
        name: name.to_owned(),
        description: "Test skill".to_owned(),
        instructions: instructions.to_owned(),
        source: kuku::skill::definition::SkillSource::Project,
        hash: String::new(),
        source_path: None,
        allowed_tools: None,
        disallowed_tools: None,
        max_turns: None,
        model: None,
        license: None,
        compatibility: None,
        metadata: serde_json::Value::Null,
    }
    .compute_hash()
}

#[tokio::test]
async fn real_factory_loads_only_the_durable_selected_skill_descriptor() {
    let (factory, mut start, _home, allowed) =
        super::activity_tests::factory_fixture("tier:default").await;
    let skills = allowed.path().join("project/.agents/skills");
    for name in ["selected", "unselected"] {
        let directory = skills.join(name);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("SKILL.md"),
            skill_markdown(name, &format!("Use {name}.")),
        )
        .unwrap();
    }
    let mut store = start.event_store.clone();
    append_skill(
        &mut store,
        start.execution_scope.clone(),
        "skill:project:selected",
        ".agents/skills/selected/SKILL.md",
        &skill_hash("selected", "Use selected."),
    );
    start.selected_skills = selected_skill_facts(
        &store,
        &start.execution_scope,
        &["skill:project:selected".to_owned()],
    )
    .unwrap();

    let handle = factory.start(start).await.unwrap();
    handle.commands.send(DriverCommand::Stop).await.unwrap();
    let registry = store
        .read_all()
        .unwrap()
        .into_iter()
        .find_map(|event| match event.payload {
            EventPayload::ContextSkills { registry, .. } => Some(registry),
            _ => None,
        })
        .unwrap();
    assert_eq!(registry["names"], serde_json::json!(["selected"]));
}

#[tokio::test]
async fn real_factory_rejects_selected_skill_content_hash_drift() {
    let (factory, mut start, _home, allowed) =
        super::activity_tests::factory_fixture("tier:default").await;
    let directory = allowed.path().join("project/.agents/skills/selected");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("SKILL.md"),
        skill_markdown("selected", "Changed after selection."),
    )
    .unwrap();
    let mut store = start.event_store.clone();
    append_skill(
        &mut store,
        start.execution_scope.clone(),
        "skill:project:selected",
        ".agents/skills/selected/SKILL.md",
        &skill_hash("selected", "Original content."),
    );
    start.selected_skills = selected_skill_facts(
        &store,
        &start.execution_scope,
        &["skill:project:selected".to_owned()],
    )
    .unwrap();

    assert!(matches!(
        factory.start(start).await,
        Err(super::DomainError::InvalidRequest)
    ));
}
