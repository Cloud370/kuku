use kuku_terminal::display::{Display, OutputLine, Verbosity};

#[test]
fn thinking_concise_hides_text() {
    let d = Display::new(Verbosity::Concise);
    assert!(d.thinking_text("secret reasoning").is_none());
    assert!(d.thinking_start(3200).contains("thinking"));
    assert!(d.thinking_end(3200).contains("3.2k"));
}

#[test]
fn thinking_verbose_shows_text() {
    let d = Display::new(Verbosity::Verbose);
    assert_eq!(
        d.thinking_text("secret reasoning"),
        Some("secret reasoning".into())
    );
}

#[test]
fn tool_call_concise_hides_id() {
    let d = Display::new(Verbosity::Concise);
    let line = d.tool_call("read_file", "src/main.rs", "tc_01");
    assert!(line.contains("read_file"));
    assert!(line.contains("src/main.rs"));
    assert!(!line.contains("tc_01"));
}

#[test]
fn tool_call_verbose_shows_id() {
    let d = Display::new(Verbosity::Verbose);
    let line = d.tool_call("read_file", "src/main.rs", "tc_01");
    assert!(line.contains("tc_01"));
}

#[test]
fn permission_ask_format() {
    let d = Display::new(Verbosity::Concise);
    let line = d.permission_ask("run_command", "cargo build");
    assert!(line.contains("?"));
    assert!(line.contains("run_command"));
    assert!(line.contains("(y/n)?"));
}

#[test]
fn error_format() {
    let d = Display::new(Verbosity::Concise);
    let line = d.error("provider", "auth", "invalid API key");
    assert!(line.contains("!!"));
    assert!(line.contains("provider"));
    assert!(line.contains("auth"));
}

#[test]
fn session_start_shows_model_and_effort() {
    let d = Display::new(Verbosity::Concise);
    let line = d.session_start("abc123", "claude-opus", "xhigh");
    assert!(line.contains("abc123"));
    assert!(line.contains("claude-opus"));
    assert!(line.contains("xhigh"));
}

#[test]
fn code_block_basic() {
    let d = Display::new(Verbosity::Concise);
    assert!(d.code_block_open(Some("rust")).contains("rust"));
}

#[test]
fn table_row_pads_cells() {
    let d = Display::new(Verbosity::Concise);
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
    assert!(json.contains("\"tokens\":1200"));
    assert!(!json.contains("\"text\""));
}

#[test]
fn json_thinking_verbose_serializes_text() {
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
fn json_session_serializes() {
    let line = OutputLine::session_started("abc123".into(), "claude-opus".into(), "xhigh".into());
    let json = line.to_json_line();
    assert!(json.contains("\"type\":\"session\""));
    assert!(json.contains("\"event\":\"started\""));
    assert!(json.contains("\"model\":\"claude-opus\""));
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
        OutputLine::session_started("id".into(), "m".into(), "e".into()).to_json_line(),
    ];
    for line in &lines {
        assert!(line.contains("\"type\":\""), "missing type field: {line}");
    }
}
