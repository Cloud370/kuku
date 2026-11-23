use std::fs;
use std::path::Path;

use httpmock::prelude::{MockServer, POST};
use serde_json::json;
use sha2::{Digest, Sha256};

use kuku::{Input, Session};

use super::{agent, common, facts, idle, options, response};

#[tokio::test]
async fn distinct_workspaces_share_guidance_and_behavior_before_project_policy() {
    let server = MockServer::start_async().await;
    let workspace_a = tempfile::tempdir().unwrap();
    let workspace_b = tempfile::tempdir().unwrap();
    let data_a = tempfile::tempdir().unwrap();
    let data_b = tempfile::tempdir().unwrap();
    let workspace_a_alias = workspace_a.path().join(".");
    let workspace_b_alias = workspace_b.path().join(".");
    fs::write(workspace_a.path().join("AGENTS.md"), "POLICY_A").unwrap();
    fs::write(workspace_b.path().join("AGENTS.md"), "POLICY_B").unwrap();
    let (first, path) = options(&server, &workspace_a_alias, data_a.path());
    let mut second = first.clone();
    second.workspace = workspace_b_alias.clone();
    second.data_dir = data_b.path().into();
    let model = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .matches(common::capture_http_request);
            then.status(200)
                .body(response(json!({"content":"done"}), "stop"));
        })
        .await;
    for original in [first, second] {
        let session = Session::open(original).await.unwrap();
        let mut config = agent();
        config.instructions = Some("SHARED_BEHAVIOR".into());
        session.create_agent("create", "main", config).unwrap();
        session.send(Input::new("first", "main", "first")).unwrap();
        idle(&session, "main").await;
        session.close().await.unwrap();
    }
    model.assert_hits_async(2).await;
    let bodies = common::take_http_requests(&path);
    assert_eq!(bodies[0]["tools"], bodies[1]["tools"]);
    let a = bodies[0]["messages"].as_array().unwrap();
    let b = bodies[1]["messages"].as_array().unwrap();
    assert_eq!(&a[..4], &b[..4]);
    assert!(a[1]["content"]
        .as_str()
        .unwrap()
        .starts_with("<kuku_tool_guidance>"));
    assert!(!a[1]["content"].as_str().unwrap().contains("<!-- tools:"));
    assert_eq!(
        kuku::builtin_prompt_catalog().blocks["memory"].text,
        a[2]["content"]
    );
    assert_eq!("SHARED_BEHAVIOR", a[3]["content"]);
    for (messages, policy, workspace) in [
        (a, "POLICY_A", workspace_a_alias.as_path()),
        (b, "POLICY_B", workspace_b_alias.as_path()),
    ] {
        let project = messages[4]["content"].as_str().unwrap();
        assert!(project.contains(policy));
        let rendered_workspace = project
            .lines()
            .find_map(|line| line.strip_prefix("- Workspace root: "))
            .expect("project context names its workspace root");
        let expected_workspace = workspace.canonicalize().unwrap();
        assert_eq!(
            expected_workspace.components().collect::<Vec<_>>(),
            Path::new(rendered_workspace)
                .components()
                .collect::<Vec<_>>()
        );
    }
    assert_ne!(a[4], b[4]);
    assert_eq!(&a[5..], &b[5..]);
}

#[tokio::test]
async fn workspace_override_is_recorded_and_keeps_the_frozen_prefix_on_reopen() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = workspace.path().canonicalize().unwrap();
    let override_path = root.join("AGENTS.override.md");
    fs::write(root.join("AGENTS.md"), "BASE_AGENTS").unwrap();
    fs::write(root.join("CLAUDE.md"), "BASE_CLAUDE").unwrap();
    fs::write(root.join(".gitignore"), "AGENTS.override.md\n").unwrap();
    fs::write(&override_path, "FIRST_OVERRIDE").unwrap();
    let expected = json!([{
        "path": override_path,
        "hash": format!("sha256:{:x}", Sha256::digest(b"FIRST_OVERRIDE")),
    }]);
    let (options, path) = options(&server, &root, data.path());
    let model = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .matches(common::capture_http_request);
            then.status(200)
                .body(response(json!({"content":"done"}), "stop"));
        })
        .await;
    kuku::query("standalone")
        .session("standalone-override")
        .workspace(&root)
        .kuku_home(data.path())
        .config(options.config.clone())
        .run()
        .await
        .unwrap();
    let journal_path =
        kuku::session::session_events_path(data.path(), &root, "standalone-override").unwrap();
    let journal = kuku::event::EventStore::replay(&journal_path).unwrap();
    assert!(journal.iter().any(|event| matches!(
        &event.payload,
        kuku::event::EventPayload::ContextSources { project_instruction_sources, .. }
            if json!(project_instruction_sources) == expected
    )));

    let session = Session::open(options.clone()).await.unwrap();
    session.create_agent("create", "main", agent()).unwrap();
    session.send(Input::new("first", "main", "first")).unwrap();
    idle(&session, "main").await;
    let mut reopen = options.clone();
    reopen.session_id = Some(session.id().into());
    session.close().await.unwrap();

    fs::write(&override_path, "UPDATED_OVERRIDE").unwrap();
    let reopened = Session::open(reopen).await.unwrap();
    reopened
        .send(Input::new("second", "main", "second"))
        .unwrap();
    idle(&reopened, "main").await;
    let events = reopened.events(0, 256).unwrap();
    let segments = facts(&events, "runtime.context_segment", "main");
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0]["project_instruction_sources"], expected);
    reopened.close().await.unwrap();

    let fresh = Session::open(options).await.unwrap();
    fresh.create_agent("create", "main", agent()).unwrap();
    fresh.send(Input::new("fresh", "main", "fresh")).unwrap();
    idle(&fresh, "main").await;
    fresh.close().await.unwrap();

    model.assert_hits_async(4).await;
    let bodies = common::take_http_requests(&path);
    for (body, source) in bodies.iter().zip([
        "FIRST_OVERRIDE",
        "FIRST_OVERRIDE",
        "FIRST_OVERRIDE",
        "UPDATED_OVERRIDE",
    ]) {
        let encoded = body.to_string();
        assert!(encoded.contains(source), "{encoded}");
        assert!(!encoded.contains("BASE_AGENTS") && !encoded.contains("BASE_CLAUDE"));
    }
    let before = bodies[1]["messages"].as_array().unwrap();
    let after = bodies[2]["messages"].as_array().unwrap();
    assert_eq!(before, &after[..before.len()]);
}
