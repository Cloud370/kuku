use super::super::chunk::ProviderChunk;
use super::AnthropicSseParser;
use crate::config::ProviderProfile;
use crate::event::ModelStopReason;
use serde_json::json;

#[test]
fn routed_claude_messages_preview_only_requested_summary_and_keep_signed_replay() {
    for (profile, display, public) in [
        (ProviderProfile::OpenRouter, "summarized", true),
        (ProviderProfile::OpenRouter, "omitted", false),
        (ProviderProfile::Compatible, "summarized", false),
    ] {
        let mut parser = AnthropicSseParser::for_request(
            profile,
            "anthropic/claude-haiku-5.5",
            &json!({"thinking":{"type":"adaptive","display":display}}),
        );
        let chunks: Vec<_> = [
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Readable summary"}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"opaque-signature"}}"#,
            r#"data: {"type":"content_block_stop","index":0}"#,
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"redacted_thinking","data":"opaque-block"}}"#,
            r#"data: {"type":"content_block_stop","index":1}"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
            r#"data: {"type":"message_stop"}"#,
        ]
        .into_iter()
        .flat_map(|frame| parser.feed(frame).unwrap())
        .collect();
        let visible: Vec<_> = chunks
            .iter()
            .filter_map(|chunk| match chunk {
                ProviderChunk::ThinkingDelta { text } | ProviderChunk::TextDelta { text } => {
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect();
        let expected: &[&str] = if public { &["Readable summary"] } else { &[] };
        assert_eq!(expected, visible);
        assert!(chunks.iter().any(|chunk| matches!(chunk, ProviderChunk::Replay { value, .. }
            if value["content"] == json!([
                {"type":"thinking","thinking":"Readable summary","signature":"opaque-signature"},
                {"type":"redacted_thinking","data":"opaque-block"}
            ]))));
    }
}

#[test]
fn unsigned_thinking_does_not_prevent_tool_dispatch() {
    let mut parser = AnthropicSseParser::new(true, false);
    for frame in [
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"fixture reasoning"}}"#,
        r#"data: {"type":"content_block_stop","index":0}"#,
        r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call","name":"read_file","input":{}}}"#,
        r#"data: {"type":"content_block_stop","index":1}"#,
    ] {
        parser.feed(frame).unwrap();
    }

    let terminal = parser
        .feed(r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}"#)
        .unwrap();
    assert!(terminal.iter().any(|chunk| matches!(
        chunk,
        ProviderChunk::StopReason {
            reason: ModelStopReason::ToolUse
        }
    )));
    assert!(
        !terminal
            .iter()
            .any(|chunk| matches!(chunk, ProviderChunk::InvalidToolStream))
    );

    let ended = parser.feed(r#"data: {"type":"message_stop"}"#).unwrap();
    let replay = ended.iter().find_map(|chunk| match chunk {
        ProviderChunk::Replay { value, .. } => Some(value),
        _ => None,
    });
    assert_eq!(
        Some(&json!([{"type":"tool_use","id":"call","name":"read_file","input":{}}])),
        replay.and_then(|value| value.get("content"))
    );
}

#[test]
fn empty_thinking_signature_is_valid_but_omitted_from_native_replay() {
    let mut parser = AnthropicSseParser::new(true, false);
    for frame in [
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"fixture reasoning"}}"#,
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":""}}"#,
        r#"data: {"type":"content_block_stop","index":0}"#,
        r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
        r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"answer"}}"#,
        r#"data: {"type":"content_block_stop","index":1}"#,
    ] {
        parser.feed(frame).unwrap();
    }

    let terminal = parser
        .feed(r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#)
        .unwrap();
    assert!(matches!(
        terminal.as_slice(),
        [ProviderChunk::StopReason {
            reason: ModelStopReason::EndTurn
        }]
    ));

    let ended = parser.feed(r#"data: {"type":"message_stop"}"#).unwrap();
    let replay = ended.iter().find_map(|chunk| match chunk {
        ProviderChunk::Replay { value, .. } => Some(value),
        _ => None,
    });
    assert_eq!(
        Some(&json!([{"type":"text","text":"answer"}])),
        replay.and_then(|value| value.get("content"))
    );
}
