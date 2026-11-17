mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kuku::event::{EventPayload, ModelStopReason};
use kuku::permission::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer};
use kuku::{
    AdapterFuture, AgentConfig, AgentStatus, Input, InputSource, ModelAdapter, ModelEvent,
    ModelRequest, ModelStream, OutputRead, Session, SessionOptions, ToolAccess, ToolAdapter,
    ToolCancellation, ToolOutput, ToolSpec, ToolStatus,
};
use serde_json::json;

#[derive(Debug, Default)]
struct Model {
    requests: Mutex<Vec<ModelRequest>>,
    wait: bool,
}

impl ModelAdapter for Model {
    fn stream(&self, request: ModelRequest) -> AdapterFuture<'_, ModelStream> {
        Box::pin(async move {
            let completed = request.agent == "child"
                || request.body["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| message["role"] == "tool");
            self.requests.lock().unwrap().push(request);
            let mut events = vec![ModelEvent::StreamStart {
                request_id: "host-model".into(),
            }];
            if completed {
                events.push(ModelEvent::TextDelta {
                    text: "done".into(),
                });
                events.push(ModelEvent::StopReason {
                    reason: ModelStopReason::EndTurn,
                });
            } else {
                let calls = if self.wait {
                    vec![("wait", json!({"name":"active.txt","wait":true}))]
                } else {
                    vec![
                        ("denied", json!({"name":"denied.txt"})),
                        ("allowed", json!({"name":"allowed.txt"})),
                    ]
                };
                for (index, (id, args)) in calls.into_iter().enumerate() {
                    events.push(ModelEvent::ToolCallStart {
                        index: index as u64,
                        id: id.into(),
                        name: "host_file".into(),
                    });
                    events.push(ModelEvent::ToolCallArgDelta {
                        index: index as u64,
                        fragment: args.to_string(),
                    });
                    events.push(ModelEvent::ToolCallStop {
                        index: index as u64,
                    });
                }
                events.push(ModelEvent::StopReason {
                    reason: ModelStopReason::ToolUse,
                });
            }
            events.push(ModelEvent::StreamUsage {
                input_tokens: Some(100),
                output_tokens: Some(20),
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            });
            events.push(ModelEvent::StreamEnd);
            Ok(Box::pin(tokio_stream::iter(events.into_iter().map(Ok))) as ModelStream)
        })
    }
}

#[derive(Debug, Default)]
struct FileTool {
    calls: AtomicUsize,
    started: AtomicBool,
    cleaned: AtomicBool,
    incomplete: bool,
}

impl ToolAdapter for FileTool {
    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: "host_file".into(),
            description: "Write the selected file or wait until cancelled.".into(),
            input_schema: json!({"type":"object","properties":{"name":{"type":"string"},"wait":{"type":"boolean"}},"required":["name"]}),
            access: ToolAccess::Write,
            max_result_bytes: 2048,
        }
    }

    fn execute(
        &self,
        action: ToolAuthorization,
        cancellation: ToolCancellation,
    ) -> AdapterFuture<'_, ToolOutput> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!("host_file", action.tool.name);
            assert_eq!("main", action.agent);
            assert_eq!(InputSource::External("test-input".into()), action.source);
            let path = action.workspace.join(action.args["name"].as_str().unwrap());
            std::fs::write(&path, "actual host tool output")?;
            self.started.store(true, Ordering::SeqCst);
            if action.args["wait"] == true {
                cancellation.cancelled().await;
                tokio::time::sleep(Duration::from_millis(30)).await;
                std::fs::remove_file(path)?;
                self.cleaned.store(true, Ordering::SeqCst);
                return Ok(ToolOutput {
                    status: ToolStatus::Cancelled,
                    summary: "cleaned".into(),
                    content: "cancelled after cleanup".into(),
                    data: None,
                    cleanup_incomplete: self.incomplete,
                });
            }
            Ok(ToolOutput {
                data: Some(json!({"verified":true})),
                ..ToolOutput::success(format!("{}END_OF_RETAINED_OUTPUT", "x".repeat(20_000)))
            })
        })
    }
}

