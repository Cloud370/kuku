use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use httpmock::prelude::{HttpMockRequest, MockServer, POST};
use kuku::config::{
    ImageInputSupport, ModelCapabilityDeclaration, ModelCapabilityEntry, ProviderFormat,
    ProviderProfile,
};
use kuku::event::EventPayload;
use kuku::permission::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer};
use kuku::{AgentConfig, AgentStatus, Input, Session, SessionOptions};
use serde_json::{Value, json};

use super::{common, until};

const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAAAAAA6fptVAAAACklEQVR4nGNgAAAAAgABSK+kcQAAAABJRU5ErkJggg==";
const ENDPOINT: &str = "/workspace-media/responses";
const CHAT_ENDPOINT: &str = "/workspace-media-chat/chat/completions";

#[derive(Debug)]
struct Gate;

impl ToolAuthorizer for Gate {
    fn authorize(&self, action: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        let path = Path::new(action.args["path"].as_str().unwrap());
        assert_eq!("view_image", action.tool.name);
        Ok(if path == Path::new("pics/hidden.png") {
            AuthorizationDecision::Deny
        } else {
            AuthorizationDecision::Allow
        })
    }
}

#[tokio::test]
async fn authorized_image_replays_from_retained_bytes_after_reopen() {
    let server = MockServer::start_async().await;
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let png = STANDARD.decode(PNG).unwrap();
    std::fs::create_dir(work.path().join("pics")).unwrap();
    std::fs::write(work.path().join("pics/screenshot.png"), &png).unwrap();
    std::fs::write(work.path().join("pics/hidden.png"), &png).unwrap();
    std::fs::write(work.path().join("over.png"), vec![0; 1024 * 1024 + 1]).unwrap();
    std::fs::write(work.path().join("text.png"), b"not an image").unwrap();

    let tools = tool_response(&[
        ("allowed", "pics/screenshot.png"),
        ("denied", "pics/./hidden.png"),
        ("over", "over.png"),
        ("text", "text.png"),
    ]);
    let initial = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(ENDPOINT)
                .matches(initial_image_request);
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(tools);
        })
        .await;
    let finish = server.mock_async(|when, then| {
        when.method(POST).path(ENDPOINT).matches(continuation);
        then.status(200).header("content-type", "text/event-stream").body(concat!(
            "event: response.output_text.delta\ndata: {\"delta\":\"image check complete\"}\n\n",
            "event: response.completed\ndata: {\"response\":{\"id\":\"media-final\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}}\n\n"
        ));
    }).await;

    let config = media_config(&server, "/workspace-media");
    let mut options = SessionOptions::new(work.path(), data.path(), config);
    options.authorizer = Arc::new(Gate);
    options.limits.output_retention_secs = 0;

    let session = Session::open(options.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    assert!(
        session
            .agent_selection("main")
            .unwrap()
            .tools
            .iter()
            .any(|tool| tool.name == "view_image")
    );
    let text_only = AgentConfig {
        connection: Some("openai".into()),
        model: Some("text-fixture".into()),
        ..Default::default()
    };
    assert!(
        !session
            .selection_for_config("main", text_only)
            .unwrap()
            .tools
            .iter()
            .any(|tool| tool.name == "view_image")
    );
    run(&session, "first", "inspect these four images").await;
    let id = session.id().to_owned();
    let events = session.events(0, 256).unwrap();
    let results = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ToolResult {
                tool_call_id,
                status,
                ..
            } => Some((tool_call_id.as_str(), status.as_str())),
            _ => None,
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        results,
        std::collections::BTreeMap::from([
            ("allowed", "ok"),
            ("denied", "blocked"),
            ("over", "error"),
            ("text", "error"),
        ])
    );
    session.close().await.unwrap();
    std::fs::remove_file(work.path().join("pics/screenshot.png")).unwrap();

    options.session_id = Some(id);
    let reopened = Session::open(options).await.unwrap();
    run(&reopened, "second", "use the image from the previous turn").await;
    run(&reopened, "third", "independent text-only question").await;
    reopened.close().await.unwrap();
    let journal = std::fs::read_to_string(reopened.events_path()).unwrap();
    assert!(journal.contains("model_image_saved"));
    assert!(!journal.contains(PNG));
    initial.assert_hits_async(1).await;
    finish.assert_hits_async(3).await;

    let bodies = common::take_http_requests(ENDPOINT);
    let wire_image = format!("data:image/png;base64,{PNG}");
    assert_eq!(3, bodies.len());
    for (index, body) in bodies.iter().enumerate() {
        let output = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["type"] == "function_call_output" && item["call_id"] == "allowed")
            .unwrap();
        if index < 2 {
            assert_eq!(
                output["output"][1],
                json!({"type":"input_image", "image_url":wire_image})
            );
            assert_eq!(1, body.to_string().matches("data:image/").count());
        } else {
            assert!(output["output"].is_string());
        }
    }
}

