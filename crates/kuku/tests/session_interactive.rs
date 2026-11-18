mod common;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, Notify};

use kuku::event::{EventPayload, EventStore, ModelStopReason};
use kuku::permission::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer, TrustedLocal};
use kuku::{
    Activation, AdapterFuture, AgentConfig, AgentStatus, Error, Input, InputSource, InputStatus,
    InputTiming, ModelAdapter, ModelEvent, ModelRequest, ModelStream, Observation,
    ProviderFailureKind, Session, SessionOptions, ToolAccess, ToolAdapter, ToolCancellation,
    ToolOutput, ToolSpec,
};

const WAIT: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct Model(mpsc::UnboundedSender<Call>);

#[derive(Debug)]
struct Call {
    request: ModelRequest,
    response: oneshot::Sender<Reply>,
}

#[derive(Debug)]
enum Reply {
    Tool,
    ToolBatch,
    Text(&'static str),
    TemporaryFailure,
}

impl ModelAdapter for Model {
    fn stream(&self, request: ModelRequest) -> AdapterFuture<'_, ModelStream> {
        Box::pin(async move {
            let (response, receive) = oneshot::channel();
            self.0.send(Call { request, response }).unwrap();
            let mut events = vec![ModelEvent::StreamStart {
                request_id: "fixture".into(),
            }];
            match receive.await.unwrap() {
                reply @ (Reply::Tool | Reply::ToolBatch) => {
                    let ids = if matches!(reply, Reply::ToolBatch) {
                        &['a', 'b'][..]
                    } else {
                        &['a'][..]
                    };
                    for (index, id) in ids.iter().enumerate() {
                        let index = index as u64;
                        events.extend([
                            ModelEvent::ToolCallStart {
                                index,
                                id: id.to_string(),
                                name: "hold".into(),
                            },
                            ModelEvent::ToolCallArgDelta {
                                index,
                                fragment: "{}".into(),
                            },
                            ModelEvent::ToolCallStop { index },
                        ]);
                    }
                    events.push(ModelEvent::StopReason {
                        reason: ModelStopReason::ToolUse,
                    });
                }
                Reply::Text(text) => {
                    events.extend([
                        ModelEvent::TextDelta { text: text.into() },
                        ModelEvent::StopReason {
                            reason: ModelStopReason::EndTurn,
                        },
                    ]);
                }
                Reply::TemporaryFailure => {
                    return Err(Error::Provider {
                        kind: ProviderFailureKind::ProviderUnavailable,
                        message: "temporary fixture outage".into(),
                        provider: None,
                        model: None,
                    });
                }
            }
            events.push(ModelEvent::StreamEnd);
            Ok(Box::pin(tokio_stream::iter(events.into_iter().map(Ok))) as ModelStream)
        })
    }
}

#[derive(Debug, Default)]
struct HoldTool {
    started: Notify,
    finished: Notify,
}

#[derive(Debug)]
struct Defer;

impl ToolAuthorizer for Defer {
    fn authorize(&self, _: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        Ok(AuthorizationDecision::Defer)
    }
}

impl ToolAdapter for HoldTool {
    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: "hold".into(),
            description: "Wait for the fixture to resume.".into(),
            input_schema: serde_json::json!({"type":"object"}),
            access: ToolAccess::Read,
            max_result_bytes: 2048,
        }
    }

    fn execute(
        &self,
        _: ToolAuthorization,
        cancellation: ToolCancellation,
    ) -> AdapterFuture<'_, ToolOutput> {
        Box::pin(async move {
            self.started.notify_one();
            tokio::select! {
                _ = self.finished.notified() => Ok(ToolOutput::success("held tool finished")),
                _ = cancellation.cancelled() => Ok(ToolOutput::success("held tool cleaned up")),
            }
        })
    }
}

async fn ready(session: &Session, status: AgentStatus) {
    tokio::time::timeout(WAIT, async {
        while session.agent("main").unwrap().status != status {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "expected {status:?}, got {:?}",
            session.agent("main").unwrap()
        )
    });
}

