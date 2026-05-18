mod config {
    pub use kuku::config::{ResolvedThinking, ThinkLevel};
}

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
    pub mod chunk {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/provider/chunk.rs"
        ));
    }

    #[allow(dead_code)]
    pub mod error {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/provider/error.rs"
        ));
    }

    use futures_core::Stream;
    use std::pin::Pin;
    #[allow(dead_code)]
    pub type ProviderChunkStream = Pin<
        Box<
            dyn Stream<Item = std::result::Result<chunk::ProviderChunk, types::ProviderFailure>>
                + Send,
        >,
    >;

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

    #[allow(dead_code)]
    pub mod openai_responses {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/provider/openai_responses.rs"
        ));
    }
}

use config::ResolvedThinking;
use context::{
    CanonicalMessage, ContextAssembly, FileSource, InstructionSource, MemorySource, MessageBlock,
    Role, ToolResult, ToolSchema, ToolUse,
};
use provider::anthropic::{messages_url, render_body as render_anthropic_body};
use provider::chunk::ProviderChunk;
use provider::openai_compat::{chat_completions_url, render_body as render_openai_body};
use provider::openai_responses::{
    parse_responses_sse, render_body as render_responses_body, responses_url,
};
use provider::types::ProviderRequest;
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
                path: "crates/kuku/prompts/project-context.md".to_string(),
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
        runtime_context: None,
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

