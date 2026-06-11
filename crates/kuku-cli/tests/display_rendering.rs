use std::time::Duration;

use kuku_cli::display::{filter_events_for_conversation, render_event_brief};
use kuku_cli::display::{Display, OutputLine};

#[test]
fn thinking_default_hides_text() {
    let mut d = Display::new(false, "medium");
    assert!(d.thinking_text("secret reasoning").is_none());
    assert!(d.thinking_start().contains("thinking"));
    assert!(d.thinking_end(Duration::from_millis(3200)).contains("3.2s"));
}

#[test]
fn thinking_show_thinking_reveals_text() {
    let d = Display::new(true, "medium");
    assert_eq!(
        d.thinking_text("secret reasoning"),
        Some("secret reasoning".into())
    );
}

#[test]
fn tool_call_format() {
    let d = Display::new(false, "medium");
    let line = d.tool_call("read_file", "src/main.rs", "tc_01");
    assert!(line.contains("read_file"));
    assert!(line.contains("src/main.rs"));
}

#[test]
fn permission_ask_format() {
    let d = Display::new(false, "medium");
    let line = d.permission_ask("run_command", "cargo build");
    assert!(line.contains("?"));
    assert!(line.contains("run_command"));
    assert!(line.contains("(Y/n)?"));
}

#[test]
fn error_format() {
    let d = Display::new(false, "medium");
    let line = d.error("provider", "auth", "invalid API key");
    assert!(line.contains("!!"));
    assert!(line.contains("provider"));
    assert!(line.contains("auth"));
}

#[test]
fn session_start_shows_tier_and_model() {
    let d = Display::new(false, "medium");
    let line = d.session_start("abc123", "strong", "claude-sonnet-4-6");
    assert!(line.contains("abc123"));
    assert!(line.contains("strong"));
    assert!(line.contains("claude-sonnet-4-6"));
}

#[test]
fn session_completed_shows_in_out_tokens() {
    let d = Display::new(false, "medium");
    let line = d.session_completed("s_001", 2, 35000, 7000, 0, 0, Duration::from_secs(18));
    assert!(
        line.contains("in 35.0k"),
        "should show input tokens: {line}"
    );
    assert!(
        line.contains("out 7.0k"),
        "should show output tokens: {line}"
    );
    assert!(line.contains("2 turns"));
    assert!(line.contains("18s"));
}

#[test]
fn code_block_basic() {
    let d = Display::new(false, "medium");
    assert!(d.code_block_open(Some("rust")).contains("rust"));
}

#[test]
fn table_row_pads_cells() {
    let d = Display::new(false, "medium");
    let row = d.table_row(&["foo", "12"], &[8, 6]);
    assert!(row.contains("foo"));
    assert!(row.contains("12"));
}

// ── JSON tests ──

#[test]
fn json_thinking_serializes() {
    let line = OutputLine::thinking(1200, None);
    let json = line.to_json_line();
    assert!(json.contains("\"type\":\"thinking\""));
    assert!(json.contains("\"duration_ms\":1200"));
    assert!(!json.contains("\"text\""));
}

#[test]
fn json_thinking_serializes_text() {
    let line = OutputLine::thinking(1200, Some("reasoning...".into()));
    let json = line.to_json_line();
    assert!(json.contains("\"text\":\"reasoning...\""));
}

#[test]
fn json_tool_call_serializes() {
    let line = OutputLine::tool_call(
        "read_file".into(),
        "tc_01".into(),
        "src/main.rs".into(),
        serde_json::json!({"path": "src/main.rs"}),
    )
    .with_conversation(Some("review".into()));
    let json = line.to_json_line();
    assert!(json.contains("\"type\":\"tool_call\""));
    assert!(json.contains("\"tool_call_id\":\"tc_01\""));
    assert!(json.contains("\"conversation\":\"review\""));
}

