use std::sync::Arc;
use std::time::Duration;

use kuku::event::{EventPayload, EventStore};
use kuku::{
    Activation, AgentConfig, AgentStatus, Input, InputSource, InputStatus, InputTiming, Session,
};
use serde_json::json;
use tokio::sync::oneshot;

use super::{Gate, INDEX, Model, NAME, Reply, SKILL_BODY, common, options, settled, tool_result};

#[tokio::test]
async fn bundled_skill_uses_real_paths_and_normal_batched_file_tools() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let tool_page = "references/docs/en/reference/tools.md";
    let index = data
        .path()
        .join("skill-resources")
        .join(NAME)
        .join(env!("KUKU_DOCS_FINGERPRINT"))
        .join(INDEX);
    let script = format!(
        r#"const path = {};
const read = await tools.read_file({{path}});
const write = await tools.write_file({{path, content: "not a document", expected_version: read.result.content_hash}});
return `${{read.status}}:${{write.status}}`;"#,
        json!(index),
    );
    let model = Model::new(
        data.path(),
        [
            Reply::Call("use-docs", "use_skill", json!({"skill_name":NAME})),
            Reply::Batch(vec![
                ("overview", "read_file", json!({"path":INDEX,"limit":5})),
                (
                    "tool-page",
                    "read_file",
                    json!({"path":tool_page,"limit":5}),
                ),
                (
                    "list",
                    "find_files",
                    json!({"path":"references/docs/en/reference"}),
                ),
                (
                    "search",
                    "search_text",
                    json!({"path":tool_page,"pattern":"# Tools"}),
                ),
                ("denied", "read_file", json!({"path":INDEX})),
                (
                    "bad-write",
                    "write_file",
                    json!({"path":INDEX,"content":"not a document"}),
                ),
                ("script", "run_js", json!({"code":script})),
            ]),
            Reply::Text,
        ],
    );
    let gate = Arc::new(Gate::default());
    let session = Session::open(options(
        workspace.path(),
        data.path(),
        model.clone(),
        gate.clone(),
    ))
    .await
    .unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new(
            "docs",
            "main",
            "Look up the overview and tool reference.",
        ))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;

    let (status, body, facts) = tool_result(&session, "use-docs");
    assert_eq!(status, "ok");
    let root = model.skill_dir().canonicalize().unwrap();
    assert_eq!(facts["skill_directory"], json!(root));
    assert!(body.contains(root.to_str().unwrap()));
    assert!(root.join("SKILL.md").is_file());
    let (status, page, _) = tool_result(&session, "overview");
    assert_eq!(status, "ok");
    assert!(page.contains("kuku"));
    let (status, page, _) = tool_result(&session, "tool-page");
    assert_eq!(status, "ok");
    assert!(page.contains("# Tools"));
    assert_eq!(tool_result(&session, "list").0, "ok");
    let (status, matches, _) = tool_result(&session, "search");
    assert_eq!(status, "ok", "{matches}");
    assert!(matches.contains("# Tools"), "{matches}");
    assert_eq!(tool_result(&session, "denied").0, "blocked");
    let (status, script, _) = tool_result(&session, "script");
    assert_eq!(status, "ok", "{script}");
    assert!(script.contains("ok:blocked"), "{script}");
    {
        let calls = gate.0.lock().unwrap();
        for (id, path) in [
            ("overview", INDEX),
            ("tool-page", tool_page),
            ("search", tool_page),
            ("denied", INDEX),
        ] {
            let action = calls
                .iter()
                .find(|action| action.tool_call_id == id)
                .unwrap();
            assert_eq!(action.args["path"], json!(root.join(path)));
        }
        assert!(!calls.iter().any(|action| action.tool.name == "write_file"));
    }
    assert!(
        tool_result(&session, "bad-write")
            .1
            .contains("managed by kuku")
    );
    assert!(root.join(INDEX).read_link().is_err());
    assert!(
        !std::fs::read_to_string(root.join(INDEX))
            .unwrap()
            .contains("not a document")
    );
    assert!(!workspace.path().join("references").exists());
    {
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(!requests[0].0.to_string().contains("skill_resources"));
        assert!(requests[0].0.to_string().contains(
            "Resolve references and bundled scripts relative to the real Skill directory"
        ));
        let final_request = requests[2].0.to_string();
        assert!(final_request.contains("overview") && final_request.contains("tool-page"));
        assert_eq!(final_request.matches(SKILL_BODY).count(), 1);
    }
    session.close().await.unwrap();
}

