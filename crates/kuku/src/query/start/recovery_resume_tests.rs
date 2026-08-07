use super::{resumed_model_request_count, resumed_request_num, resumed_tool_rounds};
use crate::conversation::address::ConversationAddress;
use crate::event::{EventPayload, ModelStopReason, StoredEvent};

fn response(id: u64, conversation: Option<&str>, request_id: &str) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::ModelResponse {
            conversation: conversation.map(str::to_string),
            turn: 4,
            ts: "t".to_string(),
            request_id: request_id.to_string(),
            text: String::new(),
            thinking: None,
            stop_reason: Some(ModelStopReason::Length),
            input_tokens_total: None,
            output_tokens_total: None,
        },
    }
}

fn recovery(id: u64, conversation: &str, from: &str, to: &str) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::ModelRecovery {
            conversation: conversation.to_string(),
            turn: 4,
            ts: "t".to_string(),
            from_request_id: from.to_string(),
            to_request_id: to.to_string(),
            reason: ModelStopReason::Length,
            attempt: 1,
            max_attempts: 1,
            failed_max_output_tokens: 32,
            retry_max_output_tokens: 32,
            output_tokens_total: None,
            discarded_tool_calls: 0,
            notice: "notice".to_string(),
            prompt_path: "runtime/recovery.md".to_string(),
            prompt_hash: "sha256:test".to_string(),
        },
    }
}

fn tool_call(id: u64, conversation: Option<&str>, request_id: &str) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::ToolCall {
            turn: 4,
            ts: "t".to_string(),
            conversation: conversation.map(str::to_string),
            tool_call_id: format!("tool_{id}"),
            request_id: request_id.to_string(),
            index: 0,
            tool: "read_file".to_string(),
            args: serde_json::json!({}),
        },
    }
}

#[test]
fn resumed_request_identity_is_scoped_to_conversation() {
    let events = vec![
        response(1, None, "req_9"),
        response(2, Some("review"), "req_1"),
        recovery(3, "review", "req_1", "req_2"),
        tool_call(4, None, "req_9"),
        tool_call(5, Some("review"), "req_1"),
    ];
    let main = ConversationAddress::MAIN;
    let review = ConversationAddress::parse("review").unwrap();

    assert_eq!(1, resumed_model_request_count(&events, 4, &main));
    assert_eq!(9, resumed_request_num(&events, 4, &main));
    assert_eq!(1, resumed_tool_rounds(&events, 4, &main));

    assert_eq!(2, resumed_model_request_count(&events, 4, &review));
    assert_eq!(2, resumed_request_num(&events, 4, &review));
    assert_eq!(1, resumed_tool_rounds(&events, 4, &review));
}
