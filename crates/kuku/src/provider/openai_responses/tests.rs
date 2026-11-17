use serde_json::json;

use crate::event::ModelStopReason;

use super::super::chunk::ProviderChunk;
use super::OpenAiResponsesSseParser;

#[test]
fn done_after_response_terminal_preserves_the_original_outcome() {
    for (event, response, reason) in [
        (
            "response.completed",
            json!({"status":"completed","output":[]}),
            Some(ModelStopReason::EndTurn),
        ),
        (
            "response.incomplete",
            json!({"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}),
            Some(ModelStopReason::Length),
        ),
        (
            "response.failed",
            json!({"status":"failed","error":{"code":"server_error","message":"failed"}}),
            None,
        ),
    ] {
        let mut parser = OpenAiResponsesSseParser::new();
        let chunks = parser
            .feed(&format!(
                "event: {event}\ndata: {}",
                json!({"response":response})
            ))
            .unwrap();
        assert!(parser.feed("data: [DONE]").unwrap().is_empty());
        assert!(parser.finish().unwrap().is_empty());
        let reasons = chunks
            .iter()
            .filter_map(|chunk| match chunk {
                ProviderChunk::StopReason { reason } => Some(reason.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(reason.clone().into_iter().collect::<Vec<_>>(), reasons);
        assert_eq!(
            usize::from(reason.is_some()),
            chunks
                .iter()
                .filter(|chunk| matches!(chunk, ProviderChunk::StreamEnd))
                .count()
        );
        assert_eq!(
            reason.is_none(),
            chunks
                .iter()
                .any(|chunk| matches!(chunk, ProviderChunk::ServerError { .. }))
        );
    }
}

#[test]
fn done_without_a_response_terminal_event_does_not_complete_the_stream() {
    for event in [
        concat!(
            "event: response.created\n",
            r#"data: {"response":{"id":"pending","status":"in_progress"}}"#
        ),
        concat!(
            "event: error\n",
            r#"data: {"code":"server_error","message":"failed"}"#
        ),
    ] {
        let mut parser = OpenAiResponsesSseParser::new();
        let chunks = parser.feed(event).unwrap();
        assert!(!chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StopReason { .. } | ProviderChunk::StreamEnd
        )));
        let error = parser.feed("data: [DONE]").unwrap_err();
        assert_eq!(
            "provider emitted [DONE] before a terminal response event",
            error.message
        );
    }
    let mut parser = OpenAiResponsesSseParser::new();
    assert!(parser.feed("data: [DONE]").is_err());
    assert!(parser.finish().is_err());
}

#[test]
fn completed_usage_reports_cached_input_tokens() {
    let mut parser = OpenAiResponsesSseParser::new();
    let frame = concat!(
        "event: response.completed\n",
        r#"data: {"type":"response.completed","response":{"id":"resp_usage","status":"completed","usage":{"input_tokens":120,"input_tokens_details":{"cached_tokens":70},"output_tokens":30,"output_tokens_details":{"reasoning_tokens":12}}}}"#,
    );

    let chunks = parser.feed(frame).unwrap();

    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        ProviderChunk::StreamUsage {
            input_tokens: Some(120),
            output_tokens: Some(30),
            cache_read_input_tokens: Some(70),
            cache_creation_input_tokens: None
        }
    )));
    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        ProviderChunk::StopReason { reason } if reason == &ModelStopReason::EndTurn
    )));
}

#[test]
fn reasoning_text_delta_emits_thinking() {
    let mut parser = OpenAiResponsesSseParser::new();
    let frame = concat!(
        "event: response.reasoning_text.delta\n",
        r#"data: {"type":"response.reasoning_text.delta","item_id":"rs_123","output_index":0,"content_index":0,"delta":"considering"}"#,
    );

    let chunks = parser.feed(frame).unwrap();

    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        ProviderChunk::ThinkingDelta { text } if text == "considering"
    )));
}

