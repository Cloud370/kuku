mod event;
mod json;
pub mod text;
pub mod util;

pub use event::{derive_final_output, render_event_brief};
pub use json::{OutputLine, SessionSummary};
pub use text::{Display, RenderMode};

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::text::Display;

    #[test]
    fn session_start_contains_tier_and_model() {
        let d = Display::new(false, "medium");
        let line = d.session_start("s_001", "strong", "claude-sonnet-4-6");
        assert!(line.contains("strong"), "should contain tier");
        assert!(line.contains("claude-sonnet-4-6"), "should contain model");
        assert!(line.contains("s_001"), "should contain session id");
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
        assert!(line.contains("2 turns"), "should show turn count");
        assert!(line.contains("18s"), "should show duration");
    }

    #[test]
    fn session_completed_shows_cache_metrics() {
        let d = Display::new(false, "medium");
        let line = d.session_completed("s_001", 1, 1000, 500, 800, 200, Duration::from_secs(5));
        assert!(
            line.contains("cache read 800"),
            "should show cache read: {line}"
        );
        assert!(
            line.contains("write 200"),
            "should show cache creation: {line}"
        );
    }

    #[test]
    fn thinking_start_has_no_duration() {
        let d = Display::new(false, "high");
        let line = d.thinking_start();
        assert_eq!(line, "\u{250c}\u{2500} thinking [high] \u{2500}");
    }

    #[test]
    fn thinking_end_shows_duration() {
        let mut d = Display::new(false, "medium");
        let line = d.thinking_end(Duration::from_millis(12500));
        assert!(line.contains("12.5s"), "should show duration");
    }

    #[test]
    fn thinking_text_shown_when_enabled() {
        let d = Display::new(true, "medium");
        assert_eq!(d.thinking_text("reasoning"), Some("reasoning".to_string()));
    }

    #[test]
    fn thinking_text_hidden_when_disabled() {
        let d = Display::new(false, "medium");
        assert_eq!(d.thinking_text("reasoning"), None);
    }

    #[test]
    fn context_previous_shows_tokens() {
        let d = Display::new(false, "medium");
        let line = d.context_previous(35000);
        assert!(line.contains("35.0k"), "should show token count: {line}");
        assert!(
            line.contains("previous"),
            "should indicate previous context"
        );
    }

    #[test]
    fn tool_result_shows_status_and_summary() {
        let d = Display::new(false, "medium");
        let line = d.tool_result("ok", "120 lines", "tc_01");
        assert!(line.contains("ok"), "should contain status");
        assert!(line.contains("120 lines"), "should contain summary");
    }

    #[test]
    fn permission_decision_shows_decision_and_tool() {
        let d = Display::new(false, "medium");
        let line = d.permission_decision("allow", "run_command", "user");
        assert!(line.contains("allow"), "should contain decision");
        assert!(line.contains("run_command"), "should contain tool");
        assert!(line.contains("user"), "should contain rule");
    }

    #[test]
    fn session_interrupted_shows_session_and_turns() {
        let d = Display::new(false, "medium");
        let line = d.session_interrupted("s_001", 3);
        assert!(line.contains("s_001"), "should contain session id");
        assert!(line.contains("3 turns"), "should contain turn count");
        assert!(line.contains("interrupted"), "should indicate interruption");
    }

    #[test]
    fn raw_thinking_start() {
        let d = Display::new_raw(false, "high");
        assert_eq!(d.thinking_start(), "thinking [high]");
    }

    #[test]
    fn tool_running_pretty_and_raw() {
        let pretty = Display::new(false, "medium");
        assert_eq!(pretty.tool_running(), "  ...");
        let raw = Display::new_raw(false, "medium");
        assert_eq!(raw.tool_running(), "run ...");
    }

    #[test]
    fn raw_tool_call() {
        let d = Display::new_raw(false, "medium");
        let line = d.tool_call("find_files", "path: \".\"", "tc_01");
        assert_eq!(line, "call find_files \u{b7} path: \".\"");
    }

    #[test]
    fn raw_tool_result() {
        let d = Display::new_raw(false, "medium");
        let line = d.tool_result("ok", "3 files", "tc_01");
        assert_eq!(line, "result ok \u{b7} 3 files");
    }

    #[test]
    fn pretty_tool_call_with_display_summary() {
        let d = Display::new(false, "medium");
        let line = d.tool_call("find_files", "path: \".\"", "tc_01");
        assert_eq!(line, "\u{2699} find_files \u{b7} path: \".\"");
    }

    #[test]
    fn thinking_line_adds_prefix() {
        let mut d = Display::new(true, "high");
        let out = d.thinking_line("hello world").unwrap();
        assert_eq!(out, "\u{2502} hello world");
    }

    #[test]
    fn thinking_line_tracks_newlines() {
        let mut d = Display::new(true, "high");
        let out = d.thinking_line("line1\nline2\n").unwrap();
        assert_eq!(out, "\u{2502} line1\n\u{2502} line2\n");
    }

    #[test]
    fn thinking_line_raw_mode() {
        let mut d = Display::new_raw(true, "medium");
        let out = d.thinking_line("reasoning\n").unwrap();
        assert_eq!(out, "thinking reasoning\n");
    }

    #[test]
    fn thinking_end_resets_line_start() {
        let mut d = Display::new(true, "high");
        d.thinking_line("some text").unwrap();
        d.thinking_end(Duration::from_millis(1000));
        let out = d.thinking_line("new block").unwrap();
        assert!(
            out.starts_with("\u{2502} "),
            "should reset line_start: {out}"
        );
    }

    #[test]
    fn thinking_line_hidden_when_disabled() {
        let mut d = Display::new(false, "high");
        assert_eq!(d.thinking_line("secret"), None);
    }

    #[test]
    fn thinking_line_empty_string() {
        let mut d = Display::new(true, "high");
        let out = d.thinking_line("").unwrap();
        assert_eq!(out, "", "empty input should produce empty output");
    }

    #[test]
    fn thinking_line_consecutive_newlines() {
        let mut d = Display::new(true, "high");
        let out = d.thinking_line("\n\n").unwrap();
        assert_eq!(
            out, "\u{2502} \n\u{2502} \n",
            "each empty line gets prefix with space"
        );
    }

    #[test]
    fn raw_thinking_end_format() {
        let mut d = Display::new_raw(false, "medium");
        let line = d.thinking_end(Duration::from_millis(3200));
        assert!(line.contains("thinking end"), "raw end label: {line}");
        assert!(line.contains("3.2s"), "raw end duration: {line}");
        assert!(
            !line.contains("\u{2514}"),
            "no box char in raw mode: {line}"
        );
    }

    #[test]
    fn thinking_line_cross_chunk_continuity() {
        let mut d = Display::new(true, "high");
        let out1 = d.thinking_line("hello ").unwrap();
        assert_eq!(out1, "\u{2502} hello ");
        let out2 = d.thinking_line("world\n").unwrap();
        assert_eq!(out2, "world\n", "no prefix mid-line");
        let out3 = d.thinking_line("next").unwrap();
        assert_eq!(out3, "\u{2502} next", "prefix after newline");
    }

    #[test]
    fn raw_permission_ask_format() {
        let d = Display::new_raw(false, "medium");
        let line = d.permission_ask("run_command", "cargo test");
        assert_eq!(line, "ask run_command \u{b7} cargo test");
        assert!(!line.contains("(y/n)?"), "no y/n in raw mode");
    }

    #[test]
    fn raw_permission_decision_format() {
        let d = Display::new_raw(false, "medium");
        let line = d.permission_decision("allow", "run_command", "posture");
        assert_eq!(line, "allow run_command \u{b7} posture");
    }

    #[test]
    fn raw_error_format() {
        let d = Display::new_raw(false, "medium");
        let line = d.error("provider", "auth", "invalid key");
        assert_eq!(line, "error provider \u{b7} auth \u{b7} invalid key");
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(super::util::truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_length() {
        assert_eq!(super::util::truncate("hello", 5), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let result = super::util::truncate("hello world", 6);
        assert_eq!(result, "hello\u{2026}");
        assert!(result.chars().count() <= 6);
    }

    #[test]
    fn truncate_max_one() {
        let result = super::util::truncate("hello", 1);
        assert_eq!(result, "\u{2026}");
    }
}
