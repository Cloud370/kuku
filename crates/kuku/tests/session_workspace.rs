mod common;

use std::sync::Arc;
use std::time::Duration;

use kuku::event::{EventPayload, ModelStopReason};
use kuku::execution::{WorkspaceActionReceipt, WorkspaceActionState};
use kuku::permission::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer};
use kuku::{
    AdapterFuture, AgentConfig, Input, ModelAdapter, ModelEvent, ModelRequest, ModelStream,
    Session, SessionOptions, WorkspaceExecutionHost, WorkspaceScriptActionStatus,
    WorkspaceScriptFacts, WorkspaceScriptOperation,
};
use serde_json::json;

#[derive(Debug)]
struct ScriptModel(&'static str);

impl ModelAdapter for ScriptModel {
    fn stream(&self, request: ModelRequest) -> AdapterFuture<'_, ModelStream> {
        Box::pin(async move {
            let completed = request.body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["role"] == "tool");
            let mut events = vec![ModelEvent::StreamStart {
                request_id: "workspace-test".into(),
            }];
            if completed {
                events.extend([
                    ModelEvent::TextDelta {
                        text: "done".into(),
                    },
                    ModelEvent::StopReason {
                        reason: ModelStopReason::EndTurn,
                    },
                ]);
            } else {
                events.extend([
                    ModelEvent::ToolCallStart {
                        index: 0,
                        id: "script".into(),
                        name: "workspace_script".into(),
                    },
                    ModelEvent::ToolCallArgDelta {
                        index: 0,
                        fragment: json!({"code":self.0}).to_string(),
                    },
                    ModelEvent::ToolCallStop { index: 0 },
                    ModelEvent::StopReason {
                        reason: ModelStopReason::ToolUse,
                    },
                ]);
            }
            events.push(ModelEvent::StreamEnd);
            Ok(Box::pin(tokio_stream::iter(events.into_iter().map(Ok))) as ModelStream)
        })
    }
}

#[derive(Debug)]
struct Gate;
impl ToolAuthorizer for Gate {
    fn authorize(&self, action: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        assert_eq!(action.agent, "main");
        Ok(
            if action.tool.name == "write_file" && action.args["path"] == "forbidden.txt" {
                AuthorizationDecision::Deny
            } else if action.tool.name == "edit_file" && action.args["path"] == "approval.txt" {
                AuthorizationDecision::Defer
            } else {
                AuthorizationDecision::Allow
            },
        )
    }
}

fn options(
    work: &tempfile::TempDir,
    data: &tempfile::TempDir,
    code: &'static str,
) -> SessionOptions {
    let mut options = SessionOptions::new(work.path(), data.path(), common::test_config());
    options.model_adapter = Some(Arc::new(ScriptModel(code)));
    options.authorizer = Arc::new(Gate);
    options.workspace_host = WorkspaceExecutionHost::new(1, 2).unwrap();
    options
}

async fn start(options: SessionOptions) -> Session {
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("new", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("input", "main", "execute workspace task"))
        .unwrap();
    session
}