#[test]
fn incomplete_response_is_terminal_and_preserves_usage() {
    let mut parser = OpenAiResponsesSseParser::new();
    let frame = concat!(
        "event: response.incomplete\n",
        r#"data: {"type":"response.incomplete","response":{"id":"resp_incomplete","status":"incomplete","incomplete_details":{"reason":"max_tokens"},"usage":{"input_tokens":120,"input_tokens_details":{"cached_tokens":70},"output_tokens":30}}}"#,
    );

    let mut chunks = parser.feed(frame).unwrap();
    chunks.extend(parser.finish().unwrap());

    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        ProviderChunk::StreamUsage {
            input_tokens: Some(120),
            output_tokens: Some(30),
            cache_read_input_tokens: Some(70),
            cache_creation_input_tokens: None
        }
    )));
    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        ProviderChunk::StopReason { reason } if reason == &ModelStopReason::Length
    )));
    assert!(chunks
        .last()
        .is_some_and(|chunk| matches!(chunk, ProviderChunk::StreamEnd)));
}

#[test]
fn incomplete_response_without_reason_is_explicitly_incomplete() {
    let mut parser = OpenAiResponsesSseParser::new();
    let frame = concat!(
        "event: response.incomplete\n",
        r#"data: {"type":"response.incomplete","response":{"id":"resp_incomplete","status":"incomplete","usage":{"input_tokens":1,"output_tokens":2}}}"#,
    );

    let chunks = parser.feed(frame).unwrap();

    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        ProviderChunk::StopReason { reason } if reason == &ModelStopReason::Incomplete
    )));
}

#[test]
fn completed_response_with_matching_function_call_is_tool_use() {
    let mut parser = OpenAiResponsesSseParser::new();
    parser
        .feed(concat!(
            "event: response.output_item.added\n",
            r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file"}}"#,
        ))
        .unwrap();
    parser
        .feed(concat!(
            "event: response.output_item.done\n",
            r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}}"#,
        ))
        .unwrap();

    let chunks = parser
        .feed(concat!(
            "event: response.completed\n",
            r#"data: {"response":{"status":"completed","output":[{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}]}}"#,
        ))
        .unwrap();

    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        ProviderChunk::StopReason { reason } if reason == &ModelStopReason::ToolUse
    )));
}

#[test]
fn function_call_done_emits_typed_tool_completion() {
    let mut parser = OpenAiResponsesSseParser::new();
    parser
        .feed(concat!(
            "event: response.output_item.added\n",
            r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file"}}"#,
        ))
        .unwrap();

    let chunks = parser
        .feed(concat!(
            "event: response.output_item.done\n",
            r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}}"#,
        ))
        .unwrap();

    assert!(matches!(
        chunks.as_slice(),
        [
            ProviderChunk::ToolCallArgDelta { index: 0, fragment },
            ProviderChunk::ToolCallStop { index: 0 }
        ] if fragment == "{}"
    ));
}

#[test]
fn completed_response_with_missing_call_marker_item_is_invalid() {
    let mut parser = OpenAiResponsesSseParser::new();
    parser
        .feed(concat!(
            "event: response.output_item.added\n",
            r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file"}}"#,
        ))
        .unwrap();
    parser
        .feed(concat!(
            "event: response.output_item.done\n",
            r#"data: {"output_index":0}"#,
        ))
        .unwrap();

    let chunks = parser
        .feed(concat!(
            "event: response.completed\n",
            r#"data: {"response":{"status":"completed","output":[{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}]}}"#,
        ))
        .unwrap();

    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse
    )));
}

#[test]
fn completed_response_with_mismatched_call_marker_is_invalid() {
    let mut parser = OpenAiResponsesSseParser::new();
    parser
        .feed(concat!(
            "event: response.output_item.added\n",
            r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}}"#,
        ))
        .unwrap();
    parser
        .feed(concat!(
            "event: response.output_item.done\n",
            r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file"}}"#,
        ))
        .unwrap();

    let chunks = parser
        .feed(concat!(
            "event: response.completed\n",
            r#"data: {"response":{"status":"completed","output":[{"type":"function_call","call_id":"call_2","name":"read_file","arguments":"{}"}]}}"#,
        ))
        .unwrap();

    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse
    )));
}