#[tokio::test]
async fn switching_to_text_only_reports_modalities_and_omits_a_recent_image() {
    const SWITCH_ENDPOINT: &str = "/image-route-switch/responses";
    const DONE: &str = concat!(
        "event: response.output_text.delta\ndata: {\"delta\":\"image check complete\"}\n\n",
        "event: response.completed\ndata: {\"response\":{\"id\":\"media-done\",\"status\":\"completed\",\"output\":[]}}\n\n"
    );
    let server = MockServer::start_async().await;
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(work.path().join("photo.png"), STANDARD.decode(PNG).unwrap()).unwrap();

    let vision = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(SWITCH_ENDPOINT)
                .matches(initial_image_request)
                .body_contains("You receive text, image.")
                .body_contains("You can see image attachments in kuku.")
                .body_contains("When the task depends on the visual contents of an existing workspace image, prefer view_image if available.")
                .body_contains("Image input is supported. The view_image tool is available.");
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(tool_response(&[("fresh", "photo.png")]));
        })
        .await;
    let image_result = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(SWITCH_ENDPOINT)
                .body_contains("data:image/png;base64,");
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(DONE);
        })
        .await;
    let text_only = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(SWITCH_ENDPOINT)
                .matches(|request| {
                    let payload = body(request);
                    payload["model"] == "text-fixture"
                        && !payload.to_string().contains("data:image/")
                        && payload["tools"].as_array().is_some_and(|tools| {
                            tools.iter().all(|tool| tool["name"] != "view_image")
                        })
                        && payload["input"].as_array().is_some_and(|items| {
                            items.iter().any(|item| {
                                item["type"] == "function_call_output"
                                    && item["output"].as_str().is_some_and(|output| {
                                        output.starts_with("Image photo.png (")
                                    })
                            })
                        })
                })
                .body_contains(
                    "Image input is unsupported by the model. The view_image tool is unavailable.",
                )
                .body_contains("You receive text.")
                .body_contains("You cannot see any images in kuku.")
                .body_contains("Never guess what an unseen image contains.");
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(DONE);
        })
        .await;

    let config = media_config(&server, "/image-route-switch");
    let session = Session::open(SessionOptions::new(work.path(), data.path(), config))
        .await
        .unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    run(&session, "inspect", "View the PNG.").await;
    let text = AgentConfig {
        connection: Some("openai".into()),
        model: Some("text-fixture".into()),
        ..Default::default()
    };
    session.configure_agent("switch", "main", text).unwrap();
    run(&session, "describe", "What did the previous tool return?").await;
    assert_eq!(
        session.agent_selection("main").unwrap().image_input,
        ImageInputSupport::UnsupportedByModel
    );
    session.close().await.unwrap();
    vision.assert_hits_async(1).await;
    image_result.assert_hits_async(1).await;
    text_only.assert_hits_async(1).await;
}

