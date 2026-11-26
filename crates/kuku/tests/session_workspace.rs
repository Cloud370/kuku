mod common;
#[path = "session_workspace/history_restore.rs"]
mod history_restore;
#[path = "session_workspace/media.rs"]
mod media;
#[path = "session_workspace/retained_return.rs"]
mod retained_return;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

use kuku::event::{EventPayload, ModelStopReason};
use kuku::execution::{WorkspaceActionReceipt, WorkspaceActionState};
use kuku::permission::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer};
use kuku::{
    AdapterFuture, AgentConfig, AgentStatus, Input, ModelAdapter, ModelEvent, ModelRequest,
    ModelStream, Session, SessionOptions, WorkspaceExecutionHost, WorkspaceScriptActionStatus,
    WorkspaceScriptFacts, WorkspaceScriptOperation,
};
use serde_json::{Value, json};

#[derive(Debug)]
struct ScriptModel {
    code: &'static str,
    repeated_id_code: Option<&'static str>,
}

impl ModelAdapter for ScriptModel {
    fn stream(&self, request: ModelRequest) -> AdapterFuture<'_, ModelStream> {
        Box::pin(async move {
            assert_eq!(kuku::config::ProviderFormat::Anthropic, request.protocol);
            let completed = request.body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|message| message["role"] == "user")
                .filter_map(|message| message["content"].as_array())
                .flatten()
                .any(|block| block["type"] == "tool_result" && block["tool_use_id"] == "script");
            let code = if completed {
                self.repeated_id_code
            } else {
                Some(self.code)
            };
            let mut events = vec![ModelEvent::StreamStart {
                request_id: "workspace-test".into(),
            }];
            if let Some(code) = code {
                events.extend([
                    ModelEvent::ToolCallStart {
                        index: 0,
                        id: "script".into(),
                        name: "run_js".into(),
                    },
                    ModelEvent::ToolCallArgDelta {
                        index: 0,
                        fragment: json!({"code":code}).to_string(),
                    },
                    ModelEvent::ToolCallStop { index: 0 },
                    ModelEvent::StopReason {
                        reason: ModelStopReason::ToolUse,
                    },
                ]);
            } else {
                events.extend([
                    ModelEvent::TextDelta {
                        text: "done".into(),
                    },
                    ModelEvent::StopReason {
                        reason: ModelStopReason::EndTurn,
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
            if action.tool.name == "write_file"
                && action.args["path"].as_str().map(std::path::Path::new)
                    == Some(std::path::Path::new("denied/nested/forbidden.txt"))
            {
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
    options.model_adapter = Some(Arc::new(ScriptModel {
        code,
        repeated_id_code: None,
    }));
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

#[tokio::test]
async fn script_search_obeys_the_sessions_native_scan_budget() {
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(work.path().join("fixture.txt"), "needle\n".repeat(32)).unwrap();
    let mut options = options(
        &work,
        &data,
        "const r = await tools.search_text({pattern:'needle'}); return r.result.scan_complete;",
    );
    options.limits.workspace_io.scan_bytes = 8;

    let session = start(options).await;
    let facts = until(|| script_facts(&session)).await;
    assert_eq!(facts.value, json!(false));
    session.close().await.unwrap();
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
const one = await tools.write_file({path:"docs/guide/allowed.txt",content:"before"});
const two = await tools.write_file({path:"unused/../denied/nested/forbidden.txt",content:"denied"});
const read = await tools.read_file({path:"docs/guide/allowed.txt"});
await tools.edit_file({path:"docs/guide/allowed.txt",old_text:read.result.raw_text,new_text:"after"});
return {denied:two.status,first:one.status};
"#,
    ))
    .await;
    let facts = until(|| script_facts(&session)).await;
    assert_eq!(json!({"denied":"blocked","first":"ok"}), facts.value);
    assert!(
        facts
            .actions
            .iter()
            .any(|action| action.tool == WorkspaceScriptOperation::WriteFile
                && action.status == WorkspaceScriptActionStatus::Blocked)
    );
    assert_eq!(
        "after",
        std::fs::read_to_string(work.path().join("docs/guide/allowed.txt")).unwrap()
    );
    assert!(!work.path().join("denied").exists());
    assert!(!work.path().join("unused").exists());
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
  const result = await tools.write_file({path:`part-${i}`,content:String(i)});
  if (result.status !== "ok") throw new Error(result.error);
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
async fn deferred_script_resumes_the_same_worker_and_revalidates_the_write() {
    for stale in [false, true] {
        let work = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::write(work.path().join("approval.txt"), "before").unwrap();
        let options = options(
            &work,
            &data,
            r#"
await tools.write_file({path:"prefix",content:"once"});
await tools.read_file({path:"approval.txt"});
const edit = await tools.edit_file({path:"approval.txt", old_text:"before",new_text:"approved"});
if (edit.status === "ok") await tools.write_file({path:"after-approval",content:"continued"});
return edit.status;
"#,
        );
        let host = options.workspace_host.clone();
        let session = start(options).await;
        let request = until(|| session.pending_permissions().pop()).await;
        assert_eq!("edit_file", request.tool);
        assert_eq!(1, host.metrics().active_workers);
        assert!(!work.path().join("after-approval").exists());
        if stale {
            std::fs::write(work.path().join("approval.txt"), "external").unwrap();
        }
        let receipt = session
            .reply_permission("allow", "main", &request.id, true)
            .unwrap();
        assert_eq!(
            receipt,
            session
                .reply_permission("allow", "main", &request.id, true)
                .unwrap()
        );
        let facts = until(|| script_facts(&session)).await;
        assert_eq!(1, host.metrics().started_workers);
        assert_eq!(facts.value == "ok", !stale);
        assert_eq!(
            if stale { "external" } else { "approved" },
            std::fs::read_to_string(work.path().join("approval.txt")).unwrap()
        );
        assert_eq!(work.path().join("after-approval").exists(), !stale);
        assert_eq!(completed_prefix_writes(&session), 1);
        session.close().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_a_script_approval_retains_completed_actions_after_reopen() {
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(work.path().join("approval.txt"), "before").unwrap();
    let mut options = options(
        &work,
        &data,
        r#"
await tools.write_file({path:"prefix",content:"done"});
await tools.read_file({path:"approval.txt"});
await tools.edit_file({path:"approval.txt",old_text:"before",new_text:"changed"});
"#,
    );
    options.session_id = Some("cancelapproval".into());
    let session = start(options.clone()).await;
    let request = until(|| session.pending_permissions().pop()).await;
    session.close().await.unwrap();
    let reopened = Session::open(options).await.unwrap();
    assert!(reopened.pending_permissions().is_empty());
    assert!(matches!(
        reopened.reply_permission("stale", "main", &request.id, true),
        Err(kuku::Error::PermissionRequestNotPending(_))
    ));
    assert_eq!(
        std::fs::read_to_string(work.path().join("approval.txt")).unwrap(),
        "before"
    );
    assert_eq!(completed_prefix_writes(&reopened), 1);
    reopened.close().await.unwrap();
}

fn completed_prefix_writes(session: &Session) -> usize {
    session
        .events(0, 256)
        .unwrap()
        .iter()
        .filter(|event| {
            matches!(&event.payload,
            EventPayload::Unknown(value) if value["kind"] == "workspace.action"
                && value["target"] == "prefix" && value["status"] == "ok")
        })
        .count()
}

#[tokio::test]
async fn stopped_execution_rejects_approval_before_its_worker_observes_cancellation() {
    use std::future::{Future, poll_fn};
    use std::task::Poll;

    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(work.path().join("approval.txt"), "before").unwrap();
    let session = start(options(
        &work,
        &data,
        r#"
await tools.read_file({path:"approval.txt"});
await tools.edit_file({path:"approval.txt",old_text:"before",new_text:"changed"});
"#,
    ))
    .await;
    let request = until(|| session.pending_permissions().pop()).await;
    let mut stop = std::pin::pin!(session.cancel("main"));
    poll_fn(|context| {
        assert!(stop.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    assert_eq!(AgentStatus::Stopped, session.agent("main").unwrap().status);
    assert!(matches!(
        session.reply_permission("approve-after-stop", "main", &request.id, true),
        Err(kuku::Error::PermissionRequestNotPending(_))
    ));
    stop.await.unwrap();
    assert_eq!(
        "before",
        std::fs::read_to_string(work.path().join("approval.txt")).unwrap()
    );
    session.close().await.unwrap();
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
for (let i=1;i<=40;i++) await tools.write_file({path:`part-${i}`,content:String(i)});
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn derived_output_is_streamed_and_abandoned_drafts_release_their_storage() {
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut options = options(
        &work,
        &data,
        r#"
const original = (await tools.create_output({})).result.id;
await tools.append_output({id:original, text:"line 甲\nline 乙\n"});
await tools.finish_output({id:original});
const output = (await tools.create_output({})).result.id;
for await (const {text} of tools.output_lines({id:original})) {
  await tools.append_output({id:output, text:text.toUpperCase()+"\n"});
}
const saved = await tools.finish_output({id:output});
const bound = await tools.save_value({name:"upper",output_id:saved.result.output_ref.id});
if (bound.status!=="ok") throw new Error(bound.error);
const names = await tools.list_values({});
if (names.status!=="ok") throw new Error(names.error);
const draft = (await tools.create_output({})).result.id;
await tools.append_output({id:draft, text:"x".repeat(32768)});
return {original, saved:saved.result.output_ref.id, draft};
"#,
    );
    options.limits.max_output_bytes = 96 * 1024;
    options.limits.max_retained_output_bytes = 196 * 1024;
    let session = start(options).await;
    let facts = until(|| script_facts(&session)).await;
    let content = |name: &str| {
        let id = facts.value[name].as_str().unwrap();
        session
            .read_agent_output("main", id, &kuku::OutputRead::default())
            .unwrap()
            .segments
            .into_iter()
            .map(|segment| segment.text)
            .collect::<String>()
    };
    assert_eq!("line 甲\nline 乙\n", content("original"));
    assert_eq!("LINE 甲\nLINE 乙\n", content("saved"));
    assert_eq!(
        session.named_value("main", "upper").unwrap().reference.id,
        facts.value["saved"].as_str().unwrap()
    );
    let draft = facts.value["draft"].as_str().unwrap();
    assert!(
        session
            .read_agent_output("main", draft, &kuku::OutputRead::default())
            .is_err()
    );
    assert_eq!(
        2,
        std::fs::read_dir(session.events_path().parent().unwrap().join("outputs"))
            .unwrap()
            .count()
    );
    session.close().await.unwrap();
}

#[derive(Debug, Default)]
struct DraftGate {
    creations: AtomicUsize,
    finishes: AtomicUsize,
}

impl ToolAuthorizer for DraftGate {
    fn authorize(&self, action: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        let decision = match action.tool.name.as_str() {
            "create_output" if self.creations.fetch_add(1, Ordering::Relaxed) == 1 => {
                AuthorizationDecision::Defer
            }
            "finish_output" if self.finishes.fetch_add(1, Ordering::Relaxed) == 1 => {
                AuthorizationDecision::Defer
            }
            "append_output" if action.args["text"] == "deferred" => AuthorizationDecision::Defer,
            "append_output" if action.args["text"] == "denied" => AuthorizationDecision::Deny,
            _ => AuthorizationDecision::Allow,
        };
        Ok(decision)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn denied_draft_operations_leave_already_published_output_intact() {
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut options = options(
        &work,
        &data,
        r#"
const base = (await tools.create_output({})).result.id;
await tools.append_output({id:base,text:"previously published"});
await tools.finish_output({id:base});
const create = await tools.create_output({});
const id = (await tools.create_output({})).result.id;
const append = await tools.append_output({id,text:"deferred"});
const deny = await tools.append_output({id,text:"denied"});
const allowed = await tools.append_output({id,text:"allowed"});
const finish = await tools.finish_output({id});
const published = await tools.finish_output({id});
return {base, id, statuses:[create.status,append.status,deny.status,allowed.status,finish.status,published.status]};
"#,
    );
    options.authorizer = Arc::new(DraftGate::default());
    let session = start(options).await;
    for (index, tool) in ["create_output", "append_output", "finish_output"]
        .into_iter()
        .enumerate()
    {
        let request = until(|| session.pending_permissions().pop()).await;
        assert_eq!(tool, request.tool);
        session
            .reply_permission(&format!("deny-{index}"), "main", &request.id, false)
            .unwrap();
    }
    let facts = until(|| script_facts(&session)).await;
    assert_eq!(
        json!(["blocked", "blocked", "blocked", "ok", "blocked", "ok"]),
        facts.value["statuses"]
    );
    for (field, expected) in [("base", "previously published"), ("id", "allowed")] {
        let id = facts.value[field].as_str().unwrap();
        let text = session
            .read_agent_output("main", id, &kuku::OutputRead::default())
            .unwrap()
            .segments
            .into_iter()
            .map(|segment| segment.text)
            .collect::<String>();
        assert_eq!(expected, text);
    }
    let events = session.events(0, 256).unwrap();
    let actions = events
        .iter()
        .filter_map(|event| WorkspaceActionReceipt::from_event(event).unwrap())
        .collect::<Vec<_>>();
    let blocked = actions
        .iter()
        .filter(|action| action.status == WorkspaceActionState::Blocked)
        .collect::<Vec<_>>();
    assert_eq!(4, blocked.len());
    for action in blocked {
        assert!(
            !actions
                .iter()
                .any(|candidate| candidate.action_id == action.action_id
                    && candidate.status == WorkspaceActionState::Dispatched)
        );
    }
    session.close().await.unwrap();
    let events = session.events(0, 256).unwrap();
    assert_eq!(
        1,
        events
            .iter()
            .filter(|event| matches!(&event.payload,
                EventPayload::ToolCall { tool_call_id, .. } if tool_call_id == "script"
            ))
            .count()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reusing_a_tool_call_id_in_the_next_model_step_cannot_redispatch_the_old_action() {
    const CODE: &str = r#"return await tools.write_file({path:"original",content:"once"});"#;
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut options = options(&work, &data, CODE);
    options.model_adapter = Some(Arc::new(ScriptModel {
        code: CODE,
        repeated_id_code: Some(
            r#"return await tools.write_file({path:"replacement",content:"invalid call"});"#,
        ),
    }));
    let host = options.workspace_host.clone();
    let session = start(options).await;
    let agent = until(|| {
        let agent = session.agent("main").unwrap();
        (agent.status != AgentStatus::Running).then_some(agent)
    })
    .await;
    assert_eq!(AgentStatus::Interrupted, agent.status);
    assert!(
        agent
            .last_error
            .unwrap()
            .contains("The model reused a tool call ID.")
    );
    assert_eq!(1, host.metrics().started_workers);
    assert_eq!(
        "once",
        std::fs::read_to_string(work.path().join("original")).unwrap()
    );
    assert!(!work.path().join("replacement").exists());

    let events = session.events(0, 256).unwrap();
    let calls = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ToolCall {
                tool_call_id,
                request_id,
                ..
            } if tool_call_id == "script" => Some(request_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(vec!["req_1"], calls);
    let mutations = events
        .iter()
        .filter_map(|event| WorkspaceActionReceipt::from_event(event).unwrap())
        .filter(|event| {
            event.tool == WorkspaceScriptOperation::WriteFile
                && event.status == WorkspaceActionState::Dispatched
        })
        .collect::<Vec<_>>();
    assert_eq!(1, mutations.len());
    session.close().await.unwrap();
}

type FileCall = (&'static str, &'static str, Value);

#[derive(Debug)]
struct FileToolModel {
    steps: Mutex<VecDeque<Vec<FileCall>>>,
    requests: Option<mpsc::UnboundedSender<ModelRequest>>,
}

impl FileToolModel {
    fn new(steps: impl Into<VecDeque<Vec<FileCall>>>) -> Self {
        Self {
            steps: Mutex::new(steps.into()),
            requests: None,
        }
    }
}

impl ModelAdapter for FileToolModel {
    fn stream(&self, request: ModelRequest) -> AdapterFuture<'_, ModelStream> {
        Box::pin(async move {
            if let Some(requests) = &self.requests {
                requests.send(request).unwrap();
            }
            let step = self
                .steps
                .lock()
                .unwrap()
                .pop_front()
                .expect("fixture model step");
            let mut events = vec![ModelEvent::StreamStart {
                request_id: "file-test".into(),
            }];
            if step.is_empty() {
                events.extend([
                    ModelEvent::TextDelta {
                        text: "done".into(),
                    },
                    ModelEvent::StopReason {
                        reason: ModelStopReason::EndTurn,
                    },
                ]);
            } else {
                for (index, (id, name, args)) in step.into_iter().enumerate() {
                    let index = index as u64;
                    events.extend([
                        ModelEvent::ToolCallStart {
                            index,
                            id: id.into(),
                            name: name.into(),
                        },
                        ModelEvent::ToolCallArgDelta {
                            index,
                            fragment: args.to_string(),
                        },
                        ModelEvent::ToolCallStop { index },
                    ]);
                }
                events.push(ModelEvent::StopReason {
                    reason: ModelStopReason::ToolUse,
                });
            }
            events.push(ModelEvent::StreamEnd);
            Ok(Box::pin(tokio_stream::iter(events.into_iter().map(Ok))) as ModelStream)
        })
    }
}

fn file_tool_options(
    work: &tempfile::TempDir,
    data: &tempfile::TempDir,
    steps: Vec<Vec<FileCall>>,
) -> SessionOptions {
    let mut options = SessionOptions::new(work.path(), data.path(), common::test_config());
    options.model_adapter = Some(Arc::new(FileToolModel::new(steps)));
    options
}

fn write_owned(id: &'static str, content: &str) -> FileCall {
    (
        id,
        "write_file",
        json!({"path":"owned.txt","content":content}),
    )
}

fn edit_owned(id: &'static str, old_text: &str, new_text: &str) -> FileCall {
    (
        id,
        "edit_file",
        json!({"path":"owned.txt","old_text":old_text,"new_text":new_text}),
    )
}

async fn completed_turns(session: &Session, count: usize) {
    until(|| {
        (session
            .events(0, 256)
            .unwrap()
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::TurnCompleted { .. }))
            .count()
            == count)
            .then_some(())
    })
    .await;
}

fn result_for(session: &Session, id: &str) -> (String, Value) {
    session
        .events(0, 256)
        .unwrap()
        .into_iter()
        .find_map(|event| match event.payload {
            EventPayload::ToolResult {
                tool_call_id,
                status,
                structured: Some(data),
                ..
            } if tool_call_id == id => Some((status, data)),
            _ => None,
        })
        .expect("tool result")
}

#[tokio::test]
async fn direct_file_read_uses_a_larger_model_page_without_expanding_search() {
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let text = "x".repeat(80);
    let mut source = String::new();
    for n in 1..=400 {
        use std::fmt::Write;
        writeln!(source, "needle-{n:03}: {text}").unwrap();
    }
    std::fs::write(work.path().join("fixture.txt"), source).unwrap();

    let (requests, mut received) = mpsc::unbounded_channel();
    let mut options = SessionOptions::new(work.path(), data.path(), common::test_config());
    options.model_adapter = Some(Arc::new(FileToolModel {
        requests: Some(requests),
        ..FileToolModel::new([
            vec![
                (
                    "read",
                    "read_file",
                    json!({"path":"fixture.txt","limit":300}),
                ),
                (
                    "search",
                    "search_text",
                    json!({"path":"fixture.txt","pattern":"needle","limit":400}),
                ),
            ],
            vec![],
        ])
    }));
    let session = start(options).await;
    completed_turns(&session, 1).await;
    let _first = received.try_recv().unwrap();
    let next = received.try_recv().unwrap();
    let output = |id: &str| {
        next.body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|message| message["content"].as_array())
            .flatten()
            .find(|block| block["type"] == "tool_result" && block["tool_use_id"] == id)
            .map(|block| {
                assert_eq!(false, block["is_error"]);
                block["content"].as_str().unwrap()
            })
            .expect("tool output sent to the model")
    };
    let read = output("read");
    assert!((8 * 1024 + 1..=32 * 1024).contains(&read.len()));
    assert_eq!(300, read.lines().count());
    assert!(read.ends_with(&format!("300\tneedle-300: {text}")));
    assert_eq!(301, result_for(&session, "read").1["next_offset"]);

    let search = output("search");
    assert!(!search.is_empty() && search.len() <= 8 * 1024);
    assert!(!search.contains("needle-400"));
    session.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_writer_can_overwrite_after_reopen_and_edit_its_new_version() {
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let session = start(file_tool_options(
        &work,
        &data,
        vec![vec![write_owned("seed", "first")], vec![]],
    ))
    .await;
    completed_turns(&session, 1).await;
    let id = session.id().to_owned();
    session.close().await.unwrap();

    let prefix_lines = 12_000;
    let prefix = "prefix\n".repeat(prefix_lines);
    let mut options = file_tool_options(
        &work,
        &data,
        vec![
            vec![write_owned(
                "rewrite",
                &format!("{prefix}full second version"),
            )],
            vec![edit_owned("edit", "second", "third")],
            vec![edit_owned("follow-up-edit", "third", "fourth")],
            vec![(
                "read",
                "read_file",
                json!({"path":"owned.txt", "offset":prefix_lines + 1}),
            )],
            vec![],
        ],
    );
    options.session_id = Some(id);
    let reopened = Session::open(options).await.unwrap();
    reopened
        .send(Input::new("second-input", "main", "update the file"))
        .unwrap();
    completed_turns(&reopened, 2).await;
    let (status, write_facts) = result_for(&reopened, "rewrite");
    assert_eq!("ok", status);
    assert_eq!(true, write_facts["raw_text_after_truncated"]);
    assert_eq!("ok", result_for(&reopened, "edit").0);
    assert_eq!("ok", result_for(&reopened, "follow-up-edit").0);
    let (_, read_facts) = result_for(&reopened, "read");
    assert_eq!(false, read_facts["cached"]);
    assert_eq!("full fourth version", read_facts["raw_text"]);
    assert_eq!(
        format!("{prefix}full fourth version"),
        std::fs::read_to_string(work.path().join("owned.txt")).unwrap()
    );
    reopened.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_writer_evidence_checks_the_actor_and_current_digest() {
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let session = start(file_tool_options(
        &work,
        &data,
        vec![
            vec![write_owned("seed", "my version")],
            vec![],
            vec![write_owned("other-agent-write", "different actor")],
            vec![],
            vec![write_owned("stale-write", "unexpected overwrite")],
            vec![],
        ],
    ))
    .await;
    completed_turns(&session, 1).await;
    session
        .create_agent("create-reviewer", "reviewer", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new(
            "review",
            "reviewer",
            "try to overwrite the file",
        ))
        .unwrap();
    completed_turns(&session, 2).await;
    assert_eq!(
        "full_snapshot_required",
        result_for(&session, "other-agent-write").1["reason_code"]
    );

    std::fs::write(work.path().join("owned.txt"), "external").unwrap();
    session
        .send(Input::new("update", "main", "try to overwrite the file"))
        .unwrap();
    completed_turns(&session, 3).await;
    assert_eq!(
        "snapshot_stale",
        result_for(&session, "stale-write").1["reason_code"]
    );
    assert_eq!(
        "external",
        std::fs::read_to_string(work.path().join("owned.txt")).unwrap()
    );
    session.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_replace_all_can_reuse_a_write_after_the_model_receives_confirmation() {
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut before = edit_owned("before-confirmation", "first", "premature");
    before.2["replace_all"] = json!(true);
    let mut after = edit_owned("after-confirmation", "first", "confirmed");
    after.2["replace_all"] = json!(true);
    let session = start(file_tool_options(
        &work,
        &data,
        vec![
            vec![write_owned("seed", "first\nfirst\n"), before],
            vec![after],
            vec![],
        ],
    ))
    .await;
    completed_turns(&session, 1).await;
    assert_eq!("ok", result_for(&session, "seed").0);
    assert_eq!(
        "full_snapshot_required",
        result_for(&session, "before-confirmation").1["reason_code"]
    );
    assert_eq!("ok", result_for(&session, "after-confirmation").0);
    assert_eq!(
        "confirmed\nconfirmed\n",
        std::fs::read_to_string(work.path().join("owned.txt")).unwrap()
    );
    session.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_disjoint_edits_preserve_unrelated_changes_without_rereading() {
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let path = work.path().join("owned.txt");
    std::fs::write(&path, "header\nalpha\nbeta\ngamma\ndelta\n").unwrap();

    let model = Arc::new(FileToolModel::new([
        vec![
            edit_owned("batch-first", "alpha", "one"),
            edit_owned("batch-second", "beta", "two"),
        ],
        vec![edit_owned("later-first", "gamma", "three")],
        vec![edit_owned("later-second", "delta", "four")],
        vec![],
    ]));
    let mut options = SessionOptions::new(work.path(), data.path(), common::test_config());
    options.model_adapter = Some(model.clone());
    let session = start(options).await;
    completed_turns(&session, 1).await;
    for id in ["batch-first", "batch-second", "later-first", "later-second"] {
        let (status, facts) = result_for(&session, id);
        assert_eq!("ok", status, "{id}: {facts}");
        assert_eq!(1, facts["replacement_count"]);
    }
    assert_eq!(
        "header\none\ntwo\nthree\nfour\n",
        std::fs::read_to_string(&path).unwrap()
    );

    let mut pinned = edit_owned("pinned", "two", "rejected");
    pinned.2["expected_version"] = result_for(&session, "later-second").1["content_hash"].clone();
    std::fs::write(&path, "updated header\none\ntwo\nthree\nfour\n").unwrap();
    model.steps.lock().unwrap().extend([
        vec![pinned],
        vec![edit_owned("unpinned", "two", "second")],
        vec![],
    ]);
    session
        .send(Input::new("follow-up", "main", "update the second value"))
        .unwrap();
    completed_turns(&session, 2).await;
    let (status, facts) = result_for(&session, "pinned");
    assert_eq!("error", status);
    assert_eq!("snapshot_stale", facts["reason_code"]);
    assert_eq!("ok", result_for(&session, "unpinned").0);
    assert_eq!(
        "updated header\none\nsecond\nthree\nfour\n",
        std::fs::read_to_string(&path).unwrap()
    );
    session.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_single_edits_work_across_actions_and_executions() {
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let session = start(file_tool_options(
        &work,
        &data,
        vec![
            vec![(
                "script",
                "run_js",
                json!({"code":r#"
const first = await tools.write_file({path:"owned.txt",content:"alpha"});
const second = await tools.write_file({path:"owned.txt",content:"beta"});
const edit = await tools.edit_file({path:"owned.txt",old_text:"beta",new_text:"gamma"});
const followUp = await tools.edit_file({path:"owned.txt",old_text:"gamma",new_text:"delta"});
const independent = await tools.write_file({path:"vm.txt",content:"observed"});
return [first.status,second.status,edit.status,followUp.status,independent.status];
"#}),
            )],
            vec![(
                "new-vm",
                "run_js",
                json!({"code":r#"
const overwrite = await tools.write_file({path:"vm.txt",content:"rejected"});
const edit = await tools.edit_file({path:"vm.txt",old_text:"observed",new_text:"different VM"});
return [overwrite.result.reason_code,edit.status];
"#}),
            )],
            vec![],
        ],
    ))
    .await;
    completed_turns(&session, 1).await;
    assert_eq!(
        json!(["ok", "ok", "ok", "ok", "ok"]),
        script_facts(&session).unwrap().value
    );
    let facts = WorkspaceScriptFacts::from_structured(&result_for(&session, "new-vm").1).unwrap();
    assert_eq!(json!(["snapshot_required", "ok"]), facts.value);
    assert_eq!(
        "delta",
        std::fs::read_to_string(work.path().join("owned.txt")).unwrap()
    );
    assert_eq!(
        "different VM",
        std::fs::read_to_string(work.path().join("vm.txt")).unwrap()
    );
    session.close().await.unwrap();
}
