use serde_json::json;

use super::{
    assembly_with_multiblock_current_user, assembly_with_recovery_notice,
    assembly_with_tool_history, assembly_with_tool_schema, current_input, render_responses_body,
    sample_assembly, test_catalog, ProviderRequest, ResolvedThinking,
};

#[test]
fn render_responses_body_includes_prior_assistant_function_call() {
    let catalog = test_catalog();
    let body = render_responses_body(&ProviderRequest {
        stream: false,
        assembly: assembly_with_tool_history(),
        catalog: &catalog,
        current_input: current_input(),
        model: "gpt-5.4".to_string(),
        max_output_tokens: None,
        temperature: None,
        think_level: kuku::config::ThinkLevel::Off,
        thinking: ResolvedThinking::default(),
    });

    let input = body["input"].as_array().unwrap();

    assert!(input.iter().any(|item| {
        item["type"] == "function_call"
            && item["call_id"] == "toolu_01"
            && item["name"] == "find_files"
            && item["arguments"] == json!("{\"path\":\".\"}")
    }));
    assert!(input.iter().any(|item| {
        item["type"] == "function_call_output"
            && item["call_id"] == "toolu_01"
            && item["output"] == "README.md"
    }));
}

#[test]
fn render_responses_body_maps_current_user_blocks_to_content_parts() {
    let catalog = test_catalog();
    let body = render_responses_body(&ProviderRequest {
        stream: false,
        assembly: assembly_with_multiblock_current_user(),
        catalog: &catalog,
        current_input: current_input(),
        model: "gpt-5.4".to_string(),
        max_output_tokens: None,
        temperature: None,
        think_level: kuku::config::ThinkLevel::Off,
        thinking: ResolvedThinking::default(),
    });

    let input = body["input"].as_array().unwrap();
    let current = input.last().unwrap();
    let content = current["content"].as_array().unwrap();

    assert_eq!(current["type"], "message");
    assert_eq!(current["role"], "user");
    assert_eq!(
        content[0],
        json!({"type": "input_text", "text": "<kuku_runtime_notices>notice</kuku_runtime_notices>"})
    );
    assert_eq!(
        content[1],
        json!({"type": "input_text", "text": "<kuku_conversation_inbox>inbox</kuku_conversation_inbox>"})
    );
    assert_eq!(
        content[2],
        json!({"type": "input_text", "text": "raw user input"})
    );
}

#[test]
fn responses_recovery_keeps_original_request_as_exact_prefix() {
    let catalog = test_catalog();
    let render = |assembly| {
        render_responses_body(&ProviderRequest {
            stream: false,
            assembly,
            catalog: &catalog,
            current_input: current_input(),
            model: "gpt-5.4".to_string(),
            max_output_tokens: Some(37),
            temperature: None,
            think_level: kuku::config::ThinkLevel::Off,
            thinking: ResolvedThinking::default(),
        })
    };
    let original = render(assembly_with_multiblock_current_user());
    let recovered = render(assembly_with_recovery_notice());
    let original_input = original["input"].as_array().unwrap();
    let recovered_input = recovered["input"].as_array().unwrap();
    let original_current = original_input.last().unwrap()["content"]
        .as_array()
        .unwrap();
    let recovered_current = recovered_input.last().unwrap()["content"]
        .as_array()
        .unwrap();

    assert_eq!("developer", original_input[0]["role"]);
    assert_eq!(
        &original_input[..original_input.len() - 1],
        &recovered_input[..recovered_input.len() - 1]
    );
    assert_eq!(
        original_current,
        &recovered_current[..original_current.len()]
    );
    assert_eq!(recovered["max_output_tokens"], 37);
}

#[test]
fn render_responses_body_basic_text() {
    let catalog = test_catalog();
    let assembly = sample_assembly();
    let request = ProviderRequest {
        assembly,
        catalog: &catalog,
        current_input: current_input(),
        model: "gpt-5.4".to_string(),
        max_output_tokens: Some(100),
        temperature: None,
        stream: true,
        think_level: kuku::config::ThinkLevel::Off,
        thinking: ResolvedThinking::default(),
    };

    let body = render_responses_body(&request);

    assert_eq!("gpt-5.4", body["model"].as_str().unwrap());
    assert_eq!("developer", body["input"][0]["role"]);
    assert_eq!(
        "You are the agent running inside kuku, a file-native software engineering runtime.",
        body["input"][0]["content"][0]["text"].as_str().unwrap()
    );
    assert_eq!(100, body["max_output_tokens"].as_u64().unwrap());
    assert!(body["input"].is_array());
    assert!(body.get("stream").is_none());
    assert!(body.get("reasoning").is_none());
}

#[test]
fn render_responses_body_with_tools() {
    let catalog = test_catalog();
    let assembly = assembly_with_tool_schema();
    let request = ProviderRequest {
        assembly,
        catalog: &catalog,
        current_input: current_input(),
        model: "gpt-5.4".to_string(),
        max_output_tokens: None,
        temperature: None,
        stream: true,
        think_level: kuku::config::ThinkLevel::Off,
        thinking: ResolvedThinking::default(),
    };

    let body = render_responses_body(&request);
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(1, tools.len());
    let tool = &tools[0];
    assert_eq!("function", tool["type"].as_str().unwrap());
    assert_eq!("find_files", tool["name"].as_str().unwrap());
    assert!(tool["strict"].is_null());
    assert!(tool["parameters"].is_object());
}

#[test]
fn render_responses_body_with_reasoning() {
    let catalog = test_catalog();
    let assembly = sample_assembly();
    let request = ProviderRequest {
        assembly,
        catalog: &catalog,
        current_input: current_input(),
        model: "gpt-5.4".to_string(),
        max_output_tokens: None,
        temperature: None,
        stream: true,
        think_level: kuku::config::ThinkLevel::High,
        thinking: ResolvedThinking::default(),
    };

    let body = render_responses_body(&request);
    let reasoning = body["reasoning"].as_object().unwrap();
    assert_eq!("xhigh", reasoning["effort"].as_str().unwrap());
}
