mod context {
    pub use kuku::context::{
        CanonicalMessage, ContextAssembly, FileSource, InstructionSource, MemorySource,
        MessageBlock, Role, ToolResult, ToolSchema, ToolUse,
    };
}

mod provider {
    #[allow(dead_code)]
    pub mod types {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/provider/types.rs"
        ));
    }

    #[allow(dead_code)]
    pub mod error {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/provider/error.rs"
        ));
    }

    #[allow(dead_code)]
    pub mod anthropic {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/provider/anthropic.rs"
        ));
    }

    #[allow(dead_code)]
    pub mod openai_compat {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/provider/openai_compat.rs"
        ));
    }
}

use context::{
    CanonicalMessage, ContextAssembly, FileSource, InstructionSource, MemorySource, MessageBlock,
    Role, ToolResult, ToolSchema, ToolUse,
};
use httpmock::prelude::*;
use provider::anthropic::{
    call as call_anthropic, messages_url, render_body as render_anthropic_body,
};
use provider::openai_compat::{
    call as call_openai_compat, chat_completions_url, render_body as render_openai_body,
};
use provider::types::{
    ProviderFailureKind, ProviderKind, ProviderRequest, ResolvedProvider, SecretString,
};
use serde_json::json;

fn sample_assembly() -> ContextAssembly {
    ContextAssembly {
        system_prompt:
            "You are the agent running inside kuku, a file-native software engineering runtime."
                .to_string(),
        prelude_messages: vec![
            CanonicalMessage::user_text(
                "<kuku_execution_context>\n- Workspace root: /workspace\n- Platform: linux\n- Current date: 2026-05-14\n</kuku_execution_context>\n\n<kuku_project_instructions>\nfollow project instructions\n</kuku_project_instructions>\n\n<kuku_memory>\n<kuku_global_memory>\n- remember the user prefers concise answers\n</kuku_global_memory>\n<kuku_project_memory>\n- No project memory.\n</kuku_project_memory>\n</kuku_memory>"
            ),
            CanonicalMessage::user_text(
                "<kuku_tool_guidance>\nUse tools to establish evidence before concluding or modifying.\n\nGuidance:\n- Do not guess when tools can establish the answer.\n- Prefer collecting enough evidence in fewer rounds instead of many tiny rounds.\n- When multiple read-only tool calls are independent and the targets are already known, prefer batching them in the same round.\n- When one step depends on the result of another, keep the calls sequential.\n- Understand the relevant context before modifying files.\n- Prefer focused edits over broader rewrites when both would work.\n- Reserve `run_command` for validation, project commands, scripts, generators, and other cases where a command is the right tool.\n- Treat tool results as evidence.\n- Do not claim conclusions that are not supported by tool or file evidence.\n</kuku_tool_guidance>"
            ),
        ],
        history: vec![
            CanonicalMessage {
                role: Role::User,
                blocks: vec![MessageBlock::Text("hello".to_string())],
            },
            CanonicalMessage {
                role: Role::Assistant,
                blocks: vec![MessageBlock::Text("hi there".to_string())],
            },
        ],
        tools: Vec::new(),
        prompt_asset_sources: vec![
            FileSource {
                path: "crates/kuku/prompts/system.md".to_string(),
                hash: "sha256:system".to_string(),
            },
            FileSource {
                path: "crates/kuku/prompts/synthetic-user.md".to_string(),
                hash: "sha256:synthetic".to_string(),
            },
            FileSource {
                path: "crates/kuku/prompts/tool-guidance.md".to_string(),
                hash: "sha256:tool-guidance".to_string(),
            },
        ],
        project_instruction_sources: vec![InstructionSource {
            path: "/workspace/AGENTS.md".to_string(),
            kind: "agents".to_string(),
            hash: "sha256:agents".to_string(),
            content: "follow project instructions".to_string(),
        }],
        memory_sources: vec![MemorySource {
            path: "/home/user/.kuku/memory.md".to_string(),
            hash: "sha256:global".to_string(),
            content: "remember the user prefers concise answers".to_string(),
        }],
    }
}

fn sample_tool_schema() -> ToolSchema {
    ToolSchema {
        name: "find_files".to_string(),
        description: "Find files".to_string(),
        input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
    }
}

fn assembly_with_tool_schema() -> ContextAssembly {
    let mut assembly = sample_assembly();
    assembly.tools.push(sample_tool_schema());
    assembly
}

