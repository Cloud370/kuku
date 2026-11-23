mod common;

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use httpmock::prelude::{MockServer, GET, POST};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

use kuku::event::{EventPayload, ModelStopReason};
use kuku::permission::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer};
use kuku::{
    query, AdapterFuture, AgentConfig, AgentStatus, Error, Input, ModelAdapter, ModelEvent,
    ModelRequest, ModelStream, OutputRead, Provider, Session, SessionOptions, UiEvent,
};

const WAIT: Duration = Duration::from_secs(10);
const BINARY: &[u8] = b"\0\xff\x1b[31m\x80\n\x01";
const TEXT_MARKER: &str = "FETCH_LITERAL_MATCH";

#[derive(Debug)]
struct Model(mpsc::UnboundedSender<Call>);

#[derive(Debug)]
struct Call {
    request: ModelRequest,
    respond: oneshot::Sender<Vec<ModelEvent>>,
}

impl ModelAdapter for Model {
    fn stream(&self, request: ModelRequest) -> AdapterFuture<'_, ModelStream> {
        Box::pin(async move {
            let (respond, receive) = oneshot::channel();
            self.0.send(Call { request, respond }).unwrap();
            let response = receive.await.unwrap();
            Ok(Box::pin(tokio_stream::iter(response.into_iter().map(Ok))) as ModelStream)
        })
    }
}

impl Call {
    fn tools(self, calls: &[(&str, &str, Value)]) {
        let mut events = start_response();
        for (index, (id, name, args)) in calls.iter().enumerate() {
            let index = index as u64;
            events.extend([
                ModelEvent::ToolCallStart {
                    index,
                    id: (*id).into(),
                    name: (*name).into(),
                },
                ModelEvent::ToolCallArgDelta {
                    index,
                    fragment: args.to_string(),
                },
                ModelEvent::ToolCallStop { index },
            ]);
        }
        self.finish(events, ModelStopReason::ToolUse);
    }

    fn done(self) {
        let mut events = start_response();
        events.push(ModelEvent::TextDelta {
            text: "done".into(),
        });
        self.finish(events, ModelStopReason::EndTurn);
    }

    fn finish(self, mut events: Vec<ModelEvent>, reason: ModelStopReason) {
        events.extend([ModelEvent::StopReason { reason }, ModelEvent::StreamEnd]);
        self.respond.send(events).unwrap();
    }
}

fn start_response() -> Vec<ModelEvent> {
    vec![ModelEvent::StreamStart {
        request_id: "fixture".into(),
    }]
}

async fn next_call(calls: &mut mpsc::UnboundedReceiver<Call>) -> Call {
    tokio::time::timeout(WAIT, calls.recv())
        .await
        .unwrap()
        .unwrap()
}

fn options(workspace: &Path, data: &Path) -> (SessionOptions, mpsc::UnboundedReceiver<Call>) {
    let (sender, calls) = mpsc::unbounded_channel();
    let mut options = SessionOptions::new(workspace, data, common::test_config());
    options.model_adapter = Some(Arc::new(Model(sender)));
    (options, calls)
}

async fn open(options: SessionOptions) -> Session {
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("main-agent", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("input", "main", "retrieve the response body"))
        .unwrap();
    session
}

