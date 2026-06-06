use std::time::Duration;

use kuku_cli::display::render_event_brief;
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
    );
    let json = line.to_json_line();
    assert!(json.contains("\"type\":\"tool_call\""));
    assert!(json.contains("\"tool_call_id\":\"tc_01\""));
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