#[test]
fn authoritative_done_arguments_are_emitted_once_without_deltas() {
    let mut parser = OpenAiResponsesSseParser::new();
    parser.feed(concat!(
        "event: response.output_item.added\n",
        r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"write_file"}}"#,
    )).unwrap();

    let done_chunks = parser
        .feed(concat!(
            "event: response.function_call_arguments.done\n",
            r#"data: {"output_index":0,"arguments":"{\"path\":\"a.txt\"}"}"#,
        ))
        .unwrap();
    let item_chunks = parser.feed(concat!(
        "event: response.output_item.done\n",
        r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"write_file","arguments":"{\"path\":\"a.txt\"}"}}"#,
    )).unwrap();
    let terminal_chunks = parser.feed(concat!(
        "event: response.completed\n",
        r#"data: {"response":{"status":"completed","output":[{"type":"function_call","call_id":"call_1","name":"write_file","arguments":"{\"path\":\"a.txt\"}"}]}}"#,
    )).unwrap();

    assert!(
        matches!(done_chunks.as_slice(), [ProviderChunk::ToolCallArgDelta { index: 0, fragment }] if fragment == "{\"path\":\"a.txt\"}")
    );
    assert!(matches!(
        item_chunks.as_slice(),
        [ProviderChunk::ToolCallStop { index: 0 }]
    ));
    assert!(terminal_chunks.iter().any(|chunk| matches!(chunk, ProviderChunk::StopReason { reason } if reason == &ModelStopReason::ToolUse)));
}

#[test]
fn done_item_arguments_are_authoritative_without_deltas() {
    let mut parser = OpenAiResponsesSseParser::new();
    parser
        .feed(concat!(
            "event: response.output_item.added\n",
            r#"data: {"output_index":0,"item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"write_file","arguments":""}}"#,
        ))
        .unwrap();

    let done_chunks = parser
        .feed(concat!(
            "event: response.function_call_arguments.done\n",
            r#"data: {"output_index":0,"item_id":"fc_1","item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"write_file","arguments":"{\"path\":\"a.txt\"}"}}"#,
        ))
        .unwrap();
    let item_chunks = parser
        .feed(concat!(
            "event: response.output_item.done\n",
            r#"data: {"output_index":0,"item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"write_file","arguments":"{\"path\":\"a.txt\"}"}}"#,
        ))
        .unwrap();
    let terminal_chunks = parser
        .feed(concat!(
            "event: response.completed\n",
            r#"data: {"response":{"status":"completed","output":[{"id":"fc_1","type":"function_call","call_id":"call_1","name":"write_file","arguments":"{\"path\":\"a.txt\"}"}]}}"#,
        ))
        .unwrap();

    assert!(matches!(
        done_chunks.as_slice(),
        [ProviderChunk::ToolCallArgDelta { index: 0, fragment }]
            if fragment == "{\"path\":\"a.txt\"}"
    ));
    assert!(matches!(
        item_chunks.as_slice(),
        [ProviderChunk::ToolCallStop { index: 0 }]
    ));
    assert!(terminal_chunks.iter().any(|chunk| matches!(
        chunk,
        ProviderChunk::StopReason { reason } if reason == &ModelStopReason::ToolUse
    )));
}

#[test]
fn done_item_and_top_level_arguments_must_match() {
    let mut parser = OpenAiResponsesSseParser::new();
    parser
        .feed(concat!(
            "event: response.output_item.added\n",
            r#"data: {"output_index":0,"item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"read_file"}}"#,
        ))
        .unwrap();
    parser
        .feed(concat!(
            "event: response.function_call_arguments.done\n",
            r#"data: {"output_index":0,"item_id":"fc_1","arguments":"{\"path\":\"top-level\"}","item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"item\"}"}}"#,
        ))
        .unwrap();
    let chunks = parser
        .feed(concat!(
            "event: response.completed\n",
            r#"data: {"response":{"status":"completed","output":[]}}"#,
        ))
        .unwrap();

    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse
    )));
}

#[test]
fn done_item_identity_mismatch_is_invalid() {
    for (field, value) in [
        ("item_id", "fc_other"),
        ("call_id", "call_other"),
        ("name", "other_tool"),
    ] {
        let mut parser = OpenAiResponsesSseParser::new();
        parser
            .feed(concat!(
                "event: response.output_item.added\n",
                r#"data: {"output_index":0,"item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"read_file"}}"#,
            ))
            .unwrap();
        let item = format!(
            "{{\"id\":\"{}\",\"type\":\"function_call\",\"call_id\":\"{}\",\"name\":\"{}\",\"arguments\":\"{{}}\"}}",
            if field == "item_id" { value } else { "fc_1" },
            if field == "call_id" { value } else { "call_1" },
            if field == "name" { value } else { "read_file" },
        );
        parser
            .feed(&format!(
                "event: response.function_call_arguments.done\ndata: {{\"output_index\":0,\"item_id\":\"fc_1\",\"item\":{item}}}"
            ))
            .unwrap();
        let chunks = parser
            .feed(concat!(
                "event: response.completed\n",
                r#"data: {"response":{"status":"completed","output":[]}}"#,
            ))
            .unwrap();
        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse
        )));
    }
}

