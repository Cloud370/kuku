use std::collections::BTreeMap;

use httpmock::prelude::{MockServer, POST};
use serde_json::{json, Value};
use tokio_stream::StreamExt;

use super::config::{ProviderFormat, ProviderProfile, ThinkLevel};
use super::context::{CanonicalMessage, MessageBlock, ToolResult, ToolSchema, ToolUse};
use super::provider::chunk::ProviderChunk;
use super::provider::types::{
    PreparedProviderRequest, ProviderCorrelation, ProviderKind, ProviderRequest, ProviderSettings,
    ResolvedProvider, SecretString,
};
use super::{current_input, sample_assembly, test_catalog};

#[tokio::test]
async fn three_protocols_preserve_tool_batches_native_replay_and_multiturn_prefixes() {
    for (protocol, kind, profile, model) in [
        (
            ProviderFormat::OpenAiChat,
            ProviderKind::OpenAiCompatible,
            ProviderProfile::OpenRouter,
            "deepseek/deepseek-v4.1-flash-1017",
        ),
        (
            ProviderFormat::OpenAiResponses,
            ProviderKind::OpenAiResponses,
            ProviderProfile::OpenAi,
            "gpt-5.4",
        ),
        (
            ProviderFormat::Anthropic,
            ProviderKind::Anthropic,
            ProviderProfile::Anthropic,
            "claude-sonnet-4-6",
        ),
    ] {
        let server = MockServer::start();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("left.txt"), "3").unwrap();
        std::fs::write(workspace.path().join("right.txt"), "4").unwrap();
        let mut config = ResolvedProvider {
            connection_name: "fixture".into(),
            kind,
            model: model.into(),
            base_url: server.base_url(),
            api_key: SecretString::new("offline-key"),
            settings: ProviderSettings {
                requested_reasoning_effort: Some("low".into()),
                reasoning_effort: Some("low".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        config.capabilities.profile = profile;
        let catalog = test_catalog();
        let mut request = ProviderRequest {
            assembly: sample_assembly(),
            catalog: &catalog,
            current_input: current_input(),
            model: model.into(),
            max_output_tokens: Some(256),
            temperature: None,
            stream: true,
            think_level: ThinkLevel::Low,
            thinking: Default::default(),
        };
        request.assembly.history.push(CanonicalMessage::user_text(
            "Read left.txt and right.txt, then report their sum.",
        ));
        request.assembly.tools = vec![ToolSchema {
            name: "read_file".into(),
            description: "Read one file".into(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        }];
        let correlation = ProviderCorrelation {
            session_id: "fixed-session".into(),
            agent_address: "main".into(),
            connection_name: "fixture".into(),
        };
        let first =
            super::provider::prepare_provider_request(&config, &request, Some(&correlation))
                .unwrap();
        let first_mock = mock_request(&server, &first, &fixture(protocol, 1, model));
        let (assistant, calls, reports) = collect_response(&config, &first).await;
        assert_eq!(2, calls.len());
        assert_eq!(Some(0.001), reports.last().unwrap().cost_usd);
        assert_eq!(
            Some(30),
            reports
                .last()
                .unwrap()
                .raw_usage
                .as_ref()
                .unwrap()
                .get(if protocol == ProviderFormat::OpenAiChat {
                    "prompt_tokens"
                } else {
                    "input_tokens"
                })
                .and_then(Value::as_u64)
                .map(|value| if protocol == ProviderFormat::Anthropic {
                    value + 20
                } else {
                    value
                })
        );
        request.assembly.history.push(assistant);
        let results = calls
            .iter()
            .map(|call| {
                let text = std::fs::read_to_string(
                    workspace.path().join(call.args["path"].as_str().unwrap()),
                )
                .unwrap();
                MessageBlock::ToolResult(ToolResult {
                    tool_call_id: call.id.clone(),
                    status: "ok".into(),
                    summary: "read".into(),
                    model_content: text,
                    structured: None,
                    truncated: false,
                })
            })
            .collect();
        request
            .assembly
            .history
            .push(CanonicalMessage::user(results));
        let second =
            super::provider::prepare_provider_request(&config, &request, Some(&correlation))
                .unwrap();
        assert_eq!(first.correlation_id, second.correlation_id);
        assert!(wire_messages(&second.body).starts_with(wire_messages(&first.body)));
        assert_tool_continuation(protocol, &second.body);
        let second_mock = mock_request(&server, &second, &fixture(protocol, 2, model));
        let (assistant, calls, _) = collect_response(&config, &second).await;
        assert!(calls.is_empty());
        assert!(assistant
            .blocks
            .iter()
            .any(|block| matches!(block, MessageBlock::Text(text) if text == "sum=7")));
        request.assembly.history.push(assistant);
        request
            .assembly
            .history
            .push(CanonicalMessage::user_text("Confirm the same result."));
        let third =
            super::provider::prepare_provider_request(&config, &request, Some(&correlation))
                .unwrap();
        assert_eq!(first.correlation_id, third.correlation_id);
        assert!(wire_messages(&third.body).starts_with(wire_messages(&second.body)));
        let third_mock = mock_request(&server, &third, &fixture(protocol, 3, model));
        let (assistant, calls, _) = collect_response(&config, &third).await;
        assert!(calls.is_empty());
        assert!(assistant
            .blocks
            .iter()
            .any(|block| matches!(block, MessageBlock::Text(text) if text == "confirmed")));
        first_mock.assert_hits(1);
        second_mock.assert_hits(1);
        third_mock.assert_hits(1);
    }
}

fn mock_request<'a>(
    server: &'a MockServer,
    request: &PreparedProviderRequest,
    response: &str,
) -> httpmock::Mock<'a> {
    server.mock(|when, then| {
        when.method(POST)
            .path(url::Url::parse(&request.url).unwrap().path())
            .json_body(request.body.clone());
        then.status(200)
            .header("content-type", "text/event-stream")
            .body(response);
    })
}

async fn collect_response(
    config: &ResolvedProvider,
    request: &PreparedProviderRequest,
) -> (
    CanonicalMessage,
    Vec<ToolUse>,
    Vec<super::provider::types::ProviderReport>,
) {
    let mut stream = super::provider::stream_prepared_provider(config, request, None)
        .await
        .unwrap();
    let mut text = String::new();
    let mut thinking = String::new();
    let mut calls: BTreeMap<u64, (String, String, String, bool)> = BTreeMap::new();
    let mut native = Vec::new();
    let mut reports = Vec::new();
    let mut ended = false;
    while let Some(chunk) = stream.next().await {
        match chunk.unwrap() {
            ProviderChunk::TextDelta { text: fragment } => text.push_str(&fragment),
            ProviderChunk::ThinkingDelta { text: fragment } => thinking.push_str(&fragment),
            ProviderChunk::ToolCallStart { index, id, name } => {
                assert!(calls
                    .insert(index, (id, name, String::new(), false))
                    .is_none());
            }
            ProviderChunk::ToolCallArgDelta { index, fragment } => {
                calls.get_mut(&index).unwrap().2.push_str(&fragment)
            }
            ProviderChunk::ToolCallStop { index } => calls.get_mut(&index).unwrap().3 = true,
            ProviderChunk::Replay { protocol, value } => {
                native.push(MessageBlock::ProviderReplay { protocol, value })
            }
            ProviderChunk::Report { report } => reports.push(report),
            ProviderChunk::StreamEnd => ended = true,
            ProviderChunk::InvalidToolStream => panic!("fixture produced an invalid tool stream"),
            ProviderChunk::ServerError { code, message } => panic!("{code}: {message}"),
            ProviderChunk::StreamStart { .. }
            | ProviderChunk::ContentBlockStop { .. }
            | ProviderChunk::StreamUsage { .. }
            | ProviderChunk::StopReason { .. } => {}
        }
    }
    assert!(ended);
    let calls: Vec<_> = calls
        .into_values()
        .map(|(id, name, arguments, completed)| {
            assert!(completed);
            ToolUse {
                id,
                name,
                args: serde_json::from_str(&arguments).unwrap(),
            }
        })
        .collect();
    let mut blocks = Vec::new();
    if !thinking.is_empty() {
        blocks.push(MessageBlock::Thinking(thinking));
    }
    if !text.is_empty() {
        blocks.push(MessageBlock::Text(text));
    }
    blocks.extend(calls.iter().cloned().map(MessageBlock::ToolUse));
    blocks.extend(native);
    (CanonicalMessage::assistant(blocks), calls, reports)
}

fn wire_messages(body: &Value) -> &[Value] {
    body.get("messages")
        .or_else(|| body.get("input"))
        .unwrap()
        .as_array()
        .unwrap()
}

fn assert_tool_continuation(protocol: ProviderFormat, body: &Value) {
    let messages = wire_messages(body);
    match protocol {
        ProviderFormat::OpenAiChat => {
            let assistant = messages
                .iter()
                .find(|value| value.get("tool_calls").is_some())
                .unwrap();
            assert_eq!("cipher-fixture", assistant["reasoning_details"][0]["data"]);
            assert!(assistant.get("reasoning_content").is_none());
            assert_eq!(2, assistant["tool_calls"].as_array().unwrap().len());
            for (id, content) in [("c1", "3"), ("c2", "4")] {
                assert!(messages.iter().any(|value| value["role"] == "tool"
                    && value["tool_call_id"] == id
                    && value["content"] == content));
            }
        }
        ProviderFormat::OpenAiResponses => {
            assert!(messages.iter().any(|value| value["type"] == "reasoning"
                && value["encrypted_content"] == "cipher-fixture"));
            for (id, content) in [("c1", "3"), ("c2", "4")] {
                assert!(messages.iter().any(|value| value["type"] == "function_call"
                    && value["call_id"] == id
                    && value["arguments"].is_string()));
                assert!(messages
                    .iter()
                    .any(|value| value["type"] == "function_call_output"
                        && value["call_id"] == id
                        && value["output"] == content));
            }
        }
        ProviderFormat::Anthropic => {
            let assistant = messages
                .iter()
                .find(|value| {
                    value["role"] == "assistant"
                        && value["content"].as_array().is_some_and(|blocks| {
                            blocks.iter().any(|block| block["type"] == "tool_use")
                        })
                })
                .unwrap();
            assert_eq!("signed-fixture", assistant["content"][0]["signature"]);
            let results = messages.last().unwrap()["content"].as_array().unwrap();
            for (id, content) in [("c1", "3"), ("c2", "4")] {
                assert!(results.iter().any(|value| value["type"] == "tool_result"
                    && value["tool_use_id"] == id
                    && value["content"] == content
                    && value["is_error"] == false));
            }
        }
    }
}

fn event(kind: &str, value: Value) -> String {
    format!("event: {kind}\r\ndata: {value}\r\n\r\n")
}

fn fixture(protocol: ProviderFormat, turn: u8, model: &str) -> String {
    match protocol {
        ProviderFormat::OpenAiChat => chat_fixture(turn, model),
        ProviderFormat::OpenAiResponses => responses_fixture(turn, model),
        ProviderFormat::Anthropic => messages_fixture(turn, model),
    }
}

fn chat_fixture(turn: u8, model: &str) -> String {
    let chunk = |delta: Value, finish: Value| {
        format!(
            ": gateway keepalive\r\ndata: {}\r\n\r\n",
            json!({"id":format!("chat-{turn}"),"model":model,"provider":"FixtureUpstream","choices":[{"index":0,"delta":delta,"finish_reason":finish}],"usage":null})
        )
    };
    let mut output = if turn == 1 {
        chunk(
            json!({"reasoning":"plan","reasoning_details":[{"index":0,"type":"reasoning.encrypted","data":"cipher-"}],"tool_calls":[{"index":0,"id":"c1","function":{"name":"read_file","arguments":"{\"path\":"}},{"index":1,"id":"c2","function":{"name":"read_file","arguments":"{\"path\":"}}]}),
            Value::Null,
        ) + &chunk(
            json!({"reasoning_details":[{"index":0,"type":"reasoning.encrypted","data":"fixture"}],"tool_calls":[{"index":1,"function":{"arguments":"\"right.txt\"}"}},{"index":0,"function":{"arguments":"\"left.txt\"}"}}]}),
            Value::Null,
        ) + &chunk(json!({}), json!("tool_calls"))
    } else {
        chunk(
            json!({"content":if turn == 2 {"sum=7"} else {"confirmed"}}),
            json!("stop"),
        )
    };
    output += &format!(
        "data: {}\r\n\r\ndata: [DONE]\r\n\r\n",
        json!({"id":format!("chat-{turn}"),"choices":[],"usage":{"prompt_tokens":30,"completion_tokens":12,"prompt_tokens_details":{"cached_tokens":20},"cost":0.001}})
    );
    output
}

fn messages_fixture(turn: u8, model: &str) -> String {
    let mut output = event(
        "message_start",
        json!({"message":{"id":format!("msg-{turn}"),"model":model,"usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":0,"output_tokens":0}}}),
    );
    if turn == 1 {
        output += &event(
            "content_block_start",
            json!({"index":0,"content_block":{"type":"thinking","thinking":""}}),
        );
        output += &event(
            "content_block_delta",
            json!({"index":0,"delta":{"type":"thinking_delta","thinking":"plan"}}),
        );
        output += &event(
            "content_block_delta",
            json!({"index":0,"delta":{"type":"signature_delta","signature":"signed-fixture"}}),
        );
        output += &event("content_block_stop", json!({"index":0}));
        for (index, id, path) in [(1, "c1", "left.txt"), (2, "c2", "right.txt")] {
            output += &event(
                "content_block_start",
                json!({"index":index,"content_block":{"type":"tool_use","id":id,"name":"read_file","input":{}}}),
            );
            output += &event(
                "content_block_delta",
                json!({"index":index,"delta":{"type":"input_json_delta","partial_json":json!({"path":path}).to_string()}}),
            );
            output += &event("content_block_stop", json!({"index":index}));
        }
    } else {
        output += &event(
            "content_block_start",
            json!({"index":0,"content_block":{"type":"text","text":if turn == 2 {"sum=7"} else {"confirmed"}}}),
        );
        output += &event("content_block_stop", json!({"index":0}));
    }
    output += &event(
        "message_delta",
        json!({"delta":{"stop_reason":if turn == 1 {"tool_use"} else {"end_turn"}},"usage":{"output_tokens":12,"cost":0.001}}),
    );
    output += &event("message_stop", json!({}));
    output
}

