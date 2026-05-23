use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

/// Read events.jsonl from the start and return the text of the first `user.input` event.
pub(crate) fn scan_first_user_input(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            let is_user_input = value.get("type").and_then(|t| t.as_str()) == Some("user.input");
            if is_user_input {
                return value
                    .get("text")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string());
            }
        }
    }
    None
}

/// Read the first line of events.jsonl as JSON and return the `created_at` field.
pub(crate) fn scan_session_meta(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            let is_session_meta =
                value.get("type").and_then(|t| t.as_str()) == Some("session.meta");
            if is_session_meta {
                return value
                    .get("created_at")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string());
            }
        }
        break;
    }
    None
}

/// Count occurrences of `"type":"turn.start"` by string scan (no JSON parse).
pub(crate) fn scan_turn_count(path: &Path) -> u64 {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return 0,
    };
    let mut buf = Vec::with_capacity(65536);
    if file.read_to_end(&mut buf).is_err() {
        return 0;
    }
    let needle = b"\"type\":\"turn.start\"";
    let mut count = 0;
    let mut pos = 0;
    while let Some(idx) = buf[pos..].windows(needle.len()).position(|w| w == needle) {
        count += 1;
        pos += idx + needle.len();
    }
    count
}

/// Read the last complete JSON line from events.jsonl and return its `type` tag.
/// Seeks to end - 4096 bytes, reads forward to find the final line.
pub(crate) fn scan_last_event_type(path: &Path) -> Option<&'static str> {
    let mut file = File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();
    if file_len == 0 {
        return None;
    }
    let start = file_len.saturating_sub(4096);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;
    let last_line = buf.lines().rfind(|l| !l.trim().is_empty())?;
    let value: Value = serde_json::from_str(last_line).ok()?;
    match value.get("type").and_then(|t| t.as_str()) {
        Some("turn.end") => Some("turn.end"),
        Some("turn.start") => Some("turn.start"),
        Some("model.response") => Some("model.response"),
        Some("tool.result") => Some("tool.result"),
        _ => None,
    }
}