async fn idle(session: &Session) {
    let mut observer = session.observe(0);
    let status = tokio::time::timeout(WAIT, async {
        loop {
            let agent = session.agent("main").unwrap();
            if agent.status != AgentStatus::Running {
                break agent.status;
            }
            observer.next().await.unwrap().unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(AgentStatus::Idle, status);
}

fn tool_result(session: &Session, id: &str) -> (String, String, Value) {
    session
        .events(0, 256)
        .unwrap()
        .into_iter()
        .find_map(|event| match event.payload {
            EventPayload::ToolResult {
                tool_call_id,
                status,
                model_content,
                structured: Some(facts),
                ..
            } if tool_call_id == id => Some((status, model_content, facts)),
            _ => None,
        })
        .expect("tool result has been committed")
}

fn outputs(session: &Session) -> PathBuf {
    session.events_path().parent().unwrap().join("outputs")
}

#[derive(Debug, Default)]
struct FetchReadPolicy(AtomicBool);

impl ToolAuthorizer for FetchReadPolicy {
    fn authorize(&self, action: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        Ok(
            if action.tool.name == "read_output" && self.0.load(Ordering::Acquire) {
                AuthorizationDecision::Deny
            } else {
                AuthorizationDecision::Allow
            },
        )
    }
}

#[tokio::test]
async fn download_hint_supports_local_json_extraction_under_read_output_authority() {
    let server = MockServer::start_async().await;
    let prefix = r#"{"count":7,"padding":""#;
    let payload = format!(
        "{prefix}{}中{TEXT_MARKER}\"}}",
        "x".repeat(65535 - prefix.len())
    );
    let download = server
        .mock_async(|when, then| {
            when.method(GET).path("/sample.json");
            then.status(200)
                .header(
                    "content-type",
                    "application/vnd.fixture+json; charset=utf-8",
                )
                .body(&payload);
        })
        .await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (mut options, mut calls) = options(workspace.path(), data.path());
    let policy = Arc::new(FetchReadPolicy::default());
    options.authorizer = policy.clone();
    let session = open(options).await;
    let fetch = json!({"url":server.url("/sample.json")});
    next_call(&mut calls)
        .await
        .tools(&[("download", "fetch_url", fetch.clone())]);

    let script_step = next_call(&mut calls).await;
    let (_, receipt, facts) = tool_result(&session, "download");
    let output_id = &facts["output_ref"]["id"];
    let selector = json!({"id":output_id});
    assert!(receipt.contains(&format!("JSON.parse(await tools.output_text({selector}))")));
    assert!(!script_step.request.body.to_string().contains(TEXT_MARKER));
    let code = format!(
        "const body = JSON.parse(await tools.output_text({selector})); return {{ count: body.count, suffix: body.padding.slice(-{}) === '中{TEXT_MARKER}' }};",
        TEXT_MARKER.len() + 1
    );
    script_step.tools(&[("extract", "run_js", json!({"code":code}))]);
    let final_step = next_call(&mut calls).await;
    let (status, _, extraction) = tool_result(&session, "extract");
    assert_eq!("ok", status);
    assert_eq!(json!({"count":7,"suffix":true}), extraction["value"]);
    assert_eq!(2, extraction["actions"][0]["result"]["pages"]);
    final_step.done();
    idle(&session).await;

    policy.0.store(true, Ordering::Release);
    session
        .send(Input::new(
            "restrict",
            "main",
            "verify the scoped output read",
        ))
        .unwrap();
    next_call(&mut calls)
        .await
        .tools(&[("denied", "run_js", json!({"code":code}))]);
    let final_step = next_call(&mut calls).await;
    let (status, denied, result) = tool_result(&session, "denied");
    assert_eq!("error", status);
    assert_eq!("blocked", result["actions"][0]["status"]);
    assert!(!denied.contains(TEXT_MARKER));
    final_step.done();
    idle(&session).await;

    session
        .configure_agent(
            "deselect",
            "main",
            AgentConfig {
                tools: Some(vec!["fetch_url".into(), "run_js".into()]),
                ..Default::default()
            },
        )
        .unwrap();
    session
        .send(Input::new(
            "metadata-only",
            "main",
            "download metadata only",
        ))
        .unwrap();
    next_call(&mut calls)
        .await
        .tools(&[("metadata", "fetch_url", fetch)]);
    let final_step = next_call(&mut calls).await;
    let (status, metadata, _) = tool_result(&session, "metadata");
    assert_eq!("ok", status);
    assert!(metadata.contains("retained with output ID"));
    assert!(!metadata.contains("tools.output_text") && !metadata.contains("read_output"));
    final_step.done();
    idle(&session).await;
    session.close().await.unwrap();
    download.assert_hits_async(2).await;
}

#[tokio::test]
async fn fetch_body_is_model_readable_and_agent_scoped_after_reopen() {
    let server = MockServer::start_async().await;
    let text = format!(
        "first page\n{}\n{TEXT_MARKER}\n",
        "fixture line\n".repeat(700)
    );
    let txt = server
        .mock_async(|when, then| {
            when.method(GET).path("/source.txt");
            then.status(200)
                .header("content-type", "text/plain; charset=utf-8")
                .body(&text);
        })
        .await;
    let bin = server
        .mock_async(|when, then| {
            when.method(GET).path("/blob.bin");
            then.status(404)
                .header("content-type", "application/octet-stream")
                .header(
                    "content-disposition",
                    "attachment; filename=\"dataset.bin\"",
                )
                .body(BINARY);
        })
        .await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (mut options, mut calls) = options(workspace.path(), data.path());
    let session = open(options.clone()).await;
    next_call(&mut calls).await.tools(&[
        (
            "text",
            "fetch_url",
            json!({"url":server.url("/source.txt")}),
        ),
        (
            "binary",
            "fetch_url",
            json!({"url":server.url("/blob.bin")}),
        ),
    ]);
    let second = next_call(&mut calls).await;
    let text_id = assert_download(
        &session,
        "text",
        200,
        "text/plain; charset=utf-8",
        "source.txt",
        text.as_bytes(),
    );
    let binary_id = assert_download(
        &session,
        "binary",
        404,
        "application/octet-stream",
        "dataset.bin",
        BINARY,
    );
    assert_ne!(text_id, binary_id);
    assert!(second.request.body.to_string().contains(&text_id));
    assert!(!second.request.body.to_string().contains(TEXT_MARKER));
    second.tools(&[("first", "read_output", json!({"id":text_id,"limit":32_768}))]);
    let third = next_call(&mut calls).await;
    let (status, _, first) = tool_result(&session, "first");
    assert_eq!("ok", status);
    assert_eq!(true, first["has_more"]);
    assert_eq!(8192, first["next_offset"]);
    assert!(first["segments"][0]["text"]
        .as_str()
        .unwrap()
        .starts_with("first page"));
    third.tools(&[(
        "search",
        "read_output",
        json!({"id":text_id,"offset":first["next_offset"],"search":TEXT_MARKER}),
    )]);
    let last = next_call(&mut calls).await;
    assert!(last.request.body.to_string().contains(TEXT_MARKER));
    let (status, _, matches) = tool_result(&session, "search");
    assert_eq!("ok", status);
    assert!(matches["segments"][0]["text"]
        .as_str()
        .unwrap()
        .contains(TEXT_MARKER));
    last.done();
    idle(&session).await;
    session
        .create_agent("peer-agent", "peer", AgentConfig::default())
        .unwrap();
    assert!(matches!(
        session.read_agent_output("peer", &binary_id, &OutputRead::default()),
        Err(Error::OutputUnavailable(_))
    ));
    options.session_id = Some(session.id().into());
    session.close().await.unwrap();
    let reopened = Session::open(options).await.unwrap();
    assert!(matches!(
        reopened.read_agent_output("peer", &binary_id, &OutputRead::default()),
        Err(Error::OutputUnavailable(_))
    ));
    let page = reopened
        .read_agent_output("main", &binary_id, &OutputRead::default())
        .unwrap();
    let bytes: Vec<u8> = page
        .segments
        .iter()
        .flat_map(|segment| segment.as_bytes())
        .copied()
        .collect();
    assert_eq!(BINARY, bytes);
    assert!(page.complete && !page.has_more && !page.capture_truncated);
    assert!(page
        .segments
        .iter()
        .any(|segment| segment.raw_bytes.is_some()));
    assert_eq!(0, fs::read_dir(workspace.path()).unwrap().count());
    reopened.close().await.unwrap();
    txt.assert_hits_async(1).await;
    bin.assert_hits_async(1).await;
}

fn assert_download(
    session: &Session,
    id: &str,
    status: u16,
    content_type: &str,
    name: &str,
    bytes: &[u8],
) -> String {
    let (outcome, content, facts) = tool_result(session, id);
    assert_eq!("ok", outcome);
    assert_eq!("fetch_url", facts["kind"]);
    assert_eq!(status, facts["status"]);
    assert_eq!(content_type, facts["content_type"]);
    assert_eq!(name, facts["filename"]);
    assert_eq!(bytes.len(), facts["size"]);
    assert_eq!(format!("{:x}", Sha256::digest(bytes)), facts["sha256"]);
    assert_eq!(true, facts["capture_complete"]);
    assert_eq!(0, facts["output_ref"]["offset"]);
    assert_eq!(bytes.len(), facts["output_ref"]["bytes"]);
    assert!(facts.get("file_path").is_none());
    let output = facts["output_ref"]["id"].as_str().unwrap();
    assert!(content.contains(&format!("read_output({})", json!({"id":output}))));
    output.into()
}

#[tokio::test]
async fn standalone_query_downloads_with_the_same_filename_are_independent() {
    let server = MockServer::start_async().await;
    let download = server
        .mock_async(|when, then| {
            when.method(GET).path("/document.bin");
            then.status(200).body(BINARY);
        })
        .await;
    let args = json!({"url":server.url("/document.bin")});
    let tool_calls: Vec<_> = ["first-download", "second-download"]
        .into_iter()
        .map(|id| json!({"id":id,"function":{"name":"fetch_url","arguments":args.to_string()}}))
        .collect();
    server
        .mock_async(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(common::openai_sse_response(json!({
                    "id": "fixture", "model": "fixture-model", "choices": [{
                        "message": {"tool_calls": tool_calls}, "finish_reason": "tool_calls",
                    }]
                })));
        })
        .await;
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut run = query("retrieve the response body twice")
        .provider(Provider::OpenAiCompatible)
        .model("fixture-model")
        .base_url(server.url("/v1"))
        .api_key("fixture-key")
        .workspace(workspace.path())
        .kuku_home(home.path())
        .start()
        .await
        .unwrap();
    let paths = tokio::time::timeout(WAIT, async {
        let mut paths = Vec::new();
        while paths.len() < 2 {
            if let UiEvent::ToolEnd {
                status,
                result: Some(facts),
                ..
            } = run.next().await.unwrap().unwrap()
            {
                assert_eq!("ok", status);
                assert_eq!("document.bin", facts["filename"]);
                assert!(facts.get("output_ref").is_none());
                let path = PathBuf::from(facts["file_path"].as_str().unwrap());
                assert_eq!(BINARY, fs::read(&path).unwrap());
                paths.push(path);
            }
        }
        paths
    })
    .await
    .unwrap();
    assert_ne!(paths[0], paths[1]);
    for path in paths {
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    download.assert_hits_async(2).await;
}

#[tokio::test]
async fn fragmented_download_fits_the_exact_framed_quota() {
    let payload = vec![b'x'; 2 * 8192 + 123];
    let (url, server) = chunked_response(&payload, 127, ChunkedEnding::Complete).await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (mut options, mut calls) = options(workspace.path(), data.path());
    let quota = 4096 + payload.len() as u64 + 3 * 5;
    options.limits.max_output_bytes = quota;
    let session = open(options).await;
    next_call(&mut calls)
        .await
        .tools(&[("fetch", "fetch_url", json!({"url":url}))]);
    let last = next_call(&mut calls).await;
    let (status, content, facts) = tool_result(&session, "fetch");
    assert_eq!("ok", status, "{content}");
    assert_eq!(payload.len(), facts["output_ref"]["bytes"]);
    assert_eq!(format!("{:x}", Sha256::digest(&payload)), facts["sha256"]);
    let id = facts["output_ref"]["id"].as_str().unwrap();
    assert_eq!(1, fs::read_dir(outputs(&session)).unwrap().count());
    assert_eq!(
        quota,
        fs::metadata(outputs(&session).join(format!("{id}.output")))
            .unwrap()
            .len()
    );
    let page = session
        .read_agent_output(
            "main",
            id,
            &OutputRead {
                limit: payload.len(),
                ..OutputRead::default()
            },
        )
        .unwrap();
    let stored: Vec<_> = page
        .segments
        .iter()
        .flat_map(|segment| segment.as_bytes())
        .copied()
        .collect();
    assert_eq!(payload, stored);
    assert!(page.complete && !page.has_more && !page.capture_truncated);
    last.done();
    idle(&session).await;
    session.close().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn streamed_download_that_exceeds_retention_rolls_back() {
    let payload = vec![b'x'; 2 * 8192];
    let (url, server) = chunked_response(&payload, 8192, ChunkedEnding::Complete).await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (mut options, mut calls) = options(workspace.path(), data.path());
    options.limits.max_output_bytes = 4096 + 8192 + 5;
    let session = open(options).await;
    next_call(&mut calls)
        .await
        .tools(&[("fetch", "fetch_url", json!({"url":url}))]);
    let last = next_call(&mut calls).await;
    let (status, content, facts) = tool_result(&session, "fetch");
    assert_eq!("error", status);
    assert!(content.contains("retained output quota"));
    assert!(facts.get("output_ref").is_none());
    assert_eq!(0, fs::read_dir(outputs(&session)).unwrap().count());
    last.done();
    idle(&session).await;
    session.close().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn incomplete_download_discards_partial_body() {
    let payload = vec![b'x'; 2 * 8192];
    let (url, server) = chunked_response(&payload, 8192, ChunkedEnding::Disconnect).await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (options, mut calls) = options(workspace.path(), data.path());
    let session = open(options).await;
    next_call(&mut calls)
        .await
        .tools(&[("fetch", "fetch_url", json!({"url":url}))]);
    let last = next_call(&mut calls).await;
    let (status, content, facts) = tool_result(&session, "fetch");
    assert_eq!("error", status);
    assert!(content.contains("failed to read response body"));
    assert!(facts.get("output_ref").is_none());
    assert_eq!(0, fs::read_dir(outputs(&session)).unwrap().count());
    last.done();
    idle(&session).await;
    session.close().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn declared_body_above_the_hard_limit_is_rejected_before_capture() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 52428801\r\nConnection: close\r\n\r\n";
    let (url, server) = response_fixture(response.to_vec(), false).await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (mut options, mut calls) = options(workspace.path(), data.path());
    options.limits.max_output_bytes = 64 * 1024 * 1024;
    let session = open(options).await;
    next_call(&mut calls)
        .await
        .tools(&[("fetch", "fetch_url", json!({"url":url}))]);
    let last = next_call(&mut calls).await;
    let (status, content, facts) = tool_result(&session, "fetch");
    assert_eq!("error", status);
    assert!(
        content.contains("download exceeds 50 MiB limit"),
        "{content}"
    );
    assert!(facts.get("output_ref").is_none());
    assert!(!outputs(&session).exists());
    last.done();
    idle(&session).await;
    session.close().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn active_download_is_unreadable_and_cancelled_draft_stays_unavailable_after_reopen() {
    let payload = vec![b'x'; 2 * 8192];
    let (url, server) = chunked_response(&payload, 8192, ChunkedEnding::Stall).await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (mut options, mut calls) = options(workspace.path(), data.path());
    let session = open(options.clone()).await;
    options.session_id = Some(session.id().into());
    next_call(&mut calls)
        .await
        .tools(&[("fetch", "fetch_url", json!({"url":url}))]);
    tokio::time::timeout(WAIT, async {
        loop {
            // NOTE: Windows directory entries can retain stale lengths while the writer is open.
            let has_payload = fs::read_dir(outputs(&session))
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .any(|entry| {
                    fs::File::open(entry.path())
                        .and_then(|file| file.metadata())
                        .is_ok_and(|metadata| metadata.len() > 4096)
                });
            if has_payload {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let id = claimed_output_id(&session);
    let draft = outputs(&session).join(format!("{id}.output-draft"));
    assert_output_unavailable(&session, &id);
    assert!(matches!(
        Session::open(options.clone()).await,
        Err(Error::SessionLocked { .. })
    ));
    assert!(fs::File::open(&draft).unwrap().metadata().unwrap().len() > 4096);

    session.cancel("main").await.unwrap();
    let (status, _, facts) = tool_result(&session, "fetch");
    assert_eq!("cancelled", status);
    assert!(facts.get("output_ref").is_none());
    assert_eq!(0, fs::read_dir(outputs(&session)).unwrap().count());
    session.close().await.unwrap();
    tokio::time::timeout(WAIT, server).await.unwrap().unwrap();

    fs::write(&draft, b"unpublished bytes left by an interrupted writer").unwrap();
    let unrelated = outputs(&session).join("outside-staging-namespace.txt");
    fs::write(&unrelated, b"unrelated data").unwrap();
    let reopened = Session::open(options).await.unwrap();
    assert_output_unavailable(&reopened, &id);
    assert!(!draft.exists());
    assert!(unrelated.is_file());
    reopened.close().await.unwrap();
}

fn claimed_output_id(session: &Session) -> String {
    session
        .events(0, 256)
        .unwrap()
        .into_iter()
        .find_map(|event| {
            let EventPayload::Unknown(value) = event.payload else {
                return None;
            };
            let record = &value["record"];
            (value["kind"] == "runtime.operation" && record["operation"] == "output_reserved")
                .then(|| format!("out_{}", event.id))
        })
        .expect("the Session reserved an output for the running fetch")
}

fn assert_output_unavailable(session: &Session, id: &str) {
    let request = OutputRead::default();
    assert!(
        matches!(session.read_agent_output("main", id, &request), Err(Error::OutputUnavailable(output)) if output == id)
    );
    assert!(
        matches!(session.read_output(id, &request), Err(Error::OutputUnavailable(output)) if output == id)
    );
}

enum ChunkedEnding {
    Complete,
    Disconnect,
    Stall,
}

async fn chunked_response(
    payload: &[u8],
    chunk_size: usize,
    ending: ChunkedEnding,
) -> (String, tokio::task::JoinHandle<()>) {
    let mut response =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
    for chunk in payload.chunks(chunk_size) {
        response.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        response.extend_from_slice(chunk);
        response.extend_from_slice(b"\r\n");
    }
    if matches!(ending, ChunkedEnding::Complete) {
        response.extend_from_slice(b"0\r\n\r\n");
    }
    response_fixture(response, matches!(ending, ChunkedEnding::Stall)).await
}

async fn response_fixture(response: Vec<u8>, stall: bool) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/fixture", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.ends_with(b"\r\n\r\n") {
            let size = socket.read(&mut buffer).await.unwrap();
            assert!(size > 0);
            request.extend_from_slice(&buffer[..size]);
        }
        socket.write_all(&response).await.unwrap();
        if stall {
            match socket.read(&mut buffer).await {
                Ok(0) => {}
                Err(error) if error.kind() == ErrorKind::ConnectionReset => {}
                result => panic!("fetch connection remained open: {result:?}"),
            }
        }
    });
    (url, task)
}