#[derive(Debug, Default)]
struct Gate(Mutex<Vec<ToolAuthorization>>);

impl ToolAuthorizer for Gate {
    fn authorize(&self, action: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        self.0.lock().unwrap().push(action.clone());
        Ok(if action.args["name"] == "denied.txt" {
            AuthorizationDecision::Deny
        } else {
            AuthorizationDecision::Allow
        })
    }
}

fn options(
    workspace: &std::path::Path,
    data: &std::path::Path,
    model: Arc<Model>,
    tool: Arc<dyn ToolAdapter>,
    gate: Arc<Gate>,
) -> SessionOptions {
    let mut config = common::test_config();
    config.handoff.enabled = false;
    config.plugin.enabled = false;
    config.discovery.auto_discover = false;
    let absent = format!(
        "KUKU_ADAPTER_ABSENT_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    assert!(std::env::var_os(&absent).is_none());
    config.providers.get_mut("openai").unwrap().api_key = kuku::config::ApiKey::Env(absent);
    let tier = config.tiers.get_mut("balanced").unwrap();
    tier.provider = "openai".into();
    tier.model = "external-model".into();
    tier.think = kuku::config::ThinkLevel::Off;
    let mut options = SessionOptions::new(workspace, data, config);
    options.model_adapter = Some(model);
    options.tools.push(tool);
    options.authorizer = gate;
    options.limits.max_requests_per_open = Some(4);
    options.limits.max_output_tokens_per_request = Some(32768);
    options
}

async fn settled(session: &Session, address: &str) -> kuku::AgentSnapshot {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = session.agent(address).unwrap();
            if snapshot.status != AgentStatus::Running {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

fn agent() -> AgentConfig {
    AgentConfig {
        tools: Some(vec!["host_file".into()]),
        no_skills: true,
        no_agents: true,
        ..Default::default()
    }
}

fn input() -> Input {
    Input {
        source: InputSource::External("test-input".into()),
        ..Input::new("input", "main", "perform the selected host action")
    }
}

#[tokio::test]
async fn external_model_and_tool_use_final_selection_authority_and_durable_bounded_output() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let model = Arc::new(Model::default());
    let tool = Arc::new(FileTool::default());
    let gate = Arc::new(Gate::default());
    let session = Session::open(options(
        workspace.path(),
        data.path(),
        model.clone(),
        tool.clone(),
        gate.clone(),
    ))
    .await
    .unwrap();
    session.create_agent("create", "main", agent()).unwrap();
    let selection = session.agent_selection("main").unwrap();
    assert_eq!("external-model", selection.model);
    assert_eq!(
        vec!["host_file"],
        selection
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(kuku::config::ProviderFormat::OpenAiChat, selection.protocol);
    assert!(model.requests.lock().unwrap().is_empty());
    assert_eq!(0, tool.calls.load(Ordering::SeqCst));
    session.send(input()).unwrap();
    let state = settled(&session, "main").await;
    assert_eq!(AgentStatus::Idle, state.status, "{:?}", state.last_error);
    assert_eq!(2, state.usage.model_requests);
    assert!(!workspace.path().join("denied.txt").exists());
    assert_eq!(
        "actual host tool output",
        std::fs::read_to_string(workspace.path().join("allowed.txt")).unwrap()
    );
    assert_eq!(1, tool.calls.load(Ordering::SeqCst));
    assert_eq!(2, gate.0.lock().unwrap().len());
    let events = session.events(0, 256).unwrap();
    let output_id = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::ToolResult {
                tool_call_id,
                model_content,
                structured: Some(value),
                truncated,
                ..
            } if tool_call_id == "allowed" => {
                assert!(*truncated, "{event:?}");
                assert!(model_content.len() <= 2048);
                assert!(model_content.contains(value["output_id"].as_str().unwrap()));
                Some(value["output_id"].as_str().unwrap().to_string())
            }
            _ => None,
        })
        .unwrap();
    let output = session
        .read_output(
            &output_id,
            &OutputRead {
                offset: 19_980,
                limit: 1024,
                search: None,
            },
        )
        .unwrap();
    assert!(output
        .segments
        .iter()
        .any(|segment| segment.text.contains("END_OF_RETAINED_OUTPUT")));
    session
        .create_agent(
            "create-child",
            "child",
            AgentConfig {
                model: Some("child-model".into()),
                tier: Some("balanced".into()),
                max_output_tokens: Some(65536),
                tools: Some(Vec::new()),
                no_agents: true,
                no_skills: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        32768,
        session.agent_selection("child").unwrap().max_output_tokens
    );
    session
        .send(Input::new(
            "child-input",
            "child",
            "check your selected output limit",
        ))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while session.agent("child").unwrap().status == AgentStatus::Running {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(AgentStatus::Idle, session.agent("child").unwrap().status);
    assert!(session.events(0, 256).unwrap().iter().any(|event| matches!(&event.payload, EventPayload::Unknown(value) if value["kind"] == "runtime.step_prepared" && value["agent"] == "child" && value["max_output_tokens"] == 32768)));
    {
        let requests = model.requests.lock().unwrap();
        assert_eq!(3, requests.len());
        assert_eq!("child", requests[2].agent);
        let body = &requests[2].body;
        assert_eq!(
            32768,
            body.get("max_tokens")
                .or_else(|| body.get("max_completion_tokens"))
                .unwrap()
                .as_u64()
                .unwrap()
        );
        assert_eq!(session.id(), requests[0].session_id);
        assert_eq!("main", requests[0].agent);
        assert_eq!(selection.connection, requests[0].connection);
        assert_eq!(selection.settings, requests[0].settings);
        assert_eq!(1, requests[0].body["tools"].as_array().unwrap().len());
        assert!(!format!("{:?}", requests[0]).contains("sk-test"));
    }
    session.close().await.unwrap();
}

#[tokio::test]
async fn custom_tool_cancellation_waits_for_cleanup_and_known_failure_is_visible() {
    for incomplete in [false, true] {
        let workspace = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let model = Arc::new(Model {
            wait: true,
            ..Default::default()
        });
        let tool = Arc::new(FileTool {
            incomplete,
            ..Default::default()
        });
        let session = Session::open(options(
            workspace.path(),
            data.path(),
            model.clone(),
            tool.clone(),
            Arc::new(Gate::default()),
        ))
        .await
        .unwrap();
        session.create_agent("create", "main", agent()).unwrap();
        session.send(input()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !tool.started.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let cancel = session.cancel("main").await;
        assert_eq!(!incomplete, cancel.is_ok());
        assert!(tool.cleaned.load(Ordering::SeqCst));
        assert!(!workspace.path().join("active.txt").exists());
        assert_eq!(1, model.requests.lock().unwrap().len());
        let events = session.events(0, 256).unwrap();
        assert!(events.iter().any(|event| matches!(&event.payload, EventPayload::ToolResult { status, structured:Some(value), .. } if status == "cancelled" && value["cleanup_incomplete"] == incomplete)));
        let close = session.close().await;
        if incomplete {
            let error = close.unwrap_err().to_string();
            assert!(error.contains("tool cleanup did not finish"), "{error}");
        } else {
            close.unwrap();
        }
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct CaptureFaultTool {
    file: FileTool,
    output: std::path::PathBuf,
    succeeded: AtomicBool,
}

#[cfg(unix)]
impl ToolAdapter for CaptureFaultTool {
    fn definition(&self) -> ToolSpec {
        self.file.definition()
    }

    fn execute(
        &self,
        action: ToolAuthorization,
        cancellation: ToolCancellation,
    ) -> AdapterFuture<'_, ToolOutput> {
        Box::pin(async move {
            let result = self.file.execute(action, cancellation).await?;
            assert_eq!(ToolStatus::Ok, result.status);
            let output_file = std::fs::read_dir(&self.output)?.next().unwrap()?.path();
            assert!(std::fs::read(output_file)?.starts_with(b"KUKUOUT1"));
            std::fs::remove_dir_all(&self.output)?;
            self.succeeded.store(true, Ordering::SeqCst);
            Ok(result)
        })
    }
}

#[cfg(unix)]
#[tokio::test]
async fn host_tool_output_commit_failure_stops_dispatch_and_survives_another_agent_on_close() {
    verify_output_failure_on_close(false).await;
}

#[cfg(unix)]
#[tokio::test]
async fn close_reports_reaped_output_failure_separately_from_another_agents_execution_error() {
    verify_output_failure_on_close(true).await;
}

#[cfg(unix)]
async fn verify_output_failure_on_close(exhaust_request_budget: bool) {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let session_id = "output-capture-failure";
    let journal =
        kuku::session::session_events_path(data.path(), workspace.path(), session_id).unwrap();
    let model = Arc::new(Model::default());
    let tool = Arc::new(CaptureFaultTool {
        file: FileTool::default(),
        output: journal.parent().unwrap().join("outputs"),
        succeeded: AtomicBool::new(false),
    });
    let mut options = options(
        workspace.path(),
        data.path(),
        model.clone(),
        tool.clone(),
        Arc::new(Gate::default()),
    );
    options.session_id = Some(session_id.into());
    options.limits.max_requests_per_open = Some(2);
    let session = Session::open(options.clone()).await.unwrap();
    session.create_agent("create", "main", agent()).unwrap();
    session.send(input()).unwrap();
    let snapshot = settled(&session, "main").await;
    assert_eq!(AgentStatus::Interrupted, snapshot.status, "{snapshot:?}");
    session
        .create_agent("create-child", "child", agent())
        .unwrap();
    session
        .send(Input::new("child-input", "child", "finish independently"))
        .unwrap();
    let child = settled(&session, "child").await;
    assert_eq!(AgentStatus::Idle, child.status, "{child:?}");
    assert_eq!(1, child.usage.model_requests);
    if exhaust_request_budget {
        session
            .send(Input::new("over-budget", "child", "one more turn"))
            .unwrap();
        let interrupted = settled(&session, "child").await;
        assert_eq!(AgentStatus::Interrupted, interrupted.status);
        assert!(
            interrupted
                .last_error
                .as_ref()
                .is_some_and(|error| error.contains("model request limit exhausted")),
            "{interrupted:?}"
        );
    }
    assert!(
        tool.succeeded.load(Ordering::SeqCst),
        "the host tool did not return success after creating its real effect"
    );
    assert_eq!(1, tool.file.calls.load(Ordering::SeqCst));
    assert_eq!(
        "actual host tool output",
        std::fs::read_to_string(workspace.path().join("allowed.txt")).unwrap()
    );
    assert_eq!(
        1,
        model
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.agent == "main")
            .count(),
        "the SDK dispatched another model request after output persistence failed"
    );
    let error = session.close().await.unwrap_err().to_string();
    assert!(
        error.contains("tool output could not be durably captured"),
        "{error}"
    );
    assert!(!error.contains("model request limit exhausted"), "{error}");
    assert_eq!(error, session.close().await.unwrap_err().to_string());
    assert!(
        session
            .events(0, 256)
            .unwrap()
            .iter()
            .any(|event| matches!(&event.payload,
        EventPayload::ToolResult { tool_call_id, status, structured: Some(value), .. }
            if tool_call_id == "allowed" && status == "error"
                && value["output_persistence_failed"] == true
                && value["cleanup_incomplete"] == false)),
        "SDK output persistence failure had no durable terminal fact"
    );
    let reopened = Session::open(options).await.unwrap();
    assert_eq!(
        AgentStatus::Interrupted,
        reopened.agent("main").unwrap().status
    );
    reopened.close().await.unwrap();
}