fn assembly_with_tool_history() -> ContextAssembly {
    ContextAssembly {
        system_prompt:
            "You are the agent running inside kuku, a file-native software engineering runtime."
                .to_string(),
        prelude_messages: vec![
            CanonicalMessage::user_text(
                "## Environment\n- Workspace: /workspace\n- Platform: linux\n\n## Project Instructions\nNo project instructions found.\n\n## Memory\n- Global memory: No global memory.\n- Project memory: No project memory."
            ),
            CanonicalMessage::user_text(
                "Use specialized tools when they match the task.\n\nGuidelines:\n\n- Use file discovery tools to find files instead of guessing paths."
            ),
        ],
        history: vec![
            CanonicalMessage {
                role: Role::Assistant,
                blocks: vec![
                    MessageBlock::Text("Let me inspect.".to_string()),
                    MessageBlock::ToolUse(ToolUse {
                        id: "toolu_01".to_string(),
                        name: "find_files".to_string(),
                        args: json!({"path": "."}),
                    }),
                ],
            },
            CanonicalMessage {
                role: Role::User,
                blocks: vec![MessageBlock::ToolResult(ToolResult {
                    tool_call_id: "toolu_01".to_string(),
                    status: "ok".to_string(),
                    summary: "found 1 files".to_string(),
                    model_content: "README.md".to_string(),
                    structured: None,
                    truncated: false,
                })],
            },
        ],
        tools: Vec::new(),
        prompt_asset_sources: vec![
            FileSource {
                path: "crates/kuku/prompts/system.md".to_string(),
                hash: "sha256:system".to_string(),
            },
            FileSource {
                path: "crates/kuku/prompts/synthetic-user.md".to_string(),
                hash: "sha256:synthetic".to_string(),
            },
            FileSource {
                path: "crates/kuku/prompts/tool-guidance.md".to_string(),
                hash: "sha256:tool-guidance".to_string(),
            },
        ],
        project_instruction_sources: Vec::new(),
        memory_sources: Vec::new(),
    }
}

#[test]
fn anthropic_messages_url_normalizes_v1_suffix() {
    assert_eq!(
        messages_url("https://api.anthropic.com"),
        "https://api.anthropic.com/v1/messages"
    );
    assert_eq!(
        messages_url("https://gateway.example/v1/"),
        "https://gateway.example/v1/messages"
    );
}

