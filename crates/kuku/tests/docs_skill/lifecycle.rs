use std::sync::Arc;
use std::time::Duration;

use kuku::{AgentConfig, AgentStatus, Input, InputTiming, Session};
use serde_json::json;
use tokio::sync::oneshot;

use super::{Gate, Model, NAME, Reply, SKILL_BODY, options, settled, tool_result};

#[tokio::test]
async fn adding_a_skill_mid_turn_does_not_interrupt_the_running_agent() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (started, seeing_request) = oneshot::channel();
    let (release, resume) = oneshot::channel();
    let model = Model::new(data.path(), [Reply::Hold(started, resume), Reply::Text]);
    let mut settings = options(
        workspace.path(),
        data.path(),
        model.clone(),
        Arc::new(Gate::default()),
    );
    settings.config.discovery.extra_project_paths = vec![workspace.path().join(".fixture")];
    let session = Session::open(settings).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("original", "main", "Keep this turn running."))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), seeing_request)
        .await
        .unwrap()
        .unwrap();
    let source = workspace.path().join(".fixture/skills/added-mid-turn");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("SKILL.md"), "---\nname: added-mid-turn\ndescription: New during an active Turn.\n---\nUnique newly installed instructions.\n").unwrap();

    let mut input = Input::new("steer", "main", "Use the new skill.");
    input.bootstrap_skill = Some("added-mid-turn".into());
    let error = session.send(input.clone()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("not available in the current Turn")
    );
    let snapshot = session.agent("main").unwrap();
    assert_eq!(AgentStatus::Running, snapshot.status);
    assert_eq!(0, snapshot.pending_inputs);
    release.send(()).unwrap();
    settled(&session, AgentStatus::Idle).await;

    input.command_id = "next".into();
    input.timing = InputTiming::NextTurn;
    session.send(input).unwrap();
    settled(&session, AgentStatus::Idle).await;
    {
        let requests = model.requests.lock().unwrap();
        assert_eq!(2, requests.len());
        assert!(
            !requests[0]
                .0
                .to_string()
                .contains("Unique newly installed instructions.")
        );
        assert!(
            requests[1]
                .0
                .to_string()
                .contains("Unique newly installed instructions.")
        );
    }
    session.close().await.unwrap();
}

#[tokio::test]
async fn a_remaining_call_reuses_skill_visible_to_the_step_before_permission_wait() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let model = Model::new(
        data.path(),
        [
            Reply::Batch(vec![
                (
                    "defer",
                    "write_file",
                    json!({"path":"never-written.txt","content":"fixture"}),
                ),
                ("reload", "use_skill", json!({"skill_name":NAME})),
            ]),
            Reply::Text,
        ],
    );
    let mut settings = options(
        workspace.path(),
        data.path(),
        model.clone(),
        Arc::new(Gate::default()),
    );
    settings.session_id = Some("skill-admitted-then-deferred".into());
    let session = Session::open(settings.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    let mut input = Input::new("first", "main", "Use the built-in instructions.");
    input.bootstrap_skill = Some(NAME.into());
    session.send(input).unwrap();
    let pending = settled(&session, AgentStatus::Waiting)
        .await
        .permission
        .unwrap();
    session.close().await.unwrap();

    let reopened = Session::open(settings).await.unwrap();
    reopened
        .reply_permission("deny", "main", &pending.id, false)
        .unwrap();
    settled(&reopened, AgentStatus::Idle).await;
    assert_eq!(tool_result(&reopened, "reload").2["reused"], true);
    {
        let requests = model.requests.lock().unwrap();
        assert_eq!(2, requests.len());
        for (body, _) in requests.iter() {
            assert_eq!(1, body.to_string().matches(SKILL_BODY).count());
        }
    }
    reopened.close().await.unwrap();
}