fn assembly_with_drift_notice() -> ContextAssembly {
    let mut assembly = sample_assembly();
    assembly.prelude_messages.insert(
        1,
        CanonicalMessage::user_text(
            "<kuku_system_notice>\n- Context drift: /workspace/AGENTS.md changed (sha256:old -> sha256:new)\n</kuku_system_notice>",
        ),
    );
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
                path: "crates/kuku/prompts/project-context.md".to_string(),
                hash: "sha256:synthetic".to_string(),
            },
            FileSource {
                path: "crates/kuku/prompts/tool-guidance.md".to_string(),
                hash: "sha256:tool-guidance".to_string(),
            },
        ],
        project_instruction_sources: Vec::new(),
        memory_sources: Vec::new(),
        runtime_context: None,
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
fn anthropic_render_body_keeps_drift_notice_between_context_and_tool_guidance() {
    let body = render_anthropic_body(&ProviderRequest {
        stream: false,
        assembly: assembly_with_drift_notice(),
        model: "claude-sonnet-4-6".to_string(),
        max_output_tokens: Some(1024),
        temperature: Some(0.2),
        think_level: "off".to_string(),
        thinking: ResolvedThinking::default(),
    });

    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(body["messages"][2]["role"], "user");
    assert!(body["messages"][0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("<kuku_execution_context>"));
    assert!(body["messages"][1]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("<kuku_system_notice>"));
    assert!(body["messages"][2]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("<kuku_tool_guidance>"));
}

#[test]
fn anthropic_render_body_preserves_layer_order() {
    let body = render_anthropic_body(&ProviderRequest {
        stream: false,
        assembly: sample_assembly(),
        model: "claude-sonnet-4-6".to_string(),
        max_output_tokens: Some(1024),
        temperature: Some(0.2),
        think_level: "off".to_string(),
        thinking: ResolvedThinking::default(),
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
        stream: false,
        assembly: assembly_with_tool_schema(),
        model: "claude-sonnet-4-6".to_string(),
        max_output_tokens: None,
        temperature: None,
        think_level: "off".to_string(),
        thinking: ResolvedThinking::default(),
    });

    assert_eq!(tool_body["tools"][0]["name"], "find_files");
    assert_eq!(tool_body["tools"][0]["input_schema"]["type"], "object");

    let history_body = render_anthropic_body(&ProviderRequest {
        stream: false,
        assembly: assembly_with_tool_history(),
        model: "claude-sonnet-4-6".to_string(),
        max_output_tokens: None,
        temperature: None,
        think_level: "off".to_string(),
        thinking: ResolvedThinking::default(),
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
fn responses_url_appends_path() {
    assert_eq!(
        responses_url("https://api.openai.com/v1"),
        "https://api.openai.com/v1/responses"
    );
    assert_eq!(
        responses_url("https://gateway.example/v1/"),
        "https://gateway.example/v1/responses"
    );
}

#[test]
fn openai_render_body_keeps_drift_notice_between_context_and_tool_guidance() {
    let body = render_openai_body(&ProviderRequest {
        stream: false,
        assembly: assembly_with_drift_notice(),
        model: "gpt-5.4-mini".to_string(),
        max_output_tokens: Some(2048),
        temperature: Some(0.7),
        think_level: "off".to_string(),
        thinking: ResolvedThinking::default(),
    });

    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(body["messages"][2]["role"], "user");
    assert_eq!(body["messages"][3]["role"], "user");
    assert!(body["messages"][1]["content"]
        .as_str()
        .unwrap()
        .contains("<kuku_execution_context>"));
    assert!(body["messages"][2]["content"]
        .as_str()
        .unwrap()
        .contains("<kuku_system_notice>"));
    assert!(body["messages"][3]["content"]
        .as_str()
        .unwrap()
        .contains("<kuku_tool_guidance>"));
}

#[test]
fn openai_render_body_preserves_layer_order() {
    let body = render_openai_body(&ProviderRequest {
        stream: false,
        assembly: sample_assembly(),
        model: "gpt-5.4-mini".to_string(),
        max_output_tokens: Some(2048),
        temperature: Some(0.7),
        think_level: "off".to_string(),
        thinking: ResolvedThinking::default(),
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
        stream: false,
        assembly: assembly_with_tool_schema(),
        model: "gpt-5.4-mini".to_string(),
        max_output_tokens: None,
        temperature: None,
        think_level: "off".to_string(),
        thinking: ResolvedThinking::default(),
    });

    assert_eq!(tool_body["tools"][0]["type"], "function");
    assert_eq!(tool_body["tools"][0]["function"]["name"], "find_files");
    assert_eq!(
        tool_body["tools"][0]["function"]["parameters"]["type"],
        "object"
    );

    let history_body = render_openai_body(&ProviderRequest {
        stream: false,
        assembly: assembly_with_tool_history(),
        model: "gpt-5.4-mini".to_string(),
        max_output_tokens: None,
        temperature: None,
        think_level: "off".to_string(),
        thinking: ResolvedThinking::default(),
    });

    assert_eq!(
        history_body["messages"][3]["tool_calls"][0]["id"],
        "toolu_01"
    );
    assert_eq!(history_body["messages"][4]["role"], "tool");
    assert_eq!(history_body["messages"][4]["tool_call_id"], "toolu_01");
    assert_eq!(history_body["messages"][4]["content"], "README.md");
}

#[test]
fn parse_responses_sse_plain_text() {
    let sse = "\
event: response.created
data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\",\"object\":\"response\",\"status\":\"in_progress\",\"model\":\"gpt-5.4\",\"output\":[]}}

event: response.output_item.added
data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"msg_test\",\"type\":\"message\",\"status\":\"in_progress\",\"role\":\"assistant\",\"content\":[]}}

event: response.content_part.added
data: {\"type\":\"response.content_part.added\",\"item_id\":\"msg_test\",\"output_index\":0,\"content_index\":0,\"part\":{\"type\":\"output_text\",\"text\":\"\",\"annotations\":[]}}

event: response.output_text.delta
data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_test\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello\"}

event: response.output_text.delta
data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_test\",\"output_index\":0,\"content_index\":0,\"delta\":\" world\"}

event: response.output_text.done
data: {\"type\":\"response.output_text.done\",\"item_id\":\"msg_test\",\"output_index\":0,\"content_index\":0,\"text\":\"Hello world\"}

event: response.content_part.done
data: {\"type\":\"response.content_part.done\",\"item_id\":\"msg_test\",\"output_index\":0,\"content_index\":0,\"part\":{\"type\":\"output_text\",\"text\":\"Hello world\",\"annotations\":[]}}

event: response.output_item.done
data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"msg_test\",\"type\":\"message\",\"role\":\"assistant\",\"status\":\"completed\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello world\",\"annotations\":[]}]}}

event: response.completed
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15}}}
";

    let chunks = parse_responses_sse(sse);

    assert!(
        matches!(&chunks[0], ProviderChunk::StreamStart { request_id } if request_id == "resp_test")
    );

    let text: String = chunks
        .iter()
        .filter_map(|c| match c {
            ProviderChunk::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!("Hello world", text);

    assert!(chunks
        .iter()
        .any(|c| matches!(c, ProviderChunk::ContentBlockStop { index: 0 })));
    assert!(chunks
        .iter()
        .any(|c| matches!(c, ProviderChunk::StopReason { reason } if reason == "end_turn")));
    assert!(chunks.iter().any(|c| matches!(
        c,
        ProviderChunk::StreamUsage {
            input_tokens: 10,
            output_tokens: 5
        }
    )));
    assert!(chunks
        .last()
        .is_some_and(|c| matches!(c, ProviderChunk::StreamEnd)));
}

#[test]
fn parse_responses_sse_function_call() {
    let sse = "\
event: response.created
data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_fc\",\"object\":\"response\",\"status\":\"in_progress\",\"model\":\"gpt-5.4\",\"output\":[]}}

event: response.output_item.added
data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"fc_test\",\"type\":\"function_call\",\"call_id\":\"call_test123\",\"name\":\"get_weather\",\"arguments\":\"\",\"status\":\"in_progress\"}}

event: response.function_call_arguments.delta
data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_test\",\"output_index\":0,\"delta\":\"{\\\"location\\\":\"}

event: response.function_call_arguments.delta
data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_test\",\"output_index\":0,\"delta\":\"\\\"Boston\\\"}\"}

event: response.function_call_arguments.done
data: {\"type\":\"response.function_call_arguments.done\",\"item_id\":\"fc_test\",\"output_index\":0,\"item\":{\"id\":\"fc_test\",\"type\":\"function_call\",\"call_id\":\"call_test123\",\"name\":\"get_weather\",\"arguments\":\"{\\\"location\\\":\\\"Boston\\\"}\",\"status\":\"completed\"}}

event: response.output_item.done
data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"fc_test\",\"type\":\"function_call\",\"call_id\":\"call_test123\",\"name\":\"get_weather\",\"arguments\":\"{\\\"location\\\":\\\"Boston\\\"}\",\"status\":\"completed\"}}

event: response.completed
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_fc\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":30,\"output_tokens\":15,\"total_tokens\":45}}}
";

    let chunks = parse_responses_sse(sse);

    let start = chunks.iter().find_map(|c| match c {
        ProviderChunk::ToolCallStart { index, id, name } => {
            Some((*index, id.clone(), name.clone()))
        }
        _ => None,
    });
    assert!(start.is_some());
    let (index, id, name) = start.unwrap();
    assert_eq!(0u64, index);
    assert_eq!("call_test123", id);
    assert_eq!("get_weather", name);

    let args: String = chunks
        .iter()
        .filter_map(|c| match c {
            ProviderChunk::ToolCallArgDelta { index: 0, fragment } => Some(fragment.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!("{\"location\":\"Boston\"}", args);

    assert!(chunks
        .iter()
        .any(|c| matches!(c, ProviderChunk::ContentBlockStop { index: 0 })));
    assert!(chunks
        .last()
        .is_some_and(|c| matches!(c, ProviderChunk::StreamEnd)));
}

#[test]
fn parse_responses_sse_adds_stream_end_when_missing() {
    let sse = "\
event: response.created
data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_partial\",\"object\":\"response\",\"status\":\"in_progress\",\"model\":\"gpt-5.4\",\"output\":[]}}

event: response.output_text.delta
data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"partial text\"}
";
    let chunks = parse_responses_sse(sse);
    assert!(chunks
        .last()
        .is_some_and(|c| matches!(c, ProviderChunk::StreamEnd)));
}

#[test]
fn render_responses_body_basic_text() {
    let assembly = sample_assembly();
    let request = ProviderRequest {
        assembly,
        model: "gpt-5.4".to_string(),
        max_output_tokens: Some(100),
        temperature: None,
        stream: true,
        think_level: "off".to_string(),
        thinking: ResolvedThinking::default(),
    };

    let body = render_responses_body(&request);

    assert_eq!("gpt-5.4", body["model"].as_str().unwrap());
    assert_eq!(
        "You are the agent running inside kuku, a file-native software engineering runtime.",
        body["instructions"].as_str().unwrap()
    );
    assert_eq!(100, body["max_output_tokens"].as_u64().unwrap());
    assert!(body["input"].is_array());
    assert!(body.get("stream").is_none());
    assert!(body.get("reasoning").is_none());
}

#[test]
fn render_responses_body_with_tools() {
    let assembly = assembly_with_tool_schema();
    let request = ProviderRequest {
        assembly,
        model: "gpt-5.4".to_string(),
        max_output_tokens: None,
        temperature: None,
        stream: true,
        think_level: "off".to_string(),
        thinking: ResolvedThinking::default(),
    };

    let body = render_responses_body(&request);
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(1, tools.len());
    let tool = &tools[0];
    assert_eq!("function", tool["type"].as_str().unwrap());
    assert_eq!("find_files", tool["name"].as_str().unwrap());
    assert_eq!(true, tool["strict"].as_bool().unwrap());
    assert!(tool["parameters"].is_object());
}

#[test]
fn render_responses_body_with_reasoning() {
    let assembly = sample_assembly();
    let request = ProviderRequest {
        assembly,
        model: "gpt-5.4".to_string(),
        max_output_tokens: None,
        temperature: None,
        stream: true,
        think_level: "high".to_string(),
        thinking: ResolvedThinking::default(),
    };

    let body = render_responses_body(&request);
    let reasoning = body["reasoning"].as_object().unwrap();
    assert_eq!("xhigh", reasoning["effort"].as_str().unwrap());
}