#[test]
fn anthropic_render_body_preserves_layer_order() {
    let body = render_anthropic_body(&ProviderRequest {
        assembly: sample_assembly(),
        model: "claude-sonnet-4-6".to_string(),
        max_output_tokens: Some(1024),
        temperature: Some(0.2),
    });

    assert_eq!(body["model"], "claude-sonnet-4-6");
    assert_eq!(body["stream"], false);
    assert_eq!(body["max_tokens"], 1024);
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][1]["role"], "user");
    assert!(body.get("stop").is_none());
    assert!(body["system"]
        .as_str()
        .unwrap()
        .contains("You are the agent running inside kuku"));
    assert!(body["messages"][0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("<kuku_execution_context>"));
    assert!(body["messages"][1]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Use tools to establish evidence before concluding or modifying."));
}

#[test]
fn anthropic_render_body_includes_tools_and_native_tool_results() {
    let tool_body = render_anthropic_body(&ProviderRequest {
        assembly: assembly_with_tool_schema(),
        model: "claude-sonnet-4-6".to_string(),
        max_output_tokens: None,
        temperature: None,
    });

    assert_eq!(tool_body["tools"][0]["name"], "find_files");
    assert_eq!(tool_body["tools"][0]["input_schema"]["type"], "object");

    let history_body = render_anthropic_body(&ProviderRequest {
        assembly: assembly_with_tool_history(),
        model: "claude-sonnet-4-6".to_string(),
        max_output_tokens: None,
        temperature: None,
    });

    assert_eq!(
        history_body["messages"][2]["content"][1]["type"],
        "tool_use"
    );
    assert_eq!(
        history_body["messages"][3]["content"][0]["type"],
        "tool_result"
    );
    assert_eq!(
        history_body["messages"][3]["content"][0]["tool_use_id"],
        "toolu_01"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_call_extracts_tool_use_blocks() {
    let server = MockServer::start();
    let request = ProviderRequest {
        assembly: assembly_with_tool_schema(),
        model: "claude-sonnet-4-6".to_string(),
        max_output_tokens: None,
        temperature: None,
    };
    let provider = ResolvedProvider {
        kind: ProviderKind::Anthropic,
        model: request.model.clone(),
        base_url: server.base_url(),
        api_key: SecretString::new("anthropic-test-key"),
    };

    let mock = server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "msg_req_tool")
            .json_body(json!({
                "type": "message",
                "content": [
                    {"type": "text", "text": "Let me inspect."},
                    {"type": "tool_use", "id": "toolu_01", "name": "find_files", "input": {"path": "docs"}},
                    {"type": "tool_use", "id": "toolu_02", "name": "read_file", "input": {"path": "README.md"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 20, "output_tokens": 30}
            }));
    });

    let response = call_anthropic(&provider, &request).await.unwrap();

    mock.assert();
    assert_eq!(response.stop_reason.as_deref(), Some("tool_use"));
    assert_eq!(response.tool_calls.len(), 2);
    assert_eq!(response.tool_calls[0].id, "toolu_01");
    assert_eq!(response.tool_calls[0].name, "find_files");
    assert_eq!(response.tool_calls[0].args, json!({"path": "docs"}));
    assert_eq!(response.tool_calls[0].index, 0);
    assert_eq!(response.tool_calls[1].index, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_call_sends_expected_headers_and_parses_success() {
    let server = MockServer::start();
    let request = ProviderRequest {
        assembly: sample_assembly(),
        model: "claude-sonnet-4-6".to_string(),
        max_output_tokens: Some(1024),
        temperature: None,
    };
    let provider = ResolvedProvider {
        kind: ProviderKind::Anthropic,
        model: request.model.clone(),
        base_url: server.base_url(),
        api_key: SecretString::new("anthropic-test-key"),
    };

    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .header("x-api-key", "anthropic-test-key")
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json_body_partial(
                json!({
                    "model": "claude-sonnet-4-6",
                    "stream": false,
                    "max_tokens": 1024,
                })
                .to_string(),
            );
        then.status(200)
            .header("request-id", "msg_req_123")
            .json_body(json!({
                "type": "message",
                "content": [{"type": "text", "text": "Hello from Anthropic"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 11, "output_tokens": 7}
            }));
    });

    let response = call_anthropic(&provider, &request).await.unwrap();

    mock.assert();
    assert_eq!(response.assistant_text, "Hello from Anthropic");
    assert_eq!(response.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(response.provider_request_id.as_deref(), Some("msg_req_123"));
    assert_eq!(response.usage.unwrap().input_tokens, Some(11));
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_http_failure_is_normalized() {
    let server = MockServer::start();
    let request = ProviderRequest {
        assembly: sample_assembly(),
        model: "claude-sonnet-4-6".to_string(),
        max_output_tokens: None,
        temperature: None,
    };
    let provider = ResolvedProvider {
        kind: ProviderKind::Anthropic,
        model: request.model.clone(),
        base_url: server.base_url(),
        api_key: SecretString::new("bad-key"),
    };

    let mock = server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(401)
            .header("request-id", "msg_req_auth")
            .body("unauthorized");
    });

    let failure = call_anthropic(&provider, &request).await.unwrap_err();

    mock.assert();
    assert_eq!(failure.kind, ProviderFailureKind::Authentication);
    assert_eq!(failure.status, Some(401));
    assert_eq!(failure.provider_request_id.as_deref(), Some("msg_req_auth"));
    assert!(!failure.retryable);
}

#[test]
fn openai_chat_completions_url_appends_path() {
    assert_eq!(
        chat_completions_url("https://api.openai.com/v1"),
        "https://api.openai.com/v1/chat/completions"
    );
    assert_eq!(
        chat_completions_url("https://gateway.example/v1/"),
        "https://gateway.example/v1/chat/completions"
    );
}

#[test]
fn openai_render_body_preserves_layer_order() {
    let body = render_openai_body(&ProviderRequest {
        assembly: sample_assembly(),
        model: "gpt-5.4-mini".to_string(),
        max_output_tokens: Some(2048),
        temperature: Some(0.7),
    });

    assert_eq!(body["model"], "gpt-5.4-mini");
    assert_eq!(body["stream"], false);
    assert_eq!(body["max_tokens"], 2048);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(body["messages"][2]["role"], "user");
    assert!(body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("You are the agent running inside kuku"));
    assert!(body.get("max_completion_tokens").is_none());
}

#[test]
fn openai_render_body_includes_tools_and_role_tool_messages() {
    let tool_body = render_openai_body(&ProviderRequest {
        assembly: assembly_with_tool_schema(),
        model: "gpt-5.4-mini".to_string(),
        max_output_tokens: None,
        temperature: None,
    });

    assert_eq!(tool_body["tools"][0]["type"], "function");
    assert_eq!(tool_body["tools"][0]["function"]["name"], "find_files");
    assert_eq!(
        tool_body["tools"][0]["function"]["parameters"]["type"],
        "object"
    );

    let history_body = render_openai_body(&ProviderRequest {
        assembly: assembly_with_tool_history(),
        model: "gpt-5.4-mini".to_string(),
        max_output_tokens: None,
        temperature: None,
    });

    assert_eq!(
        history_body["messages"][3]["tool_calls"][0]["id"],
        "toolu_01"
    );
    assert_eq!(history_body["messages"][4]["role"], "tool");
    assert_eq!(history_body["messages"][4]["tool_call_id"], "toolu_01");
    assert_eq!(history_body["messages"][4]["content"], "README.md");
}

#[tokio::test(flavor = "current_thread")]
async fn openai_call_extracts_tool_calls() {
    let server = MockServer::start();
    let request = ProviderRequest {
        assembly: assembly_with_tool_schema(),
        model: "gpt-5.4-mini".to_string(),
        max_output_tokens: None,
        temperature: None,
    };
    let provider = ResolvedProvider {
        kind: ProviderKind::OpenAiCompatible,
        model: request.model.clone(),
        base_url: format!("{}/v1", server.base_url()),
        api_key: SecretString::new("openai-test-key"),
    };

    let mock = server.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(200).header("x-request-id", "chat_req_456").json_body(json!({
            "choices": [{
                "message": {
                    "content": "Let me inspect.",
                    "tool_calls": [
                        {"id": "call_1", "type": "function", "function": {"name": "find_files", "arguments": "{\"path\":\"docs\"}"}},
                        {"type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"README.md\"}"}}
                    ]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 10}
        }));
    });

    let response = call_openai_compat(&provider, &request).await.unwrap();

    mock.assert();
    assert_eq!(response.stop_reason.as_deref(), Some("tool_use"));
    assert_eq!(response.tool_calls.len(), 2);
    assert_eq!(response.tool_calls[0].id, "call_1");
    assert_eq!(response.tool_calls[0].name, "find_files");
    assert_eq!(response.tool_calls[0].args, json!({"path": "docs"}));
    assert_eq!(response.tool_calls[0].index, 0);
    assert_eq!(response.tool_calls[1].id, "tc_chat_req_456_1");
    assert_eq!(response.tool_calls[1].index, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn openai_call_sends_bearer_auth_and_parses_success() {
    let server = MockServer::start();
    let request = ProviderRequest {
        assembly: sample_assembly(),
        model: "gpt-5.4-mini".to_string(),
        max_output_tokens: Some(512),
        temperature: Some(0.4),
    };
    let provider = ResolvedProvider {
        kind: ProviderKind::OpenAiCompatible,
        model: request.model.clone(),
        base_url: format!("{}/v1", server.base_url()),
        api_key: SecretString::new("openai-test-key"),
    };

    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/chat/completions")
            .header("authorization", "Bearer openai-test-key")
            .header("content-type", "application/json")
            .json_body_partial(
                json!({
                    "model": "gpt-5.4-mini",
                    "stream": false,
                    "max_tokens": 512,
                })
                .to_string(),
            );
        then.status(200)
            .header("x-request-id", "chat_req_456")
            .json_body(json!({
                "choices": [{
                    "message": {"content": "Hello from OpenAI"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 13, "completion_tokens": 8}
            }));
    });

    let response = call_openai_compat(&provider, &request).await.unwrap();

    mock.assert();
    assert_eq!(response.assistant_text, "Hello from OpenAI");
    assert_eq!(response.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(
        response.provider_request_id.as_deref(),
        Some("chat_req_456")
    );
    assert_eq!(response.usage.unwrap().output_tokens, Some(8));
}

#[tokio::test(flavor = "current_thread")]
async fn openai_http_failure_is_normalized() {
    let server = MockServer::start();
    let request = ProviderRequest {
        assembly: sample_assembly(),
        model: "gpt-5.4-mini".to_string(),
        max_output_tokens: None,
        temperature: None,
    };
    let provider = ResolvedProvider {
        kind: ProviderKind::OpenAiCompatible,
        model: request.model.clone(),
        base_url: format!("{}/v1", server.base_url()),
        api_key: SecretString::new("bad-key"),
    };

    let mock = server.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(429)
            .header("x-request-id", "chat_req_rate")
            .body("rate limited");
    });

    let failure = call_openai_compat(&provider, &request).await.unwrap_err();

    mock.assert();
    assert_eq!(failure.kind, ProviderFailureKind::RateLimited);
    assert_eq!(failure.status, Some(429));
    assert_eq!(
        failure.provider_request_id.as_deref(),
        Some("chat_req_rate")
    );
    assert!(failure.retryable);
}