#[test]
fn terminal_item_identity_mismatch_is_invalid() {
    let mut parser = OpenAiResponsesSseParser::new();
    parser
        .feed(concat!(
            "event: response.output_item.added\n",
            r#"data: {"output_index":0,"item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"read_file"}}"#,
        ))
        .unwrap();
    parser
        .feed(concat!(
            "event: response.output_item.done\n",
            r#"data: {"output_index":0,"item":{"id":"fc_other","type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}}"#,
        ))
        .unwrap();
    let chunks = parser
        .feed(concat!(
            "event: response.completed\n",
            r#"data: {"response":{"status":"completed","output":[{"id":"fc_other","type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}]}}"#,
        ))
        .unwrap();

    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse
    )));
}

#[test]
fn function_item_identity_cannot_be_reused_across_indices() {
    let mut parser = OpenAiResponsesSseParser::new();
    parser
        .feed(concat!(
            "event: response.output_item.added\n",
            r#"data: {"output_index":0,"item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"read_file"}}"#,
        ))
        .unwrap();
    parser
        .feed(concat!(
            "event: response.output_item.added\n",
            r#"data: {"output_index":1,"item":{"id":"fc_1","type":"function_call","call_id":"call_2","name":"read_file"}}"#,
        ))
        .unwrap();
    let chunks = parser
        .feed(concat!(
            "event: response.completed\n",
            r#"data: {"response":{"status":"completed","output":[]}}"#,
        ))
        .unwrap();

    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse
    )));
}

#[test]
fn mismatched_argument_representations_are_invalid() {
    let cases = [
        (
            Some("event: response.function_call_arguments.delta\ndata: {\"output_index\":0,\"delta\":\"{\\\"path\\\":\\\"a.txt\\\"}\"}"),
            "event: response.function_call_arguments.done\ndata: {\"output_index\":0,\"arguments\":\"{\\\"path\\\":\\\"b.txt\\\"}\"}",
            r#"{\"path\":\"b.txt\"}"#,
            r#"{\"path\":\"b.txt\"}"#,
        ),
        (
            None,
            "event: response.function_call_arguments.done\ndata: {\"output_index\":0,\"arguments\":\"{\\\"path\\\":\\\"a.txt\\\"}\"}",
            r#"{\"path\":\"b.txt\"}"#,
            r#"{\"path\":\"b.txt\"}"#,
        ),
    ];

    for (delta, done, item_arguments, output_arguments) in cases {
        let mut parser = OpenAiResponsesSseParser::new();
        parser.feed(concat!(
            "event: response.output_item.added\n",
            r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"write_file"}}"#,
        )).unwrap();
        if let Some(delta) = delta {
            parser.feed(delta).unwrap();
        }
        parser.feed(done).unwrap();
        parser.feed(&format!(
            "event: response.output_item.done\ndata: {{\"output_index\":0,\"item\":{{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"write_file\",\"arguments\":{}}}}}",
            serde_json::to_string(item_arguments).unwrap()
        )).unwrap();
        let chunks = parser.feed(&format!(
            "event: response.completed\ndata: {{\"response\":{{\"status\":\"completed\",\"output\":[{{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"write_file\",\"arguments\":{}}}]}}}}",
            serde_json::to_string(output_arguments).unwrap()
        )).unwrap();

        assert!(chunks.iter().any(|chunk| matches!(chunk, ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse)));
    }
}

