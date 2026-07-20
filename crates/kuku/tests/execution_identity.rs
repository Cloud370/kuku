mod common;

use kuku::event::{
    ConversationId, Cursor, ExecutionScope, RevisionToken, RunId, RunState, TaskId, TaskRevision,
    TaskState, TurnId, WorkspaceId,
};
use kuku::query;

#[test]
fn execution_ids_are_typed_prefixed_and_serde_transparent() {
    let task = TaskId::try_new().unwrap();
    let run = RunId::try_new().unwrap();

    assert!(task.as_str().starts_with("tsk_"));
    assert!(run.as_str().starts_with("run_"));
    assert_ne!(task.as_str(), run.as_str());
    let encoded = serde_json::to_string(&run).unwrap();
    assert_eq!(serde_json::from_str::<RunId>(&encoded).unwrap(), run);
}

#[test]
fn malformed_or_wrong_prefix_ids_are_rejected() {
    assert!(TaskId::parse("run_0123456789abcdef01234567").is_err());
    assert!(RunId::parse("run_../escape").is_err());
    assert!(RunId::parse("run_0123456789ABCDEF01234567").is_err());
}

#[test]
fn task_conversation_identity_is_stable_for_one_address() {
    let task = TaskId::parse("tsk_0123456789abcdef01234567").unwrap();
    let other_task = TaskId::parse("tsk_1123456789abcdef01234567").unwrap();

    let first = ConversationId::for_task_address(&task, "main").unwrap();
    let repeated = ConversationId::for_task_address(&task, "main").unwrap();
    let nested = ConversationId::for_task_address(&task, "review/api").unwrap();
    let other_task = ConversationId::for_task_address(&other_task, "main").unwrap();

    assert_eq!(first, repeated);
    assert_ne!(first, nested);
    assert_ne!(first, other_task);
}

#[test]
fn cursor_and_revision_reject_values_outside_json_safe_range() {
    const MAX: u64 = 9_007_199_254_740_991;

    assert_eq!(Cursor::try_new(MAX).unwrap().get(), MAX);
    assert!(Cursor::try_new(MAX + 1).is_err());
    assert!(Cursor::try_new(MAX).unwrap().checked_next().is_err());
    assert_eq!(TaskRevision::try_new(MAX).unwrap().get(), MAX);
    assert!(TaskRevision::try_new(MAX + 1).is_err());
    assert!(serde_json::from_str::<Cursor>(&format!("{}", MAX + 1)).is_err());
    let schema = serde_json::to_value(schemars::schema_for!(Cursor)).unwrap();
    assert_eq!(schema["maximum"], MAX);
}

#[test]
fn revision_tokens_accept_only_bare_lowercase_sha256_hex() {
    let valid = "0123456789abcdef".repeat(4);
    assert_eq!(RevisionToken::parse(&valid).unwrap().as_str(), valid);
    assert!(RevisionToken::parse(format!("sha256:{valid}")).is_err());
    assert!(RevisionToken::parse(valid.to_uppercase()).is_err());
}

#[test]
fn task_and_run_states_share_the_frozen_active_and_terminal_semantics() {
    assert!(TaskState::Queued.is_active());
    assert!(TaskState::Running.is_active());
    assert!(TaskState::NeedsAttention.is_active());
    assert!(TaskState::Stopping.is_active());
    assert!(!TaskState::Draft.is_active());
    assert!(RunState::NeedsAttention.is_active());
    assert!(!RunState::Interrupted.is_active());
}

#[tokio::test(flavor = "current_thread")]
async fn query_preserves_supplied_scope_and_generates_distinct_scopes() {
    let env = common::TestEnv::new();
    let supplied = ExecutionScope {
        workspace_id: WorkspaceId::try_new().unwrap(),
        task_id: TaskId::try_new().unwrap(),
        run_id: RunId::try_new().unwrap(),
        turn_id: TurnId::try_new().unwrap(),
        conversation_id: ConversationId::try_new().unwrap(),
        turn_index: 7,
    };

    let run = query("inspect")
        .config(common::test_config())
        .execution_scope(supplied.clone())
        .start()
        .await
        .unwrap();
    assert_eq!(run.task_id(), &supplied.task_id);
    assert_eq!(run.run_id(), &supplied.run_id);
    assert_eq!(run.turn_id(), &supplied.turn_id);
    drop(run);

    let first_generated = query("inspect")
        .config(common::test_config())
        .start()
        .await
        .unwrap();
    let second_generated = query("inspect again")
        .config(common::test_config())
        .start()
        .await
        .unwrap();
    assert!(first_generated.task_id().as_str().starts_with("tsk_"));
    assert!(first_generated.run_id().as_str().starts_with("run_"));
    assert!(first_generated.turn_id().as_str().starts_with("trn_"));
    assert_ne!(first_generated.task_id(), second_generated.task_id());
    assert_ne!(first_generated.run_id(), second_generated.run_id());
    assert_ne!(first_generated.turn_id(), second_generated.turn_id());
    drop(first_generated);
    drop(second_generated);
    drop(env);
}
