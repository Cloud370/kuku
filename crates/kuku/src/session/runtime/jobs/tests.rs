use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde_json::json;
use tokio::time::Instant;

use crate::event::{EventPayload, EventStore};
use crate::{Error, JobStatus, Session};

use crate::config::{
    Config, DiscoveryConfig, HandoffConfig, LogsConfig, PluginConfig, UpdateConfig,
};

use super::super::{AgentConfig, SessionOptions};
use super::{JobRequest, Record, timestamp};

fn dispatch_recorded_call(path: &Path, tool: &str, id: &str, turn: u64) -> EventStore {
    let mut store = EventStore::open(path).unwrap();
    store
        .append(EventPayload::ToolCall {
            turn,
            ts: timestamp().unwrap(),
            conversation: Some("main".into()),
            tool_call_id: id.into(),
            request_id: "req_1".into(),
            index: 0,
            tool: tool.into(),
            args: json!({}),
        })
        .unwrap();
    store
        .append(EventPayload::Unknown(json!({
            "kind": "runtime.tool_dispatched", "ts": timestamp().unwrap(), "agent": "main",
            "turn": turn, "tool_call_id": id, "tool": tool,
        })))
        .unwrap();
    store
}

fn session_options(root: &Path) -> SessionOptions {
    let config = Config {
        models: BTreeMap::new(),
        providers: BTreeMap::new(),
        default_model: crate::config::ModelRef::default(),
        discovery: DiscoveryConfig::default(),
        handoff: HandoffConfig::default(),
        logs: LogsConfig::default(),
        plugin: PluginConfig::default(),
        update: UpdateConfig::default(),
    };
    SessionOptions::new(root, root.join("data"), config)
}

#[tokio::test]
async fn reopening_settles_a_script_and_its_dispatched_sibling_before_the_first_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mut options = session_options(dir.path());
    options.session_id = Some("script-recovery".into());
    let session = Session::open(options.clone()).await.unwrap();
    let path = session.events_path().to_owned();
    {
        let mut state = session.inner.state.lock().unwrap();
        session
            .commit(
                &mut state,
                Record::AgentCreated {
                    command_id: "fixture-agent".into(),
                    fingerprint: String::new(),
                    address: "main".into(),
                    config: AgentConfig::default(),
                },
            )
            .unwrap();
    }
    session.close().await.unwrap();
    drop(session);
    dispatch_recorded_call(&path, "run_js", "script", 3);
    dispatch_recorded_call(&path, "query_session", "sibling", 3);

    for _ in 0..2 {
        let reopened = Session::open(options.clone()).await.unwrap();
        let mut results = BTreeMap::new();
        EventStore::visit_after_path(&path, 0, |event| {
            if let EventPayload::ToolResult {
                tool_call_id,
                model_content,
                status,
                ..
            } = event.payload
            {
                assert_eq!("cancelled", status);
                assert!(results.insert(tool_call_id, model_content).is_none());
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(2, results.len());
        assert!(results["script"].contains("Restart the remaining script steps"));
        assert!(results["sibling"].contains("prior effects are unknown"));
        assert!(reopened.pending_permissions().is_empty());
        reopened.close().await.unwrap();
        drop(reopened);
    }
}

#[tokio::test]
async fn reopening_settles_an_unconfirmed_job_once_without_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let mut options = session_options(dir.path());
    options.session_id = Some("job-recovery".into());
    let session = Session::open(options.clone()).await.unwrap();
    let path = session.events_path().to_owned();
    {
        let mut state = session.inner.state.lock().unwrap();
        session
            .commit(
                &mut state,
                Record::AgentCreated {
                    command_id: "fixture-agent".into(),
                    fingerprint: String::new(),
                    address: "main".into(),
                    config: AgentConfig::default(),
                },
            )
            .unwrap();
        let mut store = dispatch_recorded_call(&path, "custom", "unrelated", 3);
        store
            .append(EventPayload::ToolResult {
                turn: 3,
                ts: timestamp().unwrap(),
                conversation: Some("main".into()),
                tool_call_id: "unrelated".into(),
                status: "ok".into(),
                summary: "unrelated output".into(),
                model_content: String::new(),
                truncated: false,
                files_read: Vec::new(),
                files_changed: Vec::new(),
                commands_run: Vec::new(),
                memory_changed: None,
                structured: Some(json!({"kind": "job", "job_id": "job-42", "tool": "run_command"})),
            })
            .unwrap();
        dispatch_recorded_call(&path, "run_command", "fixture-command", 3);
        let receipt = session
            .register_job(
                &mut state,
                &JobRequest {
                    agent: "main".into(),
                    execution_id: 2,
                    turn: 3,
                    tool_call_id: "fixture-command".into(),
                    tool: "run_command".into(),
                    output_id: Some("out_1".into()),
                    summary: "interrupted command".into(),
                    started: Instant::now(),
                    cancelling: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    wait_tool_available: false,
                },
            )
            .unwrap();
        assert!(!receipt.model_content.contains("wait_job"));
    }
    assert!(session.job("main", "job-1").unwrap().output_id.is_some());
    {
        let waiting = session.wait_job("main", "job-1");
        tokio::pin!(waiting);
        tokio::select! {
            biased;
            result = &mut waiting => panic!("unconfirmed job settled before close: {result:?}"),
            _ = tokio::task::yield_now() => {}
        }
        session.close().await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap();
        assert!(matches!(result, Err(Error::InterruptedOpenTool(_))));
    }
    assert!(matches!(
        session.wait_job("main", "job-1").await,
        Err(Error::InterruptedOpenTool(_))
    ));
    let failure = "fixture settlement failure";
    session
        .inner
        .settlement_failure
        .set(failure.into())
        .unwrap();
    assert!(matches!(
        session.wait_job("main", "job-1").await,
        Err(Error::InvalidEventStream(message)) if message == failure
    ));
    drop(session);

    let offline = Session::read_job(&path, "main", "job-1").unwrap();
    assert_eq!(JobStatus::Interrupted, offline.status);
    assert_eq!(None, offline.finished_at);
    assert_eq!(None, offline.result);
    assert_eq!(None, offline.output_id);
    assert_eq!(None, offline.elapsed_ms);
    assert_eq!(vec![offline], Session::read_jobs(&path, "main").unwrap());

    let session = Session::open(options.clone()).await.unwrap();
    assert_eq!(1, session.jobs("main").unwrap().len());
    let terminal = session.job("main", "job-1").unwrap();
    assert_eq!(JobStatus::Interrupted, terminal.status);
    assert_eq!(None, terminal.elapsed_ms);
    assert!(terminal.result.as_ref().unwrap().cleanup_incomplete);
    let cursor = terminal.finished_at.unwrap();
    assert_eq!(1, session.agent("main").unwrap().pending_inputs);
    assert_eq!(
        crate::AgentStatus::Idle,
        session.agent("main").unwrap().status
    );
    assert!(session.inner.job_workers.lock().unwrap().is_empty());
    session.close().await.unwrap();
    assert_eq!(terminal, session.wait_job("main", "job-1").await.unwrap());

    let session = Session::open(options).await.unwrap();
    assert_eq!(
        Some(cursor),
        session.job("main", "job-1").unwrap().finished_at
    );
    assert_eq!(1, session.agent("main").unwrap().pending_inputs);
    assert!(session.inner.job_workers.lock().unwrap().is_empty());
    session.close().await.unwrap();
}
