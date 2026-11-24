use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

pub use crate::test_process::isolate_test;

const TOOL_OUTPUT: &str = "fixture-tool-output-42";

/// Read exact committed journal bytes through an existing in-process writer.
pub fn read_active_journal(path: &Path) -> crate::Result<Vec<u8>> {
    crate::event::EventStore::read_active_journal_for_test(path)
}

struct RuntimeBoundaryPauseState {
    events_path: PathBuf,
    reached: Barrier,
    release: Barrier,
}

static RUNTIME_BOUNDARY_PAUSE: OnceLock<Mutex<Option<Arc<RuntimeBoundaryPauseState>>>> =
    OnceLock::new();

/// Coordinate a test-only pause after a runtime turn applies its configuration boundary.
pub struct RuntimeBoundaryPause {
    state: Arc<RuntimeBoundaryPauseState>,
}

impl RuntimeBoundaryPause {
    /// Pause the next runtime boundary for the specified session event store.
    pub fn install(events_path: &Path) -> Self {
        let state = Arc::new(RuntimeBoundaryPauseState {
            events_path: events_path.to_path_buf(),
            reached: Barrier::new(2),
            release: Barrier::new(2),
        });
        let slot = RUNTIME_BOUNDARY_PAUSE.get_or_init(|| Mutex::new(None));
        let mut current = slot.lock().unwrap();
        assert!(
            current.is_none(),
            "runtime boundary pause already installed"
        );
        *current = Some(state.clone());
        Self { state }
    }

    /// Wait for the worker to apply its runtime configuration.
    pub fn wait_until_reached(&self) {
        self.state.reached.wait();
    }

    /// Allow the worker to prepare its context snapshot.
    pub fn release(&self) {
        self.state.release.wait();
    }
}

impl Drop for RuntimeBoundaryPause {
    fn drop(&mut self) {
        if let Some(slot) = RUNTIME_BOUNDARY_PAUSE.get() {
            let mut current = slot.lock().unwrap();
            if current
                .as_ref()
                .is_some_and(|state| Arc::ptr_eq(state, &self.state))
            {
                *current = None;
            }
        }
    }
}

pub(crate) fn wait_after_runtime_boundary(events_path: &Path) {
    let state = RUNTIME_BOUNDARY_PAUSE.get().and_then(|slot| {
        let mut current = slot.lock().unwrap();
        if current
            .as_ref()
            .is_some_and(|state| state.events_path == events_path)
        {
            current.take()
        } else {
            None
        }
    });
    if let Some(state) = state {
        state.reached.wait();
        state.release.wait();
    }
}

/// Run the native command fixture until its host creates the release file.
pub fn run_gated_tool(timeout: Duration) -> io::Result<()> {
    let mut attempts = OpenOptions::new()
        .create(true)
        .append(true)
        .open("tool-attempts.txt")?;
    writeln!(attempts, "{}", env::current_dir()?.display())?;
    attempts.flush()?;
    let deadline = Instant::now() + timeout;
    while !Path::new("release-tool").exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "tool release deadline expired",
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }
    fs::write("tool-result.txt", TOOL_OUTPUT)?;
    writeln!(io::stdout().lock(), "{TOOL_OUTPUT}")
}

pub fn anthropic_sse_response(msg: Value) -> String {
    let id = msg
        .get("id")
        .cloned()
        .unwrap_or(Value::String("msg_1".into()));
    let model = msg
        .get("model")
        .cloned()
        .unwrap_or(Value::String("test-model".into()));
    let stop_reason = msg
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("end_turn");
    let usage = msg
        .get("usage")
        .cloned()
        .unwrap_or(serde_json::json!({"input_tokens": 0, "output_tokens": 0}));
    let content = msg
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut sse = String::new();

    sse.push_str(&format!(
        "event: message_start\ndata: {}\n\n",
        serde_json::json!({"type":"message_start","message":{"id":id,"model":model,"content":[],"usage":usage}})
    ));

    for (i, block) in content.iter().enumerate() {
        let btype = block.get("type").and_then(Value::as_str).unwrap_or("text");
        if btype == "text" {
            let text = block.get("text").and_then(Value::as_str).unwrap_or("");
            sse.push_str(&format!(
                "event: content_block_start\ndata: {}\n\n",
                serde_json::json!({"type":"content_block_start","index":i,"content_block":{"type":"text","text":""}})
            ));
            if !text.is_empty() {
                sse.push_str(&format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    serde_json::json!({"type":"content_block_delta","index":i,"delta":{"type":"text_delta","text":text}})
                ));
            }
            sse.push_str(&format!(
                "event: content_block_stop\ndata: {}\n\n",
                serde_json::json!({"type":"content_block_stop","index":i})
            ));
        } else if btype == "thinking" {
            let thinking = block.get("thinking").and_then(Value::as_str).unwrap_or("");
            sse.push_str(&format!(
                "event: content_block_start\ndata: {}\n\n",
                serde_json::json!({"type":"content_block_start","index":i,"content_block":{"type":"thinking","thinking":""}})
            ));
            if !thinking.is_empty() {
                sse.push_str(&format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    serde_json::json!({"type":"content_block_delta","index":i,"delta":{"type":"thinking_delta","thinking":thinking}})
                ));
            }
            if let Some(signature) = block.get("signature").and_then(Value::as_str) {
                sse.push_str(&format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    serde_json::json!({"type":"content_block_delta","index":i,"delta":{"type":"signature_delta","signature":signature}})
                ));
            }
            sse.push_str(&format!(
                "event: content_block_stop\ndata: {}\n\n",
                serde_json::json!({"type":"content_block_stop","index":i})
            ));
        } else if btype == "tool_use" {
            let tc_id = block.get("id").and_then(Value::as_str).unwrap_or("tc_1");
            let name = block.get("name").and_then(Value::as_str).unwrap_or("");
            let input = block.get("input").cloned().unwrap_or(serde_json::json!({}));
            sse.push_str(&format!(
                "event: content_block_start\ndata: {}\n\n",
                serde_json::json!({"type":"content_block_start","index":i,"content_block":{"type":"tool_use","id":tc_id,"name":name,"input":{}}})
            ));
            let args_str = serde_json::to_string(&input).unwrap_or_default();
            if !args_str.is_empty() && args_str != "{}" {
                sse.push_str(&format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    serde_json::json!({"type":"content_block_delta","index":i,"delta":{"type":"input_json_delta","partial_json":args_str}})
                ));
            }
            sse.push_str(&format!(
                "event: content_block_stop\ndata: {}\n\n",
                serde_json::json!({"type":"content_block_stop","index":i})
            ));
        }
    }

    sse.push_str(&format!(
        "event: message_delta\ndata: {}\n\n",
        serde_json::json!({"type":"message_delta","delta":{"stop_reason":stop_reason},"usage":{"output_tokens":usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0)}})
    ));

    sse.push_str("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");

    sse
}