async fn open_model() -> (
    tempfile::TempDir,
    Session,
    mpsc::UnboundedReceiver<Call>,
    Arc<HoldTool>,
) {
    open_authorized(Arc::new(TrustedLocal)).await
}

async fn open_authorized(
    authorizer: Arc<dyn ToolAuthorizer>,
) -> (
    tempfile::TempDir,
    Session,
    mpsc::UnboundedReceiver<Call>,
    Arc<HoldTool>,
) {
    let root = tempfile::tempdir().unwrap();
    let mut options =
        SessionOptions::new(root.path(), root.path().join("data"), common::test_config());
    let (model, calls) = mpsc::unbounded_channel();
    let tool = Arc::new(HoldTool::default());
    options.model_adapter = Some(Arc::new(Model(model)));
    options.authorizer = authorizer;
    options.limits.max_tools_per_agent = 1;
    options.tools = vec![tool.clone()];
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    (root, session, calls, tool)
}

#[tokio::test]
async fn separate_authorization_replies_restore_only_undispatched_batch_calls() {
    let (_root, session, mut calls, _) = open_authorized(Arc::new(Defer)).await;
    session
        .send(Input::new("start", "main", "authorize the batch"))
        .unwrap();
    let first = next_call(&mut calls).await;
    first.response.send(Reply::ToolBatch).unwrap();
    ready(&session, AgentStatus::Waiting).await;
    let first_wait = session.agent("main").unwrap().permission.unwrap();
    assert_eq!("a", first_wait.tool_call_id);
    let declined = session
        .reply_permission("deny-first", "main", &first_wait.id, false)
        .unwrap();
    ready(&session, AgentStatus::Waiting).await;
    let next_wait = session.agent("main").unwrap().permission.unwrap();
    assert_eq!("b", next_wait.tool_call_id);
    assert_eq!(
        declined,
        session
            .reply_permission("deny-first", "main", &first_wait.id, false)
            .unwrap()
    );
    assert!(matches!(
        session.reply_permission("stale-other-window", "main", &first_wait.id, true),
        Err(Error::PermissionRequestNotPending(_))
    ));
    assert_eq!(
        Some(next_wait.clone()),
        session.agent("main").unwrap().permission
    );
    session
        .reply_permission("deny-next", "main", &next_wait.id, false)
        .unwrap();
    let continuation = next_call(&mut calls).await;
    assert_eq!(first.request.turn, continuation.request.turn);
    continuation
        .response
        .send(Reply::Text("both decisions applied"))
        .unwrap();
    ready(&session, AgentStatus::Idle).await;
    session.close().await.unwrap();
}

async fn next_call(calls: &mut mpsc::UnboundedReceiver<Call>) -> Call {
    tokio::time::timeout(WAIT, calls.recv())
        .await
        .unwrap()
        .unwrap()
}

fn next_turn(key: &str, text: &str) -> Input {
    Input {
        timing: InputTiming::NextTurn,
        ..Input::new(key, "main", text)
    }
}

