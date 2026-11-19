use std::fs;

use httpmock::prelude::{MockServer, POST};
use serde_json::{json, Value};
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
    fs::write(workspace_a.path().join("AGENTS.md"), "POLICY_A").unwrap();
    fs::write(workspace_b.path().join("AGENTS.md"), "POLICY_B").unwrap();
    let (first, path) = options(&server, workspace_a.path(), data_a.path());
    let mut second = first.clone();
    second.workspace = workspace_b.path().into();
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
    assert_eq!(
        kuku::builtin_prompt_catalog().blocks["tool-guidance"].text,
        a[1]["content"]
    );
    assert_eq!(
        kuku::builtin_prompt_catalog().blocks["memory"].text,
        a[2]["content"]
    );
    assert_eq!("SHARED_BEHAVIOR", a[3]["content"]);
    for (messages, policy, workspace) in [
        (a, "POLICY_A", workspace_a.path()),
        (b, "POLICY_B", workspace_b.path()),
    ] {
        let project = messages[4]["content"].as_str().unwrap();
        assert!(project.contains(policy));
        assert!(project.contains(workspace.to_str().unwrap()));
    }
    assert_ne!(a[4], b[4]);
    assert_eq!(&a[5..], &b[5..]);
}

#[tokio::test]
async fn reopening_a_legacy_segment_preserves_its_recorded_policy_first_prefix() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    fs::write(workspace.path().join("AGENTS.md"), "LEGACY_POLICY").unwrap();
    let (mut original, path) = options(&server, workspace.path(), data.path());
    let model = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .matches(common::capture_http_request);
            then.status(200)
                .body(response(json!({"content":"done"}), "stop"));
        })
        .await;
    let session = Session::open(original.clone()).await.unwrap();
    let mut config = agent();
    config.instructions = Some("LEGACY_BEHAVIOR".into());
    session.create_agent("create", "main", config).unwrap();
    session.send(Input::new("first", "main", "first")).unwrap();
    idle(&session, "main").await;
    original.session_id = Some(session.id().into());
    let journal = session.events_path().to_owned();
    session.close().await.unwrap();
    let mut body = common::take_http_requests(&path).remove(0);
    let mut records = fs::read_to_string(&journal)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let segment = records
        .iter_mut()
        .find(|e| e["kind"] == "runtime.context_segment")
        .unwrap();
    let prelude = segment["prelude"].as_array().unwrap();
    let old = [
        prelude[3].clone(),
        prelude[2].clone(),
        prelude[0].clone(),
        prelude[1].clone(),
    ]
    .into_iter()
    .chain(prelude[4..].iter().cloned())
    .collect::<Vec<_>>();
    segment["prelude"] = json!(old);
    segment["identity_slot"] = json!(1);
    let tools: Vec<kuku::context::ToolSchema> =
        serde_json::from_value(segment["tools"].clone()).unwrap();
    let prelude: Vec<kuku::context::CanonicalMessage> =
        serde_json::from_value(segment["prelude"].clone()).unwrap();
    let prefix_hash = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(&segment["system_prompt"], &tools, &prelude)).unwrap())
    );
    let messages = body["messages"].as_array_mut().unwrap();
    let leading = [
        messages[4].clone(),
        messages[3].clone(),
        messages[1].clone(),
        messages[2].clone(),
    ];
    messages[1..5].clone_from_slice(&leading);
    let raw_body = serde_json::to_vec(&body).unwrap();
    for record in records
        .iter_mut()
        .filter(|e| e["kind"] == "runtime.step_prepared")
    {
        record["prefix_hash"] = json!(prefix_hash);
        record["body_hash"] = json!(format!("{:x}", Sha256::digest(&raw_body)));
        record["request_bytes"] = json!(raw_body.len());
    }
    let fixture = records
        .iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(&journal, fixture).unwrap();
    fs::write(workspace.path().join("AGENTS.md"), "CHANGED_POLICY").unwrap();
    let reopened = Session::open(original).await.unwrap();
    reopened
        .send(Input::new("second", "main", "second"))
        .unwrap();
    idle(&reopened, "main").await;
    let events = reopened.events(0, 256).unwrap();
    reopened.close().await.unwrap();
    model.assert_hits_async(2).await;
    let bodies = common::take_http_requests(&path);
    let old = body["messages"].as_array().unwrap();
    let resumed = bodies[0]["messages"].as_array().unwrap();
    assert_eq!(old, &resumed[..old.len()]);
    assert!(resumed[1]["content"]
        .as_str()
        .unwrap()
        .contains("LEGACY_POLICY"));
    assert_eq!("LEGACY_BEHAVIOR", resumed[2]["content"]);
    assert_eq!(1, facts(&events, "runtime.context_segment", "main").len());
    let prepared = facts(&events, "runtime.step_prepared", "main");
    assert_eq!(json!(prefix_hash), prepared[1]["prefix_hash"]);
    assert_eq!(
        prepared[0]["context_head"],
        prepared[1]["prefix_context_head"]
    );
}