#[test]
fn json_permission_ask_serializes_existing_schema() {
    let line = OutputLine::permission_ask(
        "perm_1".into(),
        "run_command".into(),
        "execute".into(),
        "cargo test".into(),
    )
    .with_conversation(Some("review/api".into()));
    let json: serde_json::Value = serde_json::from_str(&line.to_json_line()).unwrap();

    assert_eq!(json["type"], "permission_ask");
    assert_eq!(json["request_id"], "perm_1");
    assert_eq!(json["tool"], "run_command");
    assert_eq!(json["risk"], "execute");
    assert_eq!(json["summary"], "cargo test");
    assert_eq!(json["conversation"], "review/api");
}

#[test]
fn derive_final_output_defaults_to_main_conversation() {
    let events = vec![
        kuku::event::StoredEvent {
            id: 1,
            payload: kuku::event::EventPayload::MessageAssistant {
                ts: "t0".into(),
                conversation: "review".into(),
                turn: 1,
                message_id: "m_review".into(),
                text: "review answer".into(),
            },
        },
        kuku::event::StoredEvent {
            id: 2,
            payload: kuku::event::EventPayload::ModelResponse {
                turn: 1,
                ts: "t1".into(),
                request_id: "req_main".into(),
                text: "main answer".into(),
                thinking: None,
                input_tokens_total: None,
            },
        },
        kuku::event::StoredEvent {
            id: 3,
            payload: kuku::event::EventPayload::TurnCompleted {
                ts: "t2".into(),
                conversation: "review".into(),
                turn: 1,
            },
        },
        kuku::event::StoredEvent {
            id: 4,
            payload: kuku::event::EventPayload::TurnCompleted {
                conversation: "main".into(),
                turn: 1,
                ts: "t3".into(),
            },
        },
    ];

    assert_eq!(
        kuku_cli::display::derive_final_output(&events),
        Some("main answer".into())
    );
}

#[test]
fn json_error_serializes() {
    let line = OutputLine::error("provider".into(), "auth".into(), "invalid key".into(), None);
    let json = line.to_json_line();
    assert!(json.contains("\"type\":\"error\""));
    assert!(json.contains("\"source\":\"provider\""));
}

#[test]
fn json_log_serializes_record_for_host_streams() {
    let mut record = kuku::log::LogRecord::new(
        "2026-06-06T00:00:00Z",
        kuku::log::LogLevel::Info,
        kuku::log::LogScope::Runtime,
    );
    record.kind = "runtime.model_request".into();
    record.message = "requesting model".into();
    record.session_id = Some("s_log".into());

    let line = OutputLine::log(record).to_json_line();
    let json: serde_json::Value = serde_json::from_str(&line).unwrap();

    assert_eq!(json["type"], "log");
    assert_eq!(json["record"]["kind"], "runtime.model_request");
    assert_eq!(json["record"]["session_id"], "s_log");
}

#[test]
fn json_session_serializes() {
    let line = OutputLine::session_started(
        "abc123".into(),
        "strong".into(),
        "claude-sonnet-4-6".into(),
        None,
    );
    let json = line.to_json_line();
    assert!(json.contains("\"type\":\"session\""));
    assert!(json.contains("\"event\":\"started\""));
    assert!(json.contains("\"tier\":\"strong\""));
    assert!(json.contains("\"model\":\"claude-sonnet-4-6\""));
}

#[test]
fn all_json_types_have_type_field() {
    let lines = vec![
        OutputLine::thinking(100, None).to_json_line(),
        OutputLine::text_delta("hello".into()).to_json_line(),
        OutputLine::tool_call("t".into(), "id".into(), "s".into(), serde_json::Value::Null)
            .to_json_line(),
        OutputLine::tool_result("id".into(), "ok".into(), "s".into(), None, false).to_json_line(),
        OutputLine::permission_ask("pr".into(), "t".into(), "read".into(), "s".into())
            .to_json_line(),
        OutputLine::permission_decision("pr".into(), "t".into(), "allow".into(), "posture".into())
            .to_json_line(),
        OutputLine::error("s".into(), "k".into(), "m".into(), None).to_json_line(),
        OutputLine::session_started("id".into(), "t".into(), "m".into(), None).to_json_line(),
    ];
    for line in &lines {
        assert!(line.contains("\"type\":\""), "missing type field: {line}");
    }
}