fn responses_fixture(turn: u8, model: &str) -> String {
    let mut output = event(
        "response.created",
        json!({"response":{"id":format!("resp-{turn}"),"model":model,"status":"in_progress","usage":null}}),
    );
    let mut items = Vec::new();
    if turn == 1 {
        let reasoning = json!({"id":"rs1","type":"reasoning","summary":[],"encrypted_content":"cipher-fixture"});
        output += &event(
            "response.output_item.added",
            json!({"output_index":0,"item":reasoning}),
        );
        output += &event(
            "response.output_item.done",
            json!({"output_index":0,"item":reasoning}),
        );
        items.push(reasoning);
        for (index, id, path) in [(1, "c1", "left.txt"), (2, "c2", "right.txt")] {
            let mut item = json!({"id":format!("fc-{id}"),"type":"function_call","call_id":id,"name":"read_file","arguments":""});
            output += &event(
                "response.output_item.added",
                json!({"output_index":index,"item":item}),
            );
            let arguments = json!({"path":path}).to_string();
            output += &event(
                "response.function_call_arguments.delta",
                json!({"output_index":index,"item_id":format!("fc-{id}"),"delta":arguments}),
            );
            output += &event(
                "response.function_call_arguments.done",
                json!({"output_index":index,"item_id":format!("fc-{id}"),"arguments":arguments}),
            );
            item["arguments"] = json!(arguments);
            output += &event(
                "response.output_item.done",
                json!({"output_index":index,"item":item}),
            );
            items.push(item);
        }
    } else {
        let text = if turn == 2 { "sum=7" } else { "confirmed" };
        let item = json!({"id":format!("text-{turn}"),"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]});
        output += &event(
            "response.output_item.added",
            json!({"output_index":0,"item":item}),
        );
        output += &event(
            "response.output_text.delta",
            json!({"output_index":0,"delta":text}),
        );
        output += &event(
            "response.output_item.done",
            json!({"output_index":0,"item":item}),
        );
        items.push(item);
    }
    output += &event(
        "response.completed",
        json!({"response":{"id":format!("resp-{turn}"),"model":model,"status":"completed","output":items,"usage":{"input_tokens":30,"output_tokens":12,"input_tokens_details":{"cached_tokens":20},"cost":0.001}}}),
    );
    output
}
