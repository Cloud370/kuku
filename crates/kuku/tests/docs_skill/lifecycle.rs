use std::sync::Arc;
use std::time::Duration;

use kuku::{Activation, AgentConfig, AgentStatus, Input, InputSource, InputTiming, Session};
use serde_json::json;
use tokio::sync::oneshot;

use super::{
    Gate, INDEX, Model, NAME, Reply, SKILL_BODY, TOOL, common, docs_archive_lock, options, settled,
    tool_result,
};

#[tokio::test]
async fn prompt_guides_skill_reuse_and_batches_known_references_without_loading() {
    const TOOLS_PAGE: &str = "references/docs/en/reference/tools.md";
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let model = Model::new(
        data.path(),
        [
            Reply::Batch(vec![
                (
                    "overview",
                    TOOL,
                    json!({"skill_name":NAME,"action":"read","path":INDEX,"limit":4}),
                ),
                (
                    "tool-reference",
                    TOOL,
                    json!({"skill_name":NAME,"action":"read","path":TOOLS_PAGE,"limit":4}),
                ),
            ]),
            Reply::Text,
        ],
    );
    let session = Session::open(options(
        workspace.path(),
        data.path(),
        model.clone(),
        Arc::new(Gate::default()),
    ))
    .await
    .unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new(
            "ask",
            "main",
            "Read the overview and the tool reference.",
        ))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;

    let (status, overview, _) = tool_result(&session, "overview");
    assert_eq!(status, "ok");
    assert!(overview.contains(INDEX));
    let (status, reference, _) = tool_result(&session, "tool-reference");
    assert_eq!(status, "ok");
    assert!(reference.contains(TOOLS_PAGE));
    {
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let initial = requests[0].0.to_string();
        assert!(initial.contains("Reuse Skill instructions already visible in your context"));
        assert!(initial.contains("a new turn is not a reason to reload them"));
        assert!(initial.contains("If you already know the Skill name, skip catalog discovery"));
        assert!(initial.contains("When paths are known, batch independent reads directly"));
        assert!(initial.contains("No Skill load is required"));
        let continued = requests[1].0.to_string();
        assert!(continued.contains("overview") && continued.contains("tool-reference"));
        assert!(!continued.contains(SKILL_BODY));
    }
    session.close().await.unwrap();
}

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
async fn an_ineligible_queued_skill_does_not_interrupt_a_workspace_script() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("fixture.txt"), "available fixture").unwrap();
    let model = Model::new(
        data.path(),
        [
            Reply::Call(
                "script",
                "run_js",
                json!({"code": r#"return await tools.read_file({path:"fixture.txt"});"#}),
            ),
            Reply::Text,
        ],
    );
    let session = Session::open(options(
        workspace.path(),
        data.path(),
        model,
        Arc::new(Gate::default()),
    ))
    .await
    .unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    let mut original = Input::new("run-script", "main", "Read the fixture.");
    original.activation = Activation::Enqueue;
    session.send(original).unwrap();
    let mut later = Input::new("use-skill", "main", "Read this using another Skill.");
    later.activation = Activation::Enqueue;
    later.source = InputSource::Host("second-source".into());
    later.bootstrap_skill = Some("not-installed".into());
    session.send(later).unwrap();
    session.advance("main").unwrap();

    let done = settled(&session, AgentStatus::Idle).await;
    assert_eq!(done.pending_inputs, 1);
    assert_eq!(tool_result(&session, "script").0, "ok");
    session.close().await.unwrap();
}

#[tokio::test]
async fn first_docs_access_retains_the_pinned_archive_before_permission_wait() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let model = Model::new(
        data.path(),
        [
            Reply::Call(
                "defer",
                TOOL,
                json!({"skill_name":NAME,"action":"read","path":INDEX}),
            ),
            Reply::Text,
        ],
    );
    let mut settings = options(
        workspace.path(),
        data.path(),
        model.clone(),
        Arc::new(Gate::default()),
    );
    settings.session_id = Some("first-resource-permission".into());
    let (archive, lock) = docs_archive_lock(&settings, &model);
    drop(lock);
    assert!(!archive.exists());
    let session = Session::open(settings.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("ask", "main", "Read the bundled documentation."))
        .unwrap();
    let waiting = settled(&session, AgentStatus::Waiting)
        .await
        .permission
        .unwrap();
    assert!(archive.is_file());
    assert!(!model.extracted());
    session.close().await.unwrap();

    let reopened = Session::open(settings).await.unwrap();
    reopened
        .reply_permission("approve", "main", &waiting.id, true)
        .unwrap();
    settled(&reopened, AgentStatus::Idle).await;
    let (status, page, facts) = tool_result(&reopened, "defer");
    assert_eq!(status, "ok");
    assert!(page.contains(INDEX));
    assert_eq!(
        facts["version"].as_str(),
        archive.file_stem().and_then(|name| name.to_str())
    );
    for (body, _) in model.requests.lock().unwrap().iter() {
        assert!(!body.to_string().contains(SKILL_BODY));
    }
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn standalone_bootstrap_retains_the_exact_programmatic_body_across_turns() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let server = httpmock::MockServer::start_async().await;
    let body = "Programmatic Skill body not present in any registry.";
    let reply = server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/chat/completions")
                .body_contains(body);
            then.status(200).body(common::openai_sse_response(json!({
                "choices": [{"finish_reason": "stop", "message": {"content": "done"}}]
            })));
        })
        .await;
    let mut config = common::test_config();
    let model = common::set_test_model(&mut config, "openai", "standalone-bootstrap");
    model.think = kuku::config::ThinkLevel::Off;
    model.max_output_tokens = Some(256);
    config.providers.get_mut("openai").unwrap().base_url = server.base_url();
    let query = |prompt| {
        kuku::query(prompt)
            .session("programmatic-bootstrap")
            .config(config.clone())
            .kuku_home(data.path())
            .workspace(workspace.path())
    };
    query("First turn.")
        .bootstrap_skill("programmatic", body.into())
        .run()
        .await
        .unwrap();
    query("Follow-up turn.").run().await.unwrap();
    reply.assert_hits_async(2).await;
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