#[test]
fn missing_done_or_terminal_arguments_are_invalid() {
    let cases = [(None, Some("{}")), (Some("{}"), None)];

    for (done_arguments, terminal_arguments) in cases {
        let mut parser = OpenAiResponsesSseParser::new();
        parser.feed(concat!(
            "event: response.output_item.added\n",
            r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file"}}"#,
        )).unwrap();
        let done = done_arguments.map_or_else(
            || "event: response.function_call_arguments.done\ndata: {\"output_index\":0}".to_string(),
            |arguments| format!("event: response.function_call_arguments.done\ndata: {{\"output_index\":0,\"arguments\":{}}}", serde_json::to_string(arguments).unwrap()),
        );
        parser.feed(&done).unwrap();
        parser.feed(concat!(
            "event: response.output_item.done\n",
            r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}}"#,
        )).unwrap();
        let output_item = terminal_arguments.map_or_else(
            || r#"{"type":"function_call","call_id":"call_1","name":"read_file"}"#.to_string(),
            |arguments| format!(r#"{{"type":"function_call","call_id":"call_1","name":"read_file","arguments":{}}}"#, serde_json::to_string(arguments).unwrap()),
        );
        let chunks = parser.feed(&format!(
            "event: response.completed\ndata: {{\"response\":{{\"status\":\"completed\",\"output\":[{output_item}]}}}}"
        )).unwrap();

        assert!(chunks.iter().any(|chunk| matches!(chunk, ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse)));
    }
}

#[test]
fn output_index_reused_across_item_types_is_invalid() {
    let mut parser = OpenAiResponsesSseParser::new();
    parser
        .feed(concat!(
            "event: response.output_item.added\n",
            r#"data: {"output_index":0,"item":{"type":"message","id":"msg_1"}}"#,
        ))
        .unwrap();
    parser.feed(concat!(
        "event: response.output_item.added\n",
        r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file"}}"#,
    )).unwrap();
    parser
        .feed(concat!(
            "event: response.function_call_arguments.done\n",
            r#"data: {"output_index":0,"arguments":"{}"}"#,
        ))
        .unwrap();
    parser.feed(concat!(
        "event: response.output_item.done\n",
        r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}}"#,
    )).unwrap();
    let chunks = parser.feed(concat!(
        "event: response.completed\n",
        r#"data: {"response":{"status":"completed","output":[{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}]}}"#,
    )).unwrap();

    assert!(chunks.iter().any(|chunk| matches!(chunk, ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse)));
}

#[test]
fn missing_or_malformed_tool_output_index_is_invalid() {
    for output_index in ["null", "\"zero\""] {
        let mut parser = OpenAiResponsesSseParser::new();
        parser.feed(&format!(
            "event: response.output_item.added\ndata: {{\"output_index\":{output_index},\"item\":{{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"read_file\"}}}}"
        )).unwrap();
        let chunks = parser
            .feed(concat!(
                "event: response.completed\n",
                r#"data: {"response":{"status":"completed","output":[]}}"#,
            ))
            .unwrap();
        assert!(chunks.iter().any(|chunk| matches!(chunk, ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse)));
    }
}

#[test]
fn failed_response_is_terminal_and_preserves_error_details() {
    let mut parser = OpenAiResponsesSseParser::new();
    let frame = concat!(
        "event: response.failed\n",
        r#"data: {"type":"response.failed","response":{"id":"resp_failed","status":"failed","error":{"code":"server_error","message":"generation failed"}}}"#,
    );

    let chunks = parser.feed(frame).unwrap();

    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        ProviderChunk::ServerError { code, message }
            if code == "server_error" && message == "generation failed"
    )));
    assert!(parser.finish().is_ok());
}

#[test]
fn error_event_preserves_top_level_error_details() {
    let mut parser = OpenAiResponsesSseParser::new();
    let frame = concat!(
        "event: error\n",
        r#"data: {"type":"error","code":"server_error","message":"stream failed","param":null}"#,
    );

    let chunks = parser.feed(frame).unwrap();

    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        ProviderChunk::ServerError { code, message }
            if code == "server_error" && message == "stream failed"
    )));
    assert!(parser.finish().is_ok());
}

#[test]
fn truncated_stream_does_not_emit_stream_end() {
    let mut parser = OpenAiResponsesSseParser::new();
    let mut chunks = parser
        .feed(
        "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_truncated\",\"status\":\"in_progress\"}}",
    )
        .unwrap();
    chunks.extend(parser.feed(
        "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"partial\"}",
    ).unwrap());
    chunks.extend(parser.feed("").unwrap());

    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        ProviderChunk::TextDelta { text } if text == "partial"
    )));
    assert!(!chunks
        .iter()
        .any(|chunk| matches!(chunk, ProviderChunk::StreamEnd)));
}