#[tokio::test]
async fn queued_replacement_recovers_from_a_torn_record_and_from_a_lost_receipt() {
    let root = tempfile::tempdir().unwrap();
    let mut options =
        SessionOptions::new(root.path(), root.path().join("data"), common::test_config());
    options.session_id = Some("replace-queued".into());
    options.limits.max_pending_inputs = 2;
    let session = Session::open(options.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    let original = Input {
        activation: Activation::Enqueue,
        correlation: Some("original submission metadata".into()),
        bootstrap_skill: Some("inspect-fixture".into()),
        ..next_turn("old", "same text")
    };
    let other = Input {
        activation: Activation::Enqueue,
        ..Input::new("other", "main", "same text")
    };
    let old_receipt = session.send(original.clone()).unwrap();
    let other_receipt = session.send(other.clone()).unwrap();
    let path = session.events_path().to_owned();
    let prefix = std::fs::read(&path).unwrap();

    let replace = |session: &Session| {
        session.replace_queued("new", "main", "old", "edited 界", &original.source)
    };
    let lost_receipt = replace(&session).unwrap();
    assert_eq!(other_receipt.event_id + 1, lost_receipt.event_id);
    let complete = std::fs::read(&path).unwrap();
    assert!(complete.starts_with(&prefix));
    assert!(complete.ends_with(b"\n"));
    assert_eq!(
        1,
        complete[prefix.len()..]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count()
    );
    drop(session);
    std::fs::write(&path, &complete[..complete.len() - 1]).unwrap();

    let reopened = Session::open(options.clone()).await.unwrap();
    let pending = reopened.pending_inputs("main").unwrap();
    assert_eq!(original, pending[0].input);
    assert_eq!(old_receipt, pending[0].receipt);
    assert_eq!(other, pending[1].input);
    assert_eq!(None, reopened.receipt("new").unwrap());
    assert!(matches!(
        reopened.replace_queued(
            "bad-source",
            "main",
            "old",
            "new text",
            &InputSource::Host("other".into())
        ),
        Err(Error::InvalidArgument(_))
    ));
    assert!(matches!(
        reopened.replace_queued("empty", "main", "old", " \n", &original.source),
        Err(Error::InvalidArgument(_))
    ));
    assert_eq!(pending, reopened.pending_inputs("main").unwrap());
    let replacement = replace(&reopened).unwrap();
    assert_eq!(lost_receipt, replacement);
    drop(reopened);

    let session = Session::open(options).await.unwrap();
    assert_eq!(replacement, replace(&session).unwrap());
    let pending = session.pending_inputs("main").unwrap();
    let edited = Input {
        command_id: "new".into(),
        text: "edited 界".into(),
        ..original
    };
    assert_eq!(2, pending.len());
    assert_eq!(edited, pending[0].input);
    assert_eq!(replacement, pending[0].receipt);
    assert_eq!(other, pending[1].input);
    assert_eq!(other_receipt, pending[1].receipt);
    assert_eq!(
        InputStatus::Withdrawn,
        session.input_status("main", "old").unwrap()
    );
    assert_eq!(
        InputStatus::Queued,
        session.input_status("main", "new").unwrap()
    );
    assert!(matches!(
        session.replace_queued("new", "main", "old", "changed retry", &edited.source),
        Err(Error::CommandConflict(id)) if id == "new"
    ));
    assert!(matches!(
        session.replace_queued("too-late", "main", "old", "another edit", &edited.source),
        Err(Error::InputNotQueued(id)) if id == "old"
    ));
    assert_eq!(None, session.receipt("too-late").unwrap());
    assert_eq!(pending, session.pending_inputs("main").unwrap());
    session.close().await.unwrap();
    assert_eq!(
        Some((replacement, Some(InputStatus::Queued))),
        Session::read_receipt(&path, "new").unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_replacement_and_admission_deliver_exactly_one_version() {
    let (_root, session, mut calls, _) = open_model().await;
    let source = InputSource::Host("local".into());
    session
        .send(Input {
            activation: Activation::Enqueue,
            ..Input::new("old", "main", "original race text")
        })
        .unwrap();
    let ready_to_race = Arc::new(tokio::sync::Barrier::new(2));
    let replace = tokio::spawn({
        let ready_to_race = ready_to_race.clone();
        let session = session.clone();
        let source = source.clone();
        async move {
            ready_to_race.wait().await;
            session.replace_queued("new", "main", "old", "replacement race text", &source)
        }
    });
    ready_to_race.wait().await;
    session.advance("main").unwrap();
    let call = next_call(&mut calls).await;
    let body = call.request.body.to_string();
    match replace.await.unwrap() {
        Ok(receipt) => {
            assert!(body.contains("replacement race text"));
            assert!(!body.contains("original race text"));
            assert_eq!(
                InputStatus::Withdrawn,
                session.input_status("main", "old").unwrap()
            );
            assert_eq!(
                InputStatus::Accepted,
                session.input_status("main", "new").unwrap()
            );
            assert_eq!(
                receipt,
                session
                    .replace_queued("new", "main", "old", "replacement race text", &source)
                    .unwrap()
            );
        }
        Err(Error::InputNotQueued(id)) => {
            assert_eq!("old", id);
            assert!(body.contains("original race text"));
            assert!(!body.contains("replacement race text"));
            assert_eq!(
                InputStatus::Accepted,
                session.input_status("main", "old").unwrap()
            );
            assert_eq!(None, session.receipt("new").unwrap());
        }
        result => panic!("unexpected race result: {result:?}"),
    }
    call.response
        .send(Reply::Text("one input accepted"))
        .unwrap();
    ready(&session, AgentStatus::Idle).await;
    assert!(session.pending_inputs("main").unwrap().is_empty());
    assert!(calls.try_recv().is_err());
    session.close().await.unwrap();
}

#[tokio::test]
async fn an_edited_input_preserves_its_order_and_admitted_text_when_retried() {
    let root = tempfile::tempdir().unwrap();
    let mut options =
        SessionOptions::new(root.path(), root.path().join("data"), common::test_config());
    let (model, mut calls) = mpsc::unbounded_channel();
    options.model_adapter = Some(Arc::new(Model(model)));
    options.limits.retry_transient_provider_errors = false;
    options.session_id = Some("retry-edited-input".into());
    let session = Session::open(options.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    let first = Input {
        activation: Activation::Enqueue,
        ..Input::new("first", "main", "original first input")
    };
    let later = Input {
        source: InputSource::Host("another client".into()),
        activation: Activation::Enqueue,
        ..next_turn("later", "subsequent input")
    };
    session.send(first.clone()).unwrap();
    let later_receipt = session.send(later.clone()).unwrap();
    let replacement = session
        .replace_queued(
            "edited",
            "main",
            "first",
            "edited first input",
            &first.source,
        )
        .unwrap();
    assert!(replacement.event_id > later_receipt.event_id);

    session.advance("main").unwrap();
    let attempt = next_call(&mut calls).await;
    let body = attempt.request.body.to_string();
    assert!(body.contains("edited first input"));
    assert!(!body.contains("subsequent input"));
    attempt.response.send(Reply::TemporaryFailure).unwrap();
    ready(&session, AgentStatus::Interrupted).await;
    session.close().await.unwrap();

    let reopened = Session::open(options).await.unwrap();
    reopened.advance("main").unwrap();
    assert!(matches!(
        reopened.replace_queued("late-edit", "main", "edited", "changed after admission", &first.source),
        Err(Error::InputNotQueued(id)) if id == "edited"
    ));
    assert_eq!(None, reopened.receipt("late-edit").unwrap());
    assert_eq!(Some(replacement), reopened.receipt("edited").unwrap());
    let retried = next_call(&mut calls).await;
    let body = retried.request.body.to_string();
    assert!(body.contains("edited first input"));
    assert!(!body.contains("changed after admission"));
    assert!(!body.contains("subsequent input"));
    let pending = reopened.pending_inputs("main").unwrap();
    assert_eq!(1, pending.len());
    assert_eq!(later, pending[0].input);
    assert_eq!(later_receipt, pending[0].receipt);
    retried
        .response
        .send(Reply::Text("replayed first input"))
        .unwrap();
    ready(&reopened, AgentStatus::Idle).await;
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn steering_waits_for_tools_and_skips_an_earlier_next_turn_receipt() {
    let (root, session, mut calls, tool) = open_model().await;
    let skill = root.path().join(".fixture/skills/inspect-fixture");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), "---\nname: inspect-fixture\ndescription: A sample selected skill.\n---\nSELECTED_SKILL_INSTRUCTIONS\n").unwrap();
    session
        .send(Input::new("start", "main", "original task"))
        .unwrap();
    let first = next_call(&mut calls).await;
    let first_execution = session.agent("main").unwrap().execution_id.unwrap();
    first.response.send(Reply::Tool).unwrap();
    tokio::time::timeout(WAIT, tool.started.notified())
        .await
        .unwrap();

    let later = session
        .send(next_turn("later", "work in the next turn"))
        .unwrap();
    let steer = session
        .send(Input {
            bootstrap_skill: Some("inspect-fixture".into()),
            ..Input::new("steer1", "main", "first steering message")
        })
        .unwrap();
    let abandoned = Input::new("abandoned", "main", "withdraw this material");
    session.send(abandoned.clone()).unwrap();
    let removed = session
        .withdraw_input("withdraw", "main", "abandoned")
        .unwrap();
    assert_eq!(InputStatus::Withdrawn, removed.status);
    assert_eq!(
        removed,
        session
            .withdraw_input("withdraw", "main", "abandoned")
            .unwrap()
    );
    assert_eq!(
        InputStatus::Accepted,
        session
            .withdraw_input("already", "main", "start")
            .unwrap()
            .status
    );
    let steer2 = session
        .send(Input::new("steer2", "main", "second steering message"))
        .unwrap();
    let view = session.agent_view("main").unwrap();
    assert_eq!(Some(first_execution), view.snapshot.execution_id);
    assert_eq!(
        vec![later, steer, steer2],
        view.pending_inputs
            .into_iter()
            .map(|entry| entry.receipt)
            .collect::<Vec<_>>()
    );
    assert!(calls.try_recv().is_err());
    tool.finished.notify_one();

    let continuation = next_call(&mut calls).await;
    assert_eq!(first.request.turn, continuation.request.turn);
    let body = continuation.request.body.to_string();
    let tool_result = body.find("held tool finished").unwrap();
    let first_steer = body.find("first steering message").unwrap();
    let second_steer = body.find("second steering message").unwrap();
    assert!(
        tool_result < first_steer && first_steer < second_steer,
        "{body}"
    );
    assert!(!body.contains("work in the next turn"), "{body}");
    assert!(!body.contains("withdraw this material"), "{body}");
    assert!(body.contains("SELECTED_SKILL_INSTRUCTIONS"), "{body}");
    assert!(session
        .agent("main")
        .unwrap()
        .config
        .bootstrap_skill
        .is_none());
    continuation
        .response
        .send(Reply::Text("current turn finished"))
        .unwrap();

    let next = next_call(&mut calls).await;
    assert!(next.request.turn > first.request.turn);
    assert!(next
        .request
        .body
        .to_string()
        .contains("work in the next turn"));
    let execution = session.agent("main").unwrap().execution_id.unwrap();
    assert!(execution > first_execution);
    assert!(
        matches!(session.cancel_execution("stale", "main", first_execution).await, Err(Error::ExecutionNotActive(id)) if id == first_execution)
    );
    assert_eq!(Some(execution), session.agent("main").unwrap().execution_id);
    next.response
        .send(Reply::Text("next turn finished"))
        .unwrap();
    ready(&session, AgentStatus::Idle).await;
    assert_eq!(
        InputStatus::Accepted,
        session.input_status("main", "steer1").unwrap()
    );
    assert_eq!(
        InputStatus::Withdrawn,
        session.input_status("main", "abandoned").unwrap()
    );
    session.close().await.unwrap();
    let detached = Session::read_view(session.events_path(), "main").unwrap();
    assert_eq!(AgentStatus::Idle, detached.snapshot.status);
    assert!(detached.pending_inputs.is_empty());
}

#[tokio::test]
async fn stopped_execution_retains_pending_inputs_and_deduplicates_cancel_and_advance() {
    let (_root, session, mut calls, _) = open_model().await;
    session
        .send(Input::new("start", "main", "start blocking model"))
        .unwrap();
    let first = next_call(&mut calls).await;
    let execution = session.agent("main").unwrap().execution_id.unwrap();
    let input = next_turn("queued", "retain this work");
    let delivery = session.send(input.clone()).unwrap();
    let stopped = session
        .cancel_execution("stop", "main", execution)
        .await
        .unwrap();
    assert!(first.response.send(Reply::Text("cancelled")).is_err());
    assert_eq!(AgentStatus::Stopped, session.agent("main").unwrap().status);
    assert_eq!(delivery, session.send(input).unwrap());
    assert_eq!(AgentStatus::Stopped, session.agent("main").unwrap().status);
    assert!(calls.try_recv().is_err());
    assert_eq!(
        InputStatus::Queued,
        session.input_status("main", "queued").unwrap()
    );
    let advanced = session.advance_command("continue", "main").unwrap();
    let next = next_call(&mut calls).await;
    let new_execution = session.agent("main").unwrap().execution_id.unwrap();
    assert!(new_execution > execution);
    assert_eq!(
        stopped,
        session
            .cancel_execution("stop", "main", execution)
            .await
            .unwrap()
    );
    assert_eq!(
        advanced,
        session.advance_command("continue", "main").unwrap()
    );
    assert!(next.request.body.to_string().contains("retain this work"));
    next.response.send(Reply::Text("continued")).unwrap();
    ready(&session, AgentStatus::Idle).await;
    session.close().await.unwrap();
}

#[tokio::test]
async fn cancelling_a_tool_batch_settles_undispatched_calls_and_new_send_restarts() {
    let (_root, session, mut calls, tool) = open_model().await;
    session
        .send(Input::new("start", "main", "start tool batch"))
        .unwrap();
    let first = next_call(&mut calls).await;
    first.response.send(Reply::ToolBatch).unwrap();
    tokio::time::timeout(WAIT, tool.started.notified())
        .await
        .unwrap();
    let execution = session.agent("main").unwrap().execution_id.unwrap();
    session
        .cancel_execution("cancel-batch", "main", execution)
        .await
        .unwrap();
    assert_eq!(AgentStatus::Stopped, session.agent("main").unwrap().status);
    let events = session.events(0, 128).unwrap();
    let cancelled = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ToolResult {
                tool_call_id,
                status,
                ..
            } if status == "cancelled" => Some(tool_call_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(vec!["a", "b"], cancelled);
    assert!(!events.iter().any(|event| matches!(&event.payload, EventPayload::Unknown(value) if value["kind"] == "runtime.tool_dispatched" && value["tool_call_id"] == "b")));
    session
        .send(Input::new("again", "main", "continue with fresh input"))
        .unwrap();
    let fresh = next_call(&mut calls).await;
    assert!(fresh.request.turn > first.request.turn);
    fresh
        .response
        .send(Reply::Text("done after restart"))
        .unwrap();
    ready(&session, AgentStatus::Idle).await;
    session.close().await.unwrap();
}

#[tokio::test]
async fn retry_uses_the_prepared_body_and_leaves_new_input_and_selection_for_the_next_step() {
    let (_root, session, mut calls, tool) = open_model().await;
    let mut observer = session.observe(0);
    session
        .send(Input::new(
            "start",
            "main",
            "start with the original selection",
        ))
        .unwrap();
    let first = next_call(&mut calls).await;
    let original_execution = session.agent("main").unwrap().execution_id.unwrap();
    session
        .configure_agent(
            "select",
            "main",
            AgentConfig {
                model: Some("updated-fixture-model".into()),
                ..AgentConfig::default()
            },
        )
        .unwrap();
    assert_ne!(
        session.agent("main").unwrap().active_config.unwrap().model,
        Some("updated-fixture-model".into())
    );
    session
        .send(Input::new(
            "steer",
            "main",
            "wait for the next prepared request",
        ))
        .unwrap();
    first.response.send(Reply::TemporaryFailure).unwrap();

    let retry = next_call(&mut calls).await;
    assert_eq!(first.request.request_id, retry.request.request_id);
    assert_eq!(first.request.turn, retry.request.turn);
    assert_eq!(first.request.body, retry.request.body);
    assert_eq!(first.request.model, retry.request.model);
    assert_eq!(
        InputStatus::Queued,
        session.input_status("main", "steer").unwrap()
    );
    retry.response.send(Reply::Tool).unwrap();
    tokio::time::timeout(WAIT, tool.started.notified())
        .await
        .unwrap();
    tool.finished.notify_one();
    let next = next_call(&mut calls).await;
    assert_eq!(first.request.turn, next.request.turn);
    assert_eq!("updated-fixture-model", next.request.model);
    assert!(next
        .request
        .body
        .to_string()
        .contains("wait for the next prepared request"));
    next.response.send(Reply::Text("finished")).unwrap();
    ready(&session, AgentStatus::Idle).await;
    let facts = session.events(0, 256).unwrap();
    assert_eq!(2, facts.iter().filter(|event| matches!(&event.payload, EventPayload::Unknown(value) if value["kind"] == "runtime.step_prepared")).count());
    assert_eq!(1, facts.iter().filter(|event| matches!(&event.payload, EventPayload::Unknown(value) if value["kind"] == "runtime.model_retry" && value["attempt"] == 2)).count());
    assert_eq!(3, session.agent("main").unwrap().usage.model_requests);
    let mut tagged = false;
    while let Some(observation) = observer.try_next().unwrap() {
        if let Observation::Live {
            execution_id,
            turn,
            after,
            ..
        } = observation
        {
            assert_eq!(original_execution, execution_id);
            assert_eq!(first.request.turn, turn);
            assert!(
                execution_id < after
                    && after <= EventStore::read_head(session.events_path()).unwrap()
            );
            tagged = true;
        }
    }
    assert!(tagged);
    session.close().await.unwrap();
}

#[tokio::test]
async fn original_creation_intent_recovers_a_claim_only_journal() {
    let root = tempfile::tempdir().unwrap();
    let mut options =
        SessionOptions::new(root.path(), root.path().join("data"), common::test_config());
    options.session_id = Some("partial-create".into());
    let input = Input {
        activation: Activation::Enqueue,
        ..Input::new("initial", "main", "initial material")
    };
    let config = AgentConfig::default();
    let session = Session::open_for_creation(options.clone(), &input, &config)
        .await
        .unwrap();
    let path = session.events_path().to_path_buf();
    drop(session);
    let bytes = std::fs::read(&path).unwrap();
    let first = bytes.iter().position(|byte| *byte == b'\n').unwrap();
    std::fs::write(&path, &bytes[..=first]).unwrap();

    let collision = Input::new("different", "main", "initial material");
    assert!(matches!(
        Session::open_for_creation(options.clone(), &collision, &config).await,
        Err(Error::SessionCreationConflict(_))
    ));
    assert_eq!(1, EventStore::read_head(&path).unwrap());
    let resumed = Session::open_for_creation(options.clone(), &input, &config)
        .await
        .unwrap();
    assert!(
        matches!(&resumed.events(1, 1).unwrap()[0].payload, EventPayload::SessionCreated { session_id, .. } if session_id == "partial-create")
    );
    let created = resumed
        .create_agent("create", "main", config.clone())
        .unwrap();
    drop(resumed);
    let resumed = Session::open_for_creation(options, &input, &config)
        .await
        .unwrap();
    assert_eq!(
        created,
        resumed.create_agent("create", "main", config).unwrap()
    );
    let delivered = resumed.send(input.clone()).unwrap();
    assert_eq!(delivered, resumed.send(input).unwrap());
    resumed.close().await.unwrap();
    assert_eq!(
        Some((delivered, Some(InputStatus::Queued))),
        Session::read_receipt(&path, "initial").unwrap()
    );
}
