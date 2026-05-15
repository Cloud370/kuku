/// Print a text delta to stdout (no newline).
pub fn text_delta(text: &str) {
    print!("{text}");
}

/// Print a thinking section start.
pub fn thinking_start() {
    println!("── thinking ──");
}

/// Print a thinking delta to stdout (no newline).
pub fn thinking_delta(text: &str) {
    print!("{text}");
}

/// Print a thinking section end.
pub fn thinking_end() {
    println!("\n── /thinking ──");
}

/// Print a response section start.
pub fn response_start() {
    println!("── response ──");
}

/// Format a tool call line.
pub fn tool_call(tool: &str, summary: &str) {
    println!("\n[tool] {tool} {summary}");
}

/// Format a tool result line.
pub fn tool_result(summary: &str) {
    println!("       \u{2192} {summary}");
}

/// Permission prompt.
pub fn permission_prompt(tool: &str, summary: &str) -> String {
    format!("[ask] {tool} {summary} \u{2014} (y/n/session/project)?")
}

/// Session start line.
pub fn session_start(session_id: &str) {
    println!("session: {session_id}");
}

/// Interrupted line.
pub fn interrupted(session_id: &str) {
    eprintln!("\ninterrupted. session: {session_id}");
}

/// Print an event line (for show events).
#[allow(dead_code)]
pub fn event_line(event_brief: &str) {
    println!("{event_brief}");
}