#[tokio::test]
async fn close_cancels_a_turn_waiting_on_bundle_publication() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let model = Model::new(data.path(), []);
    std::fs::create_dir_all(&model.cache_root).unwrap();
    let version = model.cache_root.join(env!("KUKU_DOCS_FINGERPRINT"));
    let mut publisher = fslock::LockFile::open(&version.with_extension("lock")).unwrap();
    publisher.lock().unwrap();
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
        .send(Input::new("start", "main", "Use the available Skills."))
        .unwrap();
    // On this single-threaded runtime the worker takes the input and reaches
    // the blocking publication await before yielding again.
    tokio::time::timeout(Duration::from_secs(5), async {
        while session.input_status("main", "start").unwrap() != InputStatus::Accepted {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the worker should accept the input");
    assert_eq!(session.agent("main").unwrap().status, AgentStatus::Running);
    tokio::time::timeout(Duration::from_secs(2), session.close())
        .await
        .expect("close must not wait for an unrelated publisher's lock")
        .unwrap();
    assert!(!version.exists());
    assert!(!version.with_extension("bundle").exists());
    assert!(model.requests.lock().unwrap().is_empty());
    let events = EventStore::replay(session.events_path()).unwrap();
    let turn = events.iter().find_map(|event| match &event.payload {
        EventPayload::MessageUser { text, turn, .. } if text == "Use the available Skills." => {
            Some(*turn)
        }
        _ => None,
    });
    assert!(
        turn.is_some(),
        "accepted input remains visible when startup is cancelled"
    );
    assert!(events.iter().any(|event| {
        matches!(&event.payload, EventPayload::TurnCancelled { turn: cancelled, .. } if Some(*cancelled) == turn)
    }));
}

#[cfg(unix)]
#[tokio::test]
async fn local_skill_script_path_is_real_and_workspace_cwd_is_preserved() {
    let workspace = tempfile::Builder::new()
        .prefix("skill workspace ")
        .tempdir()
        .unwrap();
    let data = tempfile::tempdir().unwrap();
    let skill = workspace.path().join(".kuku/skills/local-script");
    let script = skill.join("scripts/check.sh");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(workspace.path().join("project-marker"), "available").unwrap();
    std::fs::write(skill.join("SKILL.md"),
        "---\nname: local-script\ndescription: Check the working directory.\n---\nRun scripts/check.sh.\n",
    ).unwrap();
    std::fs::write(
        &script,
        "test -f project-marker && printf 'real skill path plus project cwd\\n'\n",
    )
    .unwrap();
    let model = Model::new(
        data.path(),
        [
            Reply::Call(
                "use-script",
                "use_skill",
                json!({"skill_name":"local-script"}),
            ),
            Reply::Call(
                "execute-script",
                "run_command",
                json!({"program":"/bin/sh","args":[script]}),
            ),
            Reply::Text,
        ],
    );
    let mut settings = options(
        workspace.path(),
        data.path(),
        model,
        Arc::new(Gate::default()),
    );
    settings.config.discovery.extra_project_paths = vec![workspace.path().join(".kuku")];
    let session = Session::open(settings).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("run", "main", "Run the local script workflow."))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;
    assert_eq!(
        tool_result(&session, "use-script").2["skill_directory"],
        json!(skill.canonicalize().unwrap())
    );
    let (status, content, _) = tool_result(&session, "execute-script");
    assert_eq!(status, "ok", "{content}");
    assert!(
        content.contains("real skill path plus project cwd"),
        "{content}"
    );
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
async fn input_with_removed_queued_skill_can_be_rewound_after_reopen() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let skills_root = workspace.path().join(".fixture");
    let skill = skills_root.join("skills/removed-skill/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(
        &skill,
        "---\nname: removed-skill\ndescription: Initially available.\n---\nLocal Skill instructions.\n",
    )
    .unwrap();
    let model = Model::new(data.path(), [Reply::Text, Reply::Text]);
    let mut settings = options(
        workspace.path(),
        data.path(),
        model.clone(),
        Arc::new(Gate::default()),
    );
    settings.session_id = Some("queued-skill-removed".into());
    settings.config.discovery.extra_project_paths = vec![skills_root];
    let session = Session::open(settings.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("earlier", "main", "Keep this earlier input."))
        .unwrap();
    settled(&session, AgentStatus::Idle).await;

    const ORIGINAL: &str = "Use the Skill that was available when queued.";
    let mut input = Input::new("queued", "main", ORIGINAL);
    input.activation = Activation::Enqueue;
    input.bootstrap_skill = Some("removed-skill".into());
    let receipt = session.send(input.clone()).unwrap();
    assert_eq!(
        session.input_status("main", "queued").unwrap(),
        InputStatus::Queued
    );
    std::fs::remove_file(skill).unwrap();
    session.advance("main").unwrap();
    let failed = settled(&session, AgentStatus::Interrupted).await;
    assert!(failed.last_error.unwrap().contains("removed-skill"));
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    session.close().await.unwrap();

    let reopened = Session::open(settings).await.unwrap();
    assert_eq!(
        reopened.agent("main").unwrap().status,
        AgentStatus::Interrupted
    );
    assert_eq!(
        reopened.input_status("main", "queued").unwrap(),
        InputStatus::Accepted
    );
    let events = EventStore::replay(reopened.events_path()).unwrap();
    let mut matches = events.iter().filter(|event| {
        matches!(&event.payload, EventPayload::MessageUser { text, .. } if text == ORIGINAL)
    });
    let target = matches
        .next()
        .expect("accepted input has a rewindable user event");
    assert!(matches.next().is_none());
    assert!(events.iter().any(|event| {
        matches!(&event.payload, EventPayload::TurnInterrupted { turn, .. } if Some(*turn) == failed.turn)
    }));
    reopened
        .rewind_message("rewind", "main", target.id, ORIGINAL, false)
        .unwrap();
    assert_eq!(
        reopened.input_status("main", "queued").unwrap(),
        InputStatus::Withdrawn
    );
    assert_eq!(reopened.send(input).unwrap(), receipt);
    assert_eq!(model.requests.lock().unwrap().len(), 1);

    let mut replacement = Input::new("replacement", "main", "Use this replacement.");
    replacement.activation = Activation::Enqueue;
    reopened.send(replacement).unwrap();
    reopened.advance("main").unwrap();
    settled(&reopened, AgentStatus::Idle).await;
    {
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let wire = requests[1].0.to_string();
        assert!(wire.contains("Use this replacement."));
        assert!(wire.contains("Keep this earlier input."));
        assert!(!wire.contains(ORIGINAL));
    }
    reopened.close().await.unwrap();
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
