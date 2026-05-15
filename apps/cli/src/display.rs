/// Print a text delta to stdout (no newline).
pub fn text_delta(text: &str) {
    print!("{text}");
}

/// Format a tool call line.
pub fn tool_call(verbose: bool, tool: &str, summary: &str, extra: &str) {
    if verbose && !extra.is_empty() {
        println!("\n[tool] {tool} {summary}  {extra}");
    } else {
        println!("\n[tool] {tool} {summary}");
    }
}

/// Format a tool result line.
pub fn tool_result(verbose: bool, summary: &str, extra: &str) {
    if verbose && !extra.is_empty() {
        println!("       \u{2192} {summary}  {extra}");
    } else {
        println!("       \u{2192} {summary}");
    }
}

/// Permission prompt.
pub fn permission_prompt(tool: &str, summary: &str) -> String {
    format!("[ask] {tool} {summary} \u{2014} (y/n/session/project)?")
}

/// Session separator line.
pub fn session_separator(session_id: &str) {
    println!("\n\u{2500}\u{2500} session: {session_id} \u{2500}\u{2500}");
}

/// Print an event line (for show events).
pub fn event_line(event_brief: &str) {
    println!("{event_brief}");
}
