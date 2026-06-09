use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

const LAST_EVENT_SCAN_CHUNK_BYTES: u64 = 4096;

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
            let kind = event_kind(&value);
            let is_user_input = matches!(kind, Some("user.input") | Some("message.user"));
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
            let kind = event_kind(&value);
            let is_session_meta = matches!(kind, Some("session.created") | Some("session.meta"));
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

/// Count occurrences of turn start events by string scan (no JSON parse).
/// Safe because serde serializes the same struct definition deterministically.
pub(crate) fn scan_turn_count(path: &Path) -> u64 {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return 0,
    };
    let mut buf = Vec::with_capacity(65536);
    if file.read_to_end(&mut buf).is_err() {
        return 0;
    }
    count_occurrences(&buf, b"\"kind\":\"turn.started\"")
        + count_occurrences(&buf, b"\"type\":\"turn.started\"")
        + count_occurrences(&buf, b"\"kind\":\"turn.start\"")
        + count_occurrences(&buf, b"\"type\":\"turn.start\"")
}

/// Read the last complete JSON line from events.jsonl and return its `kind` tag.
/// Scans backward in bounded chunks until it finds the final complete line.
pub(crate) fn scan_last_event_type(path: &Path) -> Option<&'static str> {
    let mut file = File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();
    if file_len == 0 {
        return None;
    }
    let mut start = file_len;
    let mut buffer = Vec::new();

    while start > 0 {
        let chunk_start = start.saturating_sub(LAST_EVENT_SCAN_CHUNK_BYTES);
        let chunk_len = (start - chunk_start) as usize;
        let mut chunk = vec![0_u8; chunk_len];
        file.seek(SeekFrom::Start(chunk_start)).ok()?;
        file.read_exact(&mut chunk).ok()?;
        chunk.extend_from_slice(&buffer);
        buffer = chunk;

        let complete = if chunk_start == 0 {
            buffer.as_slice()
        } else if let Some(index) = buffer.iter().position(|byte| *byte == b'\n') {
            &buffer[index + 1..]
        } else {
            start = chunk_start;
            continue;
        };

        if let Some(last_line) = complete
            .split(|byte| *byte == b'\n')
            .rev()
            .find(|line| !line.is_empty() && !line.iter().all(|byte| byte.is_ascii_whitespace()))
        {
            let value: Value = serde_json::from_slice(last_line).ok()?;
            let kind = event_kind(&value);
            return match kind {
                Some("turn.completed") => Some("turn.completed"),
                Some("turn.end") => Some("turn.completed"),
                Some("turn.started") => Some("turn.started"),
                Some("turn.start") => Some("turn.started"),
                Some("model.response") => Some("model.response"),
                Some("tool.result") => Some("tool.result"),
                _ => None,
            };
        }

        start = chunk_start;
    }

    None
}

fn event_kind(value: &Value) -> Option<&str> {
    value
        .get("kind")
        .or_else(|| value.get("type"))
        .and_then(|t| t.as_str())
}

fn count_occurrences(haystack: &[u8], needle: &[u8]) -> u64 {
    let mut count = 0;
    let mut pos = 0;
    while let Some(idx) = haystack[pos..]
        .windows(needle.len())
        .position(|w| w == needle)
    {
        count += 1;
        pos += idx + needle.len();
    }
    count
}

#[cfg(test)]
mod tests {
    use super::scan_last_event_type;

    #[test]
    fn scan_last_event_type_reads_large_final_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let large = "x".repeat(5000);
        std::fs::write(
            &path,
            format!(
                "{{\"id\":1,\"kind\":\"turn.started\",\"conversation\":\"session://s/conversations/c_main\",\"turn\":1,\"ts\":\"2026-01-01T00:00:00Z\"}}\n{{\"id\":2,\"kind\":\"turn.completed\",\"conversation\":\"session://s/conversations/c_main\",\"turn\":1,\"ts\":\"2026-01-01T00:00:01Z\",\"summary\":\"{}\"}}\n",
                large
            ),
        )
        .unwrap();

        assert_eq!(Some("turn.completed"), scan_last_event_type(&path));
    }
}