#[test]
fn event_brief_renders_permission_requested() {
    let event = kuku::event::StoredEvent {
        id: 5,
        payload: kuku::event::EventPayload::PermissionRequested {
            turn: 1,
            ts: "t".to_string(),
            tool_call_id: "toolu_cmd".to_string(),
            tool: "run_command".to_string(),
            risk: "execute".to_string(),
            summary: "run tests".to_string(),
            candidate: "cargo test".to_string(),
            source: "default_ask".to_string(),
        },
    };

    let line = render_event_brief(&event, 1);

    assert!(line.contains("permission.requested"));
    assert!(line.contains("request  run_command  execute  cargo test"));
    assert!(line.contains("source=default_ask"));
}

#[test]
fn event_filter_excludes_main_facts_from_non_main_conversation() {
    let events = vec![
        kuku::event::StoredEvent {
            id: 1,
            payload: kuku::event::EventPayload::SessionCreated {
                ts: "t".into(),
                schema_version: 2,
                session_id: "s_filter".into(),
                created_at: "t".into(),
                kuku_version: "test".into(),
            },
        },
        kuku::event::StoredEvent {
            id: 2,
            payload: kuku::event::EventPayload::MessageUser {
                ts: "t".into(),
                conversation: "review".into(),
                turn: 1,
                text: "review message".into(),
                from: None,
                via_tool_call_id: None,
            },
        },
        kuku::event::StoredEvent {
            id: 3,
            payload: kuku::event::EventPayload::ModelResponse {
                turn: 1,
                ts: "t".into(),
                request_id: "req_main".into(),
                text: "main response".into(),
                thinking: None,
                input_tokens_total: None,
            },
        },
    ];

    let review = filter_events_for_conversation(&events, "review");
    let review_ids: Vec<u64> = review.iter().map(|event| event.id).collect();
    let main = filter_events_for_conversation(&events, "main");
    let main_ids: Vec<u64> = main.iter().map(|event| event.id).collect();

    assert_eq!(review_ids, vec![1, 2]);
    assert_eq!(main_ids, vec![1, 3]);
}

#[test]
fn event_brief_renders_conversation_address_for_conversation_opened() {
    let event = kuku::event::StoredEvent {
        id: 2,
        payload: kuku::event::EventPayload::Unknown(serde_json::json!({
            "id": 2,
            "ts": "2026-06-09T00:00:01Z",
            "kind": "conversation.opened",
            "conversation": "session://s_001/conversations/c_main"
        })),
    };

    let line = render_event_brief(&event, 1);

    assert!(line.contains("conversation.opened"));
    assert!(line.contains("session://s_001/conversations/c_main"));
}

#[test]
fn event_brief_renders_terminal_turn_states() {
    let cancelled = kuku::event::StoredEvent {
        id: 11,
        payload: kuku::event::EventPayload::Unknown(serde_json::json!({
            "id": 11,
            "ts": "2026-06-09T00:00:10Z",
            "kind": "turn.cancelled",
            "conversation": "session://s_001/conversations/c_main",
            "turn": 2,
            "reason": "user_cancelled"
        })),
    };
    let interrupted = kuku::event::StoredEvent {
        id: 12,
        payload: kuku::event::EventPayload::Unknown(serde_json::json!({
            "id": 12,
            "ts": "2026-06-09T00:00:11Z",
            "kind": "turn.interrupted",
            "conversation": "session://s_001/conversations/c_main",
            "turn": 3,
            "reason": "approval_required"
        })),
    };

    let cancelled_line = render_event_brief(&cancelled, 1);
    let interrupted_line = render_event_brief(&interrupted, 1);

    assert!(cancelled_line.contains("turn.cancelled"));
    assert!(cancelled_line.contains("turn=2"));
    assert!(cancelled_line.contains("user_cancelled"));
    assert!(interrupted_line.contains("turn.interrupted"));
    assert!(interrupted_line.contains("turn=3"));
    assert!(interrupted_line.contains("approval_required"));
}
