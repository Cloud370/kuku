mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use kuku::event::{EventPayload, ModelStopReason};
use kuku::permission::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer};
use kuku::{
    Activation, AdapterFuture, AgentConfig, Error, Input, InputSource, ModelAdapter, ModelEvent,
    ModelRequest, ModelStream, NamedValueKind, NamedValueSource, Observation, OutputRead,
    OutputReference, Session, SessionOptions,
};

const ORIGINAL: &str = "ORIGINAL_PAYLOAD_标记";
const REPLACEMENT: &str = "replacement payload";
const READ_TASK: &str = "Read the attached evidence.";

fn options(workspace: &Path, data: &Path) -> SessionOptions {
    let mut config = common::test_config();
    let tier = config.tiers.get_mut("balanced").unwrap();
    tier.provider = "openai".into();
    tier.model = "values-test".into();
    tier.think = kuku::config::ThinkLevel::Off;
    config.plugin.enabled = false;
    config.handoff.enabled = false;
    SessionOptions::new(workspace, data, config)
}

fn text(session: &Session, agent: &str, reference: &OutputReference) -> String {
    session
        .read_agent_output(agent, &reference.id, &OutputRead::default())
        .unwrap()
        .segments
        .into_iter()
        .map(|segment| segment.text)
        .collect()
}

#[tokio::test]
async fn original_value_survives_name_replacement_in_a_durable_child_input() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut options = options(workspace.path(), data.path());
    options.limits.output_retention_secs = 0;
    let session = Session::open(options.clone()).await.unwrap();
    session
        .create_agent("main-create", "main", AgentConfig::default())
        .unwrap();
    session
        .create_agent(
            "child-create",
            "child",
            AgentConfig {
                tools: Some(vec!["read_output".into()]),
                ..Default::default()
            },
        )
        .unwrap();
    let payload = format!("{ORIGINAL}{}", "x".repeat(128 * 1024));
    let source = NamedValueSource::Text(payload);
    let (first, retry) = tokio::join!(
        session.save_named_value("main", "Baseline", source.clone(), false),
        session.save_named_value("main", "Baseline", source, false),
    );
    let original = first.unwrap();
    assert_eq!(original, retry.unwrap());
    assert_eq!(NamedValueKind::Text, original.kind);
    assert!(session.named_value("main", "baseline").is_err());
    assert!(session.named_value("child", "Baseline").is_err());
    assert!(matches!(
        session.read_agent_output("child", &original.reference.id, &OutputRead::default()),
        Err(Error::OutputUnavailable(_))
    ));
    let output_dir = session.events_path().parent().unwrap().join("outputs");
    assert_eq!(1, std::fs::read_dir(output_dir).unwrap().count());
    let events_path = session.events_path().to_owned();
    assert_eq!(
        vec![original.clone()],
        Session::read_named_values(&events_path, "main").unwrap()
    );

    let mut handoff = Input::new("share", "child", READ_TASK);
    handoff.source = InputSource::Agent("main".into());
    handoff.activation = Activation::Enqueue;
    let values = std::slice::from_ref(&original.reference);
    let receipt = session.send_with_values(handoff.clone(), values).unwrap();
    assert_eq!(
        Some(receipt),
        session.input_receipt_with_values(&handoff, values).unwrap()
    );
    assert!(matches!(
        session.send(handoff),
        Err(Error::CommandConflict(_))
    ));
    let replacement = session
        .save_named_value(
            "main",
            "Baseline",
            NamedValueSource::Text(REPLACEMENT.into()),
            true,
        )
        .await
        .unwrap();
    assert_ne!(original.reference.id, replacement.reference.id);
    options.session_id = Some(session.id().into());
    session.close().await.unwrap();
    assert!(!std::fs::read_to_string(&events_path)
        .unwrap()
        .contains(ORIGINAL));
    assert_eq!(
        vec![replacement.clone()],
        Session::read_named_values(&events_path, "main").unwrap()
    );
    assert!(Session::read_saved_agent_output(
        &events_path,
        "child",
        &original.reference.id,
        &OutputRead::default()
    )
    .is_ok());
    assert!(matches!(
        Session::read_saved_agent_output(
            &events_path,
            "child",
            &replacement.reference.id,
            &OutputRead::default()
        ),
        Err(Error::OutputUnavailable(_))
    ));

    options.model_adapter = Some(Arc::new(ReferenceReader {
        agent: "child",
        reference: original.reference.clone(),
        name: None,
    }));
    let reopened = Session::open(options).await.unwrap();
    assert_eq!(
        replacement,
        reopened.named_value("main", "Baseline").unwrap()
    );
    assert_eq!(REPLACEMENT, text(&reopened, "main", &replacement.reference));
    reopened.advance("child").unwrap();
    let settled = runtime_record(&reopened, "child", "turn_settled").await;
    assert_eq!("verified original reference", settled["output"]);
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn deferred_named_read_keeps_the_id_authorized_before_replacement_and_reopen() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut options = options(workspace.path(), data.path());
    options.limits.output_retention_secs = 0;
    let session = Session::open(options.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    let original = session
        .save_named_value(
            "main",
            "baseline",
            NamedValueSource::Text(ORIGINAL.into()),
            false,
        )
        .await
        .unwrap();
    options.session_id = Some(session.id().into());
    session.close().await.unwrap();
    options.authorizer = Arc::new(DeferRead {
        id: original.reference.id.clone(),
    });
    options.model_adapter = Some(Arc::new(ReferenceReader {
        agent: "main",
        reference: original.reference.clone(),
        name: Some("baseline"),
    }));
    let session = Session::open(options.clone()).await.unwrap();
    session
        .send(Input::new("read-original", "main", READ_TASK))
        .unwrap();
    let waiting = runtime_record(&session, "main", "permission_waiting").await;
    let request = waiting["request"]["id"].as_str().unwrap();
    let replacement = session
        .save_named_value(
            "main",
            "baseline",
            NamedValueSource::Text(REPLACEMENT.into()),
            true,
        )
        .await
        .unwrap();
    session.close().await.unwrap();

    let reopened = Session::open(options).await.unwrap();
    assert_eq!(
        replacement,
        reopened.named_value("main", "baseline").unwrap()
    );
    reopened
        .reply_permission("approve-read", "main", request, true)
        .unwrap();
    let settled = runtime_record(&reopened, "main", "turn_settled").await;
    assert_eq!("verified original reference", settled["output"]);
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn write_fault_and_quota_failure_leave_the_original_named_value_intact() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut options = options(workspace.path(), data.path());
    options.limits.max_retained_output_bytes = 16 * 1024;
    options.limits.max_named_values = 1;
    let session = Session::open(options.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    let baseline = "a".repeat(6_000);
    let original = session
        .save_named_value(
            "main",
            "baseline",
            NamedValueSource::Text(baseline.clone()),
            false,
        )
        .await
        .unwrap();
    let error = session
        .save_named_value(
            "main",
            "another",
            NamedValueSource::Output(original.reference.id.clone()),
            false,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("named value limit"));
    let error = session
        .save_named_value(
            "main",
            "baseline",
            NamedValueSource::Text(REPLACEMENT.into()),
            false,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("set replace"));
    let error = session
        .save_named_value(
            "main",
            "baseline",
            NamedValueSource::Text("b".repeat(6_500)),
            true,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, Error::OutputQuotaExceeded { .. }));

    let outputs = session.events_path().parent().unwrap().join("outputs");
    let parked = outputs.with_extension("parked");
    std::fs::rename(&outputs, &parked).unwrap();
    std::fs::write(&outputs, "filesystem fixture").unwrap();
    let error = session
        .save_named_value(
            "main",
            "baseline",
            NamedValueSource::Text(REPLACEMENT.into()),
            true,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Io(_)));
    std::fs::remove_file(&outputs).unwrap();
    std::fs::rename(parked, outputs).unwrap();
    assert_eq!(original, session.named_value("main", "baseline").unwrap());

    options.session_id = Some(session.id().into());
    session.close().await.unwrap();
    let reopened = Session::open(options).await.unwrap();
    assert_eq!(original, reopened.named_value("main", "baseline").unwrap());
    assert_eq!(baseline, text(&reopened, "main", &original.reference));
    reopened.close().await.unwrap();
}