fn media_config(server: &MockServer, path: &str) -> kuku::config::Config {
    let mut config = common::test_config();
    let provider = config.providers.get_mut("openai").unwrap();
    provider.base_url = server.url(path);
    provider.format = ProviderFormat::OpenAiResponses;
    provider.options.profile = ProviderProfile::DeepSeek;
    for (model, input) in [
        ("vision-fixture", vec!["text", "image"]),
        ("text-fixture", vec!["text"]),
    ] {
        provider.options.model_capabilities.insert(
            model.into(),
            ModelCapabilityEntry {
                manual: Some(ModelCapabilityDeclaration {
                    input_modalities: Some(input.into_iter().map(str::to_owned).collect()),
                    output_modalities: Some(vec!["text".into()]),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
    }
    let settings = common::set_test_model(&mut config, "openai", "vision-fixture").clone();
    config
        .models
        .insert(kuku::ModelRef::new("openai", "text-fixture"), settings);
    config
}

async fn run(session: &Session, id: &str, input: &str) {
    session.send(Input::new(id, "main", input)).unwrap();
    let snapshot = until(|| {
        let snapshot = session.agent("main").unwrap();
        (snapshot.status != AgentStatus::Running).then_some(snapshot)
    })
    .await;
    assert_eq!(snapshot.last_error, None);
    assert_eq!(
        snapshot.last_output.as_deref(),
        Some("image check complete")
    );
}

fn body(request: &HttpMockRequest) -> Value {
    serde_json::from_slice(request.body.as_deref().unwrap_or_default()).unwrap()
}

fn initial_image_request(request: &HttpMockRequest) -> bool {
    let body = body(request);
    !has_tool_results(&body)
        && !body
            .to_string()
            .contains("You cannot see any images in kuku.")
        && body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "view_image")
}

fn continuation(request: &HttpMockRequest) -> bool {
    has_tool_results(&body(request)) && common::capture_http_request(request)
}

fn has_tool_results(body: &Value) -> bool {
    body["input"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["type"] == "function_call_output")
}

fn tool_response(calls: &[(&str, &str)]) -> String {
    let mut output = Vec::new();
    let mut stream = String::new();
    for (index, (id, path)) in calls.iter().enumerate() {
        let mut item = json!({"type":"function_call", "id":format!("item-{id}"), "call_id":id,
            "name":"view_image", "status":"in_progress", "arguments":""});
        stream.push_str(&format!(
            "event: response.output_item.added\ndata: {}\n\n",
            json!({"output_index":index, "item":item})
        ));
        let arguments = json!({"path":path}).to_string();
        stream.push_str(&format!(
            "event: response.function_call_arguments.delta\ndata: {}\n\n",
            json!({"output_index":index, "item_id":item["id"], "delta":arguments})
        ));
        item["status"] = json!("completed");
        item["arguments"] = json!(arguments);
        stream.push_str(&format!(
            "event: response.output_item.done\ndata: {}\n\n",
            json!({"output_index":index, "item":item})
        ));
        output.push(item);
    }
    stream.push_str(&format!(
        "event: response.completed\ndata: {}\n\n",
        json!({"response":{
            "id":"media-tools", "status":"completed", "output":output,
            "usage":{"input_tokens":10,"output_tokens":2}
        }})
    ));
    stream
}

#[tokio::test]
async fn chat_completions_sends_attachments_and_preserves_image_tool_batches() {
    let server = MockServer::start_async().await;
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let png = STANDARD.decode(PNG).unwrap();
    let other = png_with_comment(32, b'A');
    std::fs::create_dir(work.path().join("pics")).unwrap();
    std::fs::write(work.path().join("pics/screenshot.png"), &png).unwrap();
    std::fs::write(work.path().join("pics/other.png"), &other).unwrap();
    std::fs::write(work.path().join("pics/hidden.png"), &other).unwrap();
    let tool_calls = [
        ("first", "screenshot"),
        ("second", "other"),
        ("denied", "hidden"),
    ]
    .map(|(id, file)| {
        json!({"id": id, "type": "function", "function": {
            "name": "view_image",
            "arguments": json!({"path": format!("pics/{file}.png")}).to_string(),
        }})
    });
    let tools = common::openai_sse_response(json!({"choices": [{
        "message": {"tool_calls": tool_calls}, "finish_reason": "tool_calls",
    }]}));
    let done = common::openai_sse_response(json!({"choices": [{
        "message": {"content": "image check complete"}, "finish_reason": "stop",
    }]}));
    let initial = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(CHAT_ENDPOINT)
                .body_contains("Image input is supported. The view_image tool is available.")
                .matches(|request| {
                    let body = body(request);
                    body["tools"].as_array().unwrap().iter()
                        .any(|tool| tool["function"]["name"] == "view_image")
                })
                .matches(|request| {
                    let body = body(request);
                    let messages = body["messages"].as_array().unwrap();
                    let content = &messages.last().unwrap()["content"];
                    content == &json!([
                        {"type": "text", "text": "Compare this attachment and the files"},
                        {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{PNG}")}},
                    ])
                    && !messages.iter().any(|message| message["role"] == "tool")
                });
            then.status(200).header("content-type", "text/event-stream").body(tools);
        })
        .await;
    let continuation = server
        .mock_async(|when, then| {
            when.method(POST).path(CHAT_ENDPOINT).matches(|request| {
                let body = body(request);
                let messages = body["messages"].as_array().unwrap();
                let screenshot = format!("data:image/png;base64,{PNG}");
                let other_url = format!(
                    "data:image/png;base64,{}",
                    STANDARD.encode(png_with_comment(32, b'A'))
                );
                let Some(batch) = messages
                    .windows(6)
                    .find(|batch| batch[0]["tool_calls"].is_array())
                else {
                    return false;
                };
                body["model"] == "vision-fixture"
                    && batch[0]["role"] == "assistant"
                    && ["first", "second", "denied"]
                        .into_iter()
                        .zip(&batch[1..4])
                        .all(|(id, m)| {
                            m["role"] == "tool"
                                && m["tool_call_id"] == id
                                && m["content"].is_string()
                        })
                    && [("first", &screenshot), ("second", &other_url)]
                        .into_iter()
                        .zip(&batch[4..6])
                        .all(|((id, url), m)| {
                            m["role"] == "user"
                                && m["content"][0]["text"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .contains(&format!("tool call \"{id}\""))
                                && m["content"][1]["image_url"]["url"] == url.as_str()
                        })
                    && messages
                        .iter()
                        .filter_map(|m| m["content"].as_array())
                        .flatten()
                        .filter(|part| part["type"] == "image_url")
                        .count()
                        == 3
            });
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(&done);
        })
        .await;
    let text_only = server
        .mock_async(|when, then| {
            when.method(POST).path(CHAT_ENDPOINT).matches(|request| {
                let body = body(request);
                body["model"] == "text-fixture"
                    && !body.to_string().contains("data:image/")
                    && body["tools"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|tool| tool["function"]["name"] != "view_image")
            });
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(&done);
        })
        .await;
    let mut config = media_config(&server, "/workspace-media-chat");
    let provider = config.providers.get_mut("openai").unwrap();
    provider.format = ProviderFormat::OpenAiChat;
    provider.options.profile = ProviderProfile::Compatible;
    let mut options = SessionOptions::new(work.path(), data.path(), config);
    options.authorizer = Arc::new(Gate);
    options.limits.output_retention_secs = 0;
    let session = Session::open(options.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    assert_eq!(
        session.agent_selection("main").unwrap().image_input,
        ImageInputSupport::Supported
    );
    let mut input = Input::new("input", "main", "Compare this attachment and the files");
    input
        .images
        .push(session.save_input_image("main", &png).await.unwrap());
    session.send(input).unwrap();
    let state = session.wait_for_agent("main").await.unwrap();
    assert_eq!(state.last_error, None);
    assert_eq!(state.last_output.as_deref(), Some("image check complete"));
    options.session_id = Some(session.id().into());
    session.close().await.unwrap();
    std::fs::remove_file(work.path().join("pics/screenshot.png")).unwrap();

    let reopened = Session::open(options).await.unwrap();
    run(&reopened, "replay", "Compare the images again").await;
    reopened
        .configure_agent(
            "switch",
            "main",
            AgentConfig {
                connection: Some("openai".into()),
                model: Some("text-fixture".into()),
                ..Default::default()
            },
        )
        .unwrap();
    run(&reopened, "text", "A text-only question").await;
    reopened.close().await.unwrap();
    assert!(
        !std::fs::read_to_string(reopened.events_path())
            .unwrap()
            .contains(PNG)
    );
    initial.assert_hits_async(1).await;
    continuation.assert_hits_async(2).await;
    text_only.assert_hits_async(1).await;
}

#[tokio::test]
async fn user_image_reopens_and_reaches_only_an_image_capable_route() {
    const PATH: &str = "/pasted-image/responses";
    const DONE: &str = concat!(
        "event: response.output_text.delta\ndata: {\"delta\":\"image check complete\"}\n\n",
        "event: response.completed\ndata: {\"response\":{\"id\":\"pasted\",\"status\":\"completed\",\"output\":[]}}\n\n"
    );
    let server = MockServer::start_async().await;
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let vision = server
        .mock_async(|when, then| {
            when.method(POST).path(PATH).matches(|request| {
                body(request)["input"].as_array().is_some_and(|items| {
                    let captions = items
                        .iter()
                        .filter(|item| item["role"] == "user")
                        .filter_map(|item| item["content"].as_array())
                        .flatten()
                        .filter(|part| part["text"] == "Describe [Image #1]")
                        .count();
                    captions == 1
                        && items.iter().any(|item| {
                            item["role"] == "user"
                                && item["content"].as_array().is_some_and(|parts| {
                                    parts
                                        .iter()
                                        .any(|part| part["text"] == "Describe [Image #1]")
                                        && parts.iter().any(|part| {
                                            part["image_url"]
                                                == format!("data:image/png;base64,{PNG}")
                                        })
                                })
                        })
                })
            });
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(DONE);
        })
        .await;
    let text_only = server
        .mock_async(|when, then| {
            when.method(POST).path(PATH).matches(|request| {
                let body = body(request);
                body["model"] == "text-fixture" && !body.to_string().contains("data:image/")
            });
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(DONE);
        })
        .await;
    let mut options = SessionOptions::new(
        work.path(),
        data.path(),
        media_config(&server, "/pasted-image"),
    );
    let session = Session::open(options.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    let mut input = Input::new("pasted", "main", "Describe [Image #1]");
    let image = session
        .save_input_image("main", &STANDARD.decode(PNG).unwrap())
        .await
        .unwrap();
    input.images.push(image.clone());
    session.send(input.clone()).unwrap();
    let finished = session.wait_for_agent("main").await.unwrap();
    assert_eq!(
        finished.last_output.as_deref(),
        Some("image check complete")
    );
    options.session_id = Some(session.id().into());
    session.close().await.unwrap();

    let saved =
        Session::read_saved_input_images(session.events_path(), "main", &input.images).unwrap();
    assert_eq!(saved, vec![STANDARD.decode(PNG).unwrap()]);
    assert!(
        Session::read_saved_input_images(session.events_path(), "other", &input.images).is_err()
    );
    let reopened = Session::open(options).await.unwrap();
    assert_eq!(reopened.send(input).unwrap().command_id, "pasted");
    run(&reopened, "follow-up", "Now describe the same image again").await;
    reopened
        .configure_agent(
            "switch",
            "main",
            AgentConfig {
                connection: Some("openai".into()),
                model: Some("text-fixture".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let mut incompatible = Input::new("blocked", "main", "[Image #1]");
    incompatible.images.push(image);
    assert!(
        reopened
            .send(incompatible)
            .unwrap_err()
            .to_string()
            .contains("cannot receive images")
    );
    assert!(reopened.receipt("blocked").unwrap().is_none());
    run(&reopened, "text", "A text question").await;
    reopened.close().await.unwrap();
    assert!(
        !std::fs::read_to_string(reopened.events_path())
            .unwrap()
            .contains(PNG)
    );
    vision.assert_hits_async(2).await;
    text_only.assert_hits_async(1).await;
}

const IMAGE_DONE: &str = concat!(
    "event: response.output_text.delta\ndata: {\"delta\":\"image check complete\"}\n\n",
    "event: response.completed\ndata: {\"response\":{\"id\":\"media-regression\",\"status\":\"completed\",\"output\":[]}}\n\n"
);

fn png_with_comment(size: usize, byte: u8) -> Vec<u8> {
    let png = STANDARD.decode(PNG).unwrap();
    let mut result = png[..png.len() - 12].to_vec();
    let mut chunk = b"tEXtComment\0".to_vec();
    chunk.extend(std::iter::repeat_n(byte, size));
    result.extend(u32::try_from(chunk.len() - 4).unwrap().to_be_bytes());
    result.extend(&chunk);
    result.extend(crc32fast::hash(&chunk).to_be_bytes());
    result.extend(&png[png.len() - 12..]);
    result
}

#[tokio::test]
async fn queued_images_follow_the_edit_and_stay_in_separate_messages() {
    const PATH: &str = "/queued-media/responses";
    let server = MockServer::start_async().await;
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path(PATH).matches(|request| {
                body(request)["input"].as_array().is_some_and(|items| {
                    items
                        .iter()
                        .filter(|item| item["role"] == "user")
                        .all(|item| {
                            item["content"].as_array().is_some_and(|parts| {
                                let caption = parts
                                    .iter()
                                    .filter_map(|part| part["text"].as_str())
                                    .collect::<String>();
                                let images = parts
                                    .iter()
                                    .filter(|part| part["type"] == "input_image")
                                    .count();
                                caption.matches("[Image #1]").count() <= 1
                                    && (!caption.contains("Edited plain text") || images == 0)
                                    && (!caption.contains("Revised caption") || images == 1)
                            })
                        })
                })
            });
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(IMAGE_DONE);
        })
        .await;
    let options = SessionOptions::new(
        work.path(),
        data.path(),
        media_config(&server, "/queued-media"),
    );
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    let image = session
        .save_input_image("main", &STANDARD.decode(PNG).unwrap())
        .await
        .unwrap();
    for (id, caption) in [
        ("removed", "Remove this"),
        ("kept", "Original caption"),
        ("separate", "Separate caption"),
    ] {
        let mut input = Input::new(id, "main", format!("{caption} [Image #1]"));
        input.activation = kuku::Activation::Enqueue;
        input.timing = kuku::InputTiming::NextTurn;
        input
            .images
            .push(image.clone().with_marker(kuku::execution::ImageMarker {
                offset: caption.len() + 1,
                label: "[Image #1]".into(),
            }));
        session.send(input.clone()).unwrap();
        if id == "removed" {
            session
                .replace_queued(
                    "edit-removed",
                    "main",
                    id,
                    "Edited plain text",
                    &input.source,
                )
                .unwrap();
        } else if id == "kept" {
            session
                .replace_queued_with_images(
                    "edit-kept",
                    "main",
                    id,
                    "Revised caption [Image #1]",
                    &input.source,
                    &[kuku::execution::ImageMarker {
                        offset: "Revised caption ".len(),
                        label: "[Image #1]".into(),
                    }],
                )
                .unwrap();
        }
    }
    let pending = session.pending_inputs("main").unwrap();
    assert!(pending[0].input.images.is_empty());
    assert_eq!(
        pending[1].input.images[0].marker().unwrap().offset,
        "Revised caption ".len()
    );
    for _ in 0..3 {
        session.advance("main").unwrap();
        let finished = session.wait_for_agent("main").await.unwrap();
        assert_eq!(finished.last_error, None);
    }
    assert!(session.pending_inputs("main").unwrap().is_empty());
    mock.assert_hits_async(3).await;
    session.close().await.unwrap();
}

#[tokio::test]
async fn history_images_yield_request_budget_to_the_current_input() {
    const PATH: &str = "/media-budget/responses";
    let server = MockServer::start_async().await;
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let pngs = (*b"ABC").map(|byte| png_with_comment(24_000, byte));
    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path(PATH).matches(|request| {
                let urls = (*b"ABC").map(|byte| {
                    format!(
                        "data:image/png;base64,{}",
                        STANDARD.encode(png_with_comment(24_000, byte))
                    )
                });
                let body = body(request);
                let content: Vec<_> = body["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|item| item["content"].as_array())
                    .flatten()
                    .collect();
                let current = content
                    .iter()
                    .filter_map(|part| part["text"].as_str())
                    .rfind(|text| text.starts_with("Image turn ") || *text == "Text follow-up");
                let images: Vec<_> = content
                    .iter()
                    .filter_map(|part| part["image_url"].as_str())
                    .collect();
                match current {
                    Some("Image turn 0") => images == [&urls[0]],
                    Some("Image turn 1") => images == [&urls[0], &urls[1]],
                    Some("Image turn 2" | "Text follow-up") => images == [&urls[1], &urls[2]],
                    _ => false,
                }
            });
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(IMAGE_DONE);
        })
        .await;
    let mut options = SessionOptions::new(
        work.path(),
        data.path(),
        media_config(&server, "/media-budget"),
    );
    options.limits.max_request_bytes = Some(110 * 1024);
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    for (index, png) in pngs.into_iter().enumerate() {
        let mut input = Input::new(
            format!("image-{index}"),
            "main",
            format!("Image turn {index}"),
        );
        input
            .images
            .push(session.save_input_image("main", &png).await.unwrap());
        session.send(input).unwrap();
        let snapshot = session.wait_for_agent("main").await.unwrap();
        assert_eq!(snapshot.last_error, None);
    }
    run(&session, "plain", "Text follow-up").await;
    mock.assert_hits_async(4).await;
    session.close().await.unwrap();
}

#[tokio::test]
async fn queued_images_get_a_request_each_and_follow_their_markers() {
    const PATH: &str = "/steered-images/responses";
    let server = MockServer::start_async().await;
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let prompts = work.path().join("prompts");
    std::fs::create_dir_all(prompts.join("agents")).unwrap();
    std::fs::write(
        prompts.join("agents/main.md"),
        r#"{% if agent.image_input.supported %}The main template sees images.{% endif %}"#,
    )
    .unwrap();

    let config = media_config(&server, "/steered-images");
    let mut options = SessionOptions::new(work.path(), data.path(), config);
    options.prompts_dir = Some(prompts);
    options.limits.max_request_bytes = Some(45 * 1024);
    let session = Session::open(options).await.unwrap();
    session
        .create_agent(
            "create",
            "main",
            AgentConfig {
                tools: Some(Vec::new()),
                no_skills: true,
                no_agents: true,
                ..Default::default()
            },
        )
        .unwrap();

    let mut first = Input::new("first", "main", "First [Image #1] and [Image #2]");
    let mut second = Input::new("second", "main", "Second [Image #1]");
    for (input, images) in [
        (
            &mut first,
            &[(b'B', "[Image #2]", 10_000), (b'A', "[Image #1]", 10_000)][..],
        ),
        (&mut second, &[(b'C', "[Image #1]", 24_000)][..]),
    ] {
        for &(byte, label, size) in images {
            input.images.push(
                session
                    .save_input_image("main", &png_with_comment(size, byte))
                    .await
                    .unwrap()
                    .with_marker(kuku::execution::ImageMarker {
                        label: label.into(),
                        offset: input.text.find(label).unwrap(),
                    }),
            );
        }
        input.activation = kuku::Activation::Enqueue;
        session.send(input.clone()).unwrap();
    }
    let mock = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(PATH)
                .body_contains("The main template sees images.")
                .matches(|request| {
                    let request = body(request);
                    let captions = ["First [Image #1] and [Image #2]", "Second [Image #1]"];
                    let current =
                        request["input"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .rev()
                            .find_map(|item| {
                                let parts = item["content"].as_array()?;
                                let caption = parts
                                    .iter()
                                    .filter_map(|part| part["text"].as_str())
                                    .find(|text| captions.contains(text))?;
                                Some((caption, parts))
                            });
                    let Some((caption, parts)) = current else {
                        return false;
                    };
                    let expected: &[(u8, usize)] = if caption == captions[0] {
                        &[(b'A', 10_000), (b'B', 10_000)]
                    } else {
                        &[(b'C', 24_000)]
                    };
                    parts
                        .iter()
                        .filter_map(|part| part["image_url"].as_str())
                        .eq(expected.iter().map(|&(byte, size)| {
                            format!(
                                "data:image/png;base64,{}",
                                STANDARD.encode(png_with_comment(size, byte))
                            )
                        }))
                });
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(IMAGE_DONE);
        })
        .await;
    for id in ["first", "second"] {
        assert_eq!(
            session.input_status("main", id).unwrap(),
            kuku::InputStatus::Queued
        );
        session.advance("main").unwrap();
        let finished = session.wait_for_agent("main").await.unwrap();
        assert_eq!(finished.last_error, None, "delivery {id}");
        assert_eq!(
            session.input_status("main", id).unwrap(),
            kuku::InputStatus::Accepted
        );
    }
    mock.assert_hits_async(2).await;
    session.close().await.unwrap();
}

#[tokio::test]
async fn an_expired_queued_image_is_protected_until_withdrawn() {
    let server = MockServer::start_async().await;
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut options = SessionOptions::new(
        work.path(),
        data.path(),
        media_config(&server, "/retention"),
    );
    options.limits.output_retention_secs = 0;
    options.limits.max_retained_output_bytes = 10_000;
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    let mut input = Input::new("queued", "main", "[Image #1]");
    input.activation = kuku::Activation::Enqueue;
    input.images.push(
        session
            .save_input_image("main", &png_with_comment(2500, b'A'))
            .await
            .unwrap(),
    );
    session.send(input).unwrap();
    let next = png_with_comment(2500, b'B');
    assert!(matches!(
        session.save_input_image("main", &next).await,
        Err(kuku::Error::OutputQuotaExceeded { .. })
    ));
    session
        .withdraw_input("withdraw", "main", "queued")
        .unwrap();
    session.save_input_image("main", &next).await.unwrap();
    session.close().await.unwrap();
}

#[tokio::test]
async fn image_staging_validates_content_and_protects_each_attachment_until_enqueue() {
    let server = MockServer::start_async().await;
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut options =
        SessionOptions::new(work.path(), data.path(), media_config(&server, "/staging"));
    options.session_id = Some("image-staging".into());
    options.limits.output_retention_secs = 0;
    let session = Session::open(options.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    assert!(
        session
            .save_input_image("main", b"\x89PNG\r\n\x1a\n")
            .await
            .is_err()
    );
    let mut input = Input::new("queued", "main", "Two images");
    input.activation = kuku::Activation::Enqueue;
    for byte in *b"XY" {
        input.images.push(
            session
                .save_input_image("main", &png_with_comment(500, byte))
                .await
                .unwrap(),
        );
    }
    session.send(input).unwrap();
    assert_eq!(
        session.pending_inputs("main").unwrap()[0]
            .input
            .images
            .len(),
        2
    );
    session
        .withdraw_input("withdraw", "main", "queued")
        .unwrap();
    let bytes = png_with_comment(500, b'X');
    let restaged = session.save_input_image("main", &bytes).await.unwrap();
    session.close().await.unwrap();

    let reopened = Session::open(options).await.unwrap();
    assert_eq!(reopened.read_input_image("main", &restaged).unwrap(), bytes);
    let mut input = Input::new("resend", "main", "Restaged image");
    input.activation = kuku::Activation::Enqueue;
    input.images.push(restaged);
    reopened.send(input).unwrap();
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn current_images_can_trigger_compaction_before_inference() {
    const PATH: &str = "/image-compaction/responses";
    const REPLY: &str = concat!(
        "event: response.output_text.delta\ndata: {\"delta\":\"image check complete\"}\n\n",
        "event: response.completed\ndata: {\"response\":{\"id\":\"fixture\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":13000,\"output_tokens\":10}}}\n\n"
    );
    let server = MockServer::start_async().await;
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path(PATH);
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(REPLY);
        })
        .await;
    let mut config = media_config(&server, "/image-compaction");
    let model_settings = config.models.get_mut(&config.default_model).unwrap();
    model_settings.think = kuku::config::ThinkLevel::Off;
    model_settings.context_window = 24_000;
    model_settings.max_output_tokens = None;
    config.handoff.enabled = true;
    config.handoff.threshold = 0.45;
    let mut options = SessionOptions::new(work.path(), data.path(), config);
    options.limits.max_request_bytes = Some(99_000);
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new(
            "prior",
            "main",
            "A preceding fact. ".repeat(1_500),
        ))
        .unwrap();
    assert_eq!(
        session.wait_for_agent("main").await.unwrap().last_error,
        None
    );
    let mut input = Input::new("image", "main", "[Image #1]");
    input.images.push(
        session
            .save_input_image("main", &png_with_comment(45_000, b'V'))
            .await
            .unwrap(),
    );
    session.send(input).unwrap();
    let snapshot = session.wait_for_agent("main").await.unwrap();
    assert_eq!(snapshot.last_error, None);
    assert!(
        session
            .events(0, 512)
            .unwrap()
            .iter()
            .any(|event| matches!(&event.payload,
        EventPayload::Unknown(value) if value["kind"] == "runtime.compaction_started"
            && value["trigger"] == "request_byte_budget"))
    );
    mock.assert_hits_async(3).await;
    session.close().await.unwrap();
}