async fn until<T>(mut probe: impl FnMut() -> Option<T>) -> T {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(value) = probe() {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("public Session did not reach expected state")
}

fn script_facts(session: &Session) -> Option<WorkspaceScriptFacts> {
    session
        .events(0, 256)
        .unwrap()
        .into_iter()
        .find_map(|event| match event.payload {
            EventPayload::ToolResult {
                tool_call_id,
                structured: Some(value),
                summary,
                ..
            } if tool_call_id == "script" => Some(
                WorkspaceScriptFacts::from_structured(&value).unwrap_or_else(|err| {
                    panic!("could not decode SDK result ({summary}): {err}: {value}")
                }),
            ),
            _ => None,
        })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_checks_final_authority_and_keeps_host_outcomes_after_js_returns() {
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let session = start(options(
        &work,
        &data,
        r#"
const one = await workspace.write_file({path:"allowed.txt",content:"before"});
const two = await workspace.write_file({path:"forbidden.txt",content:"denied"});
const read = await workspace.read_file({path:"allowed.txt"});
await workspace.edit_file({path:"allowed.txt",old_text:read.result.raw_text,new_text:"after"});
return {denied:two.status,first:one.status};
"#,
    ))
    .await;
    let facts = until(|| script_facts(&session)).await;
    assert_eq!(json!({"denied":"blocked","first":"ok"}), facts.value);
    assert!(facts
        .actions
        .iter()
        .any(|action| action.tool == WorkspaceScriptOperation::WriteFile
            && action.status == WorkspaceScriptActionStatus::Blocked));
    assert_eq!(
        "after",
        std::fs::read_to_string(work.path().join("allowed.txt")).unwrap()
    );
    assert!(!work.path().join("forbidden.txt").exists());
    session.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_after_twelve_writes_preserves_exact_receipts_on_reopen() {
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let options = options(
        &work,
        &data,
        r#"
for (let i=1; i<=40; i++) {
  const result = await workspace.write_file({path:`part-${i}`,content:String(i)});
  if (result.status !== "ok") throw new Error(result.summary);
  if (i === 12) while(true) {}
}
"#,
    );
    let host = options.workspace_host.clone();
    let session = start(options.clone()).await;
    let id = session.id().to_owned();
    until(|| work.path().join("part-12").exists().then_some(())).await;
    session.cancel("main").await.unwrap();
    assert_eq!(0, host.metrics().active_workers);
    session.close().await.unwrap();
    let mut options = options;
    options.session_id = Some(id);
    let session = Session::open(options).await.unwrap();
    let mut writes = Vec::new();
    for event in session.events(0, 256).unwrap() {
        if let Some(fact) = WorkspaceActionReceipt::from_event(&event).unwrap() {
            if fact.tool == WorkspaceScriptOperation::WriteFile
                && fact.status == WorkspaceActionState::Ok
            {
                assert!(fact.facts.content_hash_after.is_some());
                writes.push(fact.target.unwrap());
            }
        }
    }
    assert_eq!(
        (1..=12).map(|i| format!("part-{i}")).collect::<Vec<_>>(),
        writes
    );
    assert_eq!(
        12,
        std::fs::read_dir(work.path())
            .unwrap()
            .filter_map(|item| item.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("part-"))
            .count()
    );
    session.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deferred_script_action_reopens_without_its_worker_or_replaying_the_prefix() {
    for stale in [false, true] {
        let work = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::write(work.path().join("approval.txt"), "before").unwrap();
        let mut options = options(
            &work,
            &data,
            r#"
await workspace.write_file({path:"prefix",content:"once"});
await workspace.read_file({path:"approval.txt"});
await workspace.edit_file({path:"approval.txt", old_text:"before",new_text:"approved"});
await workspace.write_file({path:"after-approval",content:"should not run"});
"#,
        );
        options.session_id = Some("approvaltest".into());
        let host = options.workspace_host.clone();
        let session = start(options.clone()).await;
        let request = until(|| session.agent("main").unwrap().permission).await;
        assert_eq!("edit_file", request.tool);
        assert_eq!(0, host.metrics().active_workers);
        session.close().await.unwrap();
        if stale {
            std::fs::write(work.path().join("approval.txt"), "external").unwrap();
        }
        let reopened = Session::open(options).await.unwrap();
        reopened
            .reply_permission("allow", "main", &request.id, true)
            .unwrap();
        let facts = until(|| script_facts(&reopened)).await;
        assert_eq!(1, host.metrics().started_workers);
        assert!(
            facts
                .actions
                .iter()
                .any(|action| action.tool == WorkspaceScriptOperation::EditFile
                    && if stale {
                        action.status != WorkspaceScriptActionStatus::Ok
                    } else {
                        action.status == WorkspaceScriptActionStatus::Ok
                    }),
            "{facts:?}"
        );
        assert_eq!(
            if stale { "external" } else { "approved" },
            std::fs::read_to_string(work.path().join("approval.txt")).unwrap()
        );
        assert!(!work.path().join("after-approval").exists());
        let writes=reopened.events(0,256).unwrap().into_iter().filter(|event|matches!(&event.payload,
            EventPayload::Unknown(value) if value["kind"]=="workspace.action" && value["target"]=="prefix" && value["status"]=="ok")).count();
        assert_eq!(1, writes);
        reopened.close().await.unwrap();
    }
}

#[derive(Debug)]
struct SteerGate {
    reached: std::sync::atomic::AtomicBool,
    resume: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl ToolAuthorizer for SteerGate {
    fn authorize(&self, action: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        if action.tool.name == "write_file" && action.args["path"] == "part-13" {
            self.reached
                .store(true, std::sync::atomic::Ordering::Release);
            self.resume
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        }
        Ok(AuthorizationDecision::Allow)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn next_step_input_stops_further_file_writes_at_dispatch_boundary() {
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (release, resume) = std::sync::mpsc::channel();
    let gate = Arc::new(SteerGate {
        reached: std::sync::atomic::AtomicBool::new(false),
        resume: std::sync::Mutex::new(resume),
    });
    let mut options = options(
        &work,
        &data,
        r#"
for (let i=1;i<=40;i++) await workspace.write_file({path:`part-${i}`,content:String(i)});
"#,
    );
    options.authorizer = gate.clone();
    let session = start(options).await;
    until(|| {
        gate.reached
            .load(std::sync::atomic::Ordering::Acquire)
            .then_some(())
    })
    .await;
    session
        .send(Input::new(
            "change",
            "main",
            "Stop writing. Report what has already been saved.",
        ))
        .unwrap();
    release.send(()).unwrap();
    let facts = until(|| script_facts(&session)).await;
    assert_eq!(
        12,
        facts
            .actions
            .iter()
            .filter(|action| action.tool == WorkspaceScriptOperation::WriteFile
                && action.status == WorkspaceScriptActionStatus::Ok)
            .count()
    );
    assert!(work.path().join("part-12").is_file());
    assert!(!work.path().join("part-13").exists());
    session.close().await.unwrap();
}