#[derive(Debug)]
struct ReferenceReader {
    agent: &'static str,
    reference: OutputReference,
    name: Option<&'static str>,
}

impl ModelAdapter for ReferenceReader {
    fn stream(&self, request: ModelRequest) -> AdapterFuture<'_, ModelStream> {
        Box::pin(async move {
            assert_eq!(self.agent, request.agent);
            let messages = request.body["messages"].as_array().unwrap();
            let mut events = vec![ModelEvent::StreamStart {
                request_id: "reader".into(),
            }];
            let reason = if let Some(tool) =
                messages.iter().find(|message| message["role"] == "tool")
            {
                let chunk: Value = serde_json::from_str(tool["content"].as_str().unwrap()).unwrap();
                assert_eq!(self.reference.id, chunk["output_id"]);
                assert!(chunk["segments"][0]["text"]
                    .as_str()
                    .unwrap()
                    .starts_with(ORIGINAL));
                events.push(ModelEvent::TextDelta {
                    text: "verified original reference".into(),
                });
                ModelStopReason::EndTurn
            } else {
                let user = messages
                    .iter()
                    .filter(|message| message["role"] == "user")
                    .filter_map(|message| message["content"].as_str())
                    .find(|text| text.contains(READ_TASK))
                    .expect("model request contains the delivered input");
                assert_eq!(self.name.is_none(), user.contains(&self.reference.id));
                assert!(!user.contains(ORIGINAL));
                assert!(!user.contains(REPLACEMENT));
                let args = match self.name {
                    Some(name) => json!({"name":name,"limit":128}),
                    None => json!({"id":self.reference.id,"limit":128}),
                };
                events.extend([
                    ModelEvent::ToolCallStart {
                        index: 0,
                        id: "read".into(),
                        name: "read_output".into(),
                    },
                    ModelEvent::ToolCallArgDelta {
                        index: 0,
                        fragment: args.to_string(),
                    },
                    ModelEvent::ToolCallStop { index: 0 },
                ]);
                ModelStopReason::ToolUse
            };
            events.push(ModelEvent::StopReason { reason });
            events.push(ModelEvent::StreamEnd);
            Ok(Box::pin(tokio_stream::iter(events.into_iter().map(Ok))) as ModelStream)
        })
    }
}

#[derive(Debug)]
struct DeferRead {
    id: String,
}

impl ToolAuthorizer for DeferRead {
    fn authorize(&self, action: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        assert_eq!("read_output", action.tool.name);
        assert_eq!(json!({"id": self.id, "limit": 128}), action.args);
        Ok(AuthorizationDecision::Defer)
    }
}

async fn runtime_record(session: &Session, agent: &str, operation: &str) -> Value {
    let mut observer = session.observe(0);
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(observation) = observer.next().await.unwrap() {
            let Observation::Committed(event) = observation else {
                continue;
            };
            let EventPayload::Unknown(event) = event.payload else {
                continue;
            };
            let record = &event["record"];
            if event["kind"] == "runtime.operation"
                && record["operation"] == operation
                && record["address"] == agent
            {
                return record.clone();
            }
        }
        panic!("session closed before '{operation}' for agent '{agent}'");
    })
    .await
    .expect("expected durable session fact")
}
