use std::io;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration};

use crate::tool::ToolResultEnvelope;

use super::common::{plural, require_brief};

const RUN_COMMAND_MAX_CHARS: usize = 80_000;
const RUN_COMMAND_TIMEOUT_CAP_SECONDS: u64 = 600;
const FLUSH_THRESHOLD_MS: u64 = 50;
const FLUSH_THRESHOLD_BYTES: usize = 4096;

#[derive(Debug, Clone)]
pub(crate) enum CommandEvent {
    Stdout(String),
    Stderr(String),
}

struct CommandRequest {
    command: String,
    timeout_seconds: u64,
    _brief: String,
}

pub(crate) async fn run_command(
    args: &Value,
    workspace: &Path,
    event_tx: Option<mpsc::Sender<CommandEvent>>,
    cancel: Option<Arc<Notify>>,
) -> ToolResultEnvelope {
    let request = match run_command_request(args) {
        Ok(request) => request,
        Err(result) => return result,
    };
    if let Some(reason) = blocked_command_reason(&request.command) {
        return ToolResultEnvelope::blocked(
            format!("blocked by command hard guard: {reason}"),
            format!("blocked by command hard guard: {reason}"),
        );
    }
    let workspace = match workspace.canonicalize() {
        Ok(path) => path,
        Err(_) => {
            return ToolResultEnvelope::error(
                "failed: workspace not found",
                "workspace path does not exist",
            )
        }
    };

    let started = Instant::now();
    let mut command = shell_command(&request.command);
    command
        .current_dir(&workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return ToolResultEnvelope::error(
                format!("failed: command could not start: {error}"),
                format!("command could not start: {error}"),
            )
        }
    };
    let stdout_task = child
        .stdout
        .take()
        .map(|p| read_pipe_streaming(p, event_tx.clone(), true));
    let stderr_task = child
        .stderr
        .take()
        .map(|p| read_pipe_streaming(p, event_tx, false));

    let cancel_fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> = match &cancel
    {
        Some(c) => {
            let c = c.clone();
            Box::pin(async move { c.notified().await })
        }
        None => Box::pin(std::future::pending()),
    };

    let status = tokio::select! {
        status = child.wait() => match status {
            Ok(status) => status,
            Err(error) => {
                return ToolResultEnvelope::error(
                    format!("failed: command wait error: {error}"),
                    format!("command wait error: {error}"),
                )
            }
        },
        _ = sleep(Duration::from_secs(request.timeout_seconds)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let duration_ms = started.elapsed().as_millis() as u64;
            let stdout = collect_pipe(stdout_task).await.unwrap_or_default();
            let stderr = collect_pipe(stderr_task).await.unwrap_or_default();
            return render_command_timeout_result(
                &request.command,
                request.timeout_seconds,
                duration_ms,
                &stdout,
                &stderr,
            );
        }
        _ = cancel_fut => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let duration_ms = started.elapsed().as_millis() as u64;
            let stdout = collect_pipe(stdout_task).await.unwrap_or_default();
            let stderr = collect_pipe(stderr_task).await.unwrap_or_default();
            return render_command_cancelled_result(
                &request.command,
                duration_ms,
                &stdout,
                &stderr,
            );
        }
    };

    let duration_ms = started.elapsed().as_millis() as u64;
    let stdout = match collect_pipe(stdout_task).await {
        Ok(bytes) => bytes,
        Err(error) => {
            return ToolResultEnvelope::error(
                format!("failed: stdout capture error: {error}"),
                format!("stdout capture error: {error}"),
            )
        }
    };
    let stderr = match collect_pipe(stderr_task).await {
        Ok(bytes) => bytes,
        Err(error) => {
            return ToolResultEnvelope::error(
                format!("failed: stderr capture error: {error}"),
                format!("stderr capture error: {error}"),
            )
        }
    };

    render_command_result(&request.command, status, stdout, stderr, duration_ms)
}

fn run_command_request(args: &Value) -> Result<CommandRequest, ToolResultEnvelope> {
    let Some(command) = args.get("command").and_then(Value::as_str) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing command",
            "run_command requires command",
        ));
    };
    if command.trim().is_empty() {
        return Err(ToolResultEnvelope::error(
            "failed: command is empty",
            "command must not be empty",
        ));
    }
    let Some(timeout_seconds) = args.get("timeout").and_then(Value::as_u64) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing timeout",
            "run_command requires timeout in seconds",
        ));
    };
    if timeout_seconds == 0 {
        return Err(ToolResultEnvelope::error(
            "failed: timeout must be >= 1",
            "timeout must be >= 1",
        ));
    }
    if timeout_seconds > RUN_COMMAND_TIMEOUT_CAP_SECONDS {
        return Err(ToolResultEnvelope::error(
            format!("failed: timeout must be <= {RUN_COMMAND_TIMEOUT_CAP_SECONDS}"),
            format!("timeout must be <= {RUN_COMMAND_TIMEOUT_CAP_SECONDS}"),
        ));
    }

    let brief = require_brief("run_command", args)?;

    Ok(CommandRequest {
        command: command.to_string(),
        timeout_seconds,
        _brief: brief,
    })
}

fn blocked_command_reason(command: &str) -> Option<&'static str> {
    let normalized = command
        .to_ascii_lowercase()
        .replace("&&", "\x00")
        .replace("||", "\n")
        .replace(['&', ';', '\x00'], "\n");
    for segment in normalized_command_segments(&normalized) {
        for (prefix, reason) in [
            ("git reset --hard", "git reset --hard discards local work"),
            ("git clean -f", "git clean deletes untracked files"),
            ("rm -rf", "recursive force delete is destructive"),
            ("rm -fr", "recursive force delete is destructive"),
            ("rmdir /s /q", "recursive force delete is destructive"),
            ("del /s", "recursive delete is destructive"),
            (
                "remove-item -recurse -force",
                "recursive force delete is destructive",
            ),
            (
                "cargo publish",
                "publish affects external package registries",
            ),
            ("npm publish", "publish affects external package registries"),
            (
                "pnpm publish",
                "publish affects external package registries",
            ),
            (
                "yarn publish",
                "publish affects external package registries",
            ),
            ("bun publish", "publish affects external package registries"),
            ("npm run deploy", "deploy affects external systems"),
            ("pnpm deploy", "deploy affects external systems"),
            ("yarn deploy", "deploy affects external systems"),
            ("make deploy", "deploy affects external systems"),
            ("cargo release", "release affects external systems"),
        ] {
            if segment.starts_with(prefix) {
                return Some(reason);
            }
        }
    }
    None
}

fn normalized_command_segments(command: &str) -> Vec<String> {
    command
        .lines()
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.strip_prefix("sudo ").unwrap_or(segment).trim())
        .flat_map(unwrap_shell_wrapper)
        .collect()
}

fn unwrap_shell_wrapper(segment: &str) -> Vec<String> {
    for prefix in [
        "sh -c ",
        "bash -lc ",
        "zsh -lc ",
        "cmd /c ",
        "powershell -command ",
        "pwsh -command ",
    ] {
        if let Some(rest) = segment.strip_prefix(prefix) {
            let rest = rest.trim().trim_matches('"').trim_matches('\'');
            return rest
                .lines()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.strip_prefix("sudo ").unwrap_or(value).trim())
                .map(ToString::to_string)
                .collect();
        }
    }
    vec![segment.to_string()]
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut child = Command::new("cmd");
    child.arg("/C").arg(command);
    child
}

#[cfg(not(windows))]
fn shell_command(command: &str) -> Command {
    let mut child = Command::new("sh");
    child.arg("-c").arg(command);
    child
}

fn read_pipe_streaming<P>(
    pipe: P,
    tx: Option<mpsc::Sender<CommandEvent>>,
    is_stdout: bool,
) -> JoinHandle<io::Result<Vec<u8>>>
where
    P: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut pipe = pipe;
        let mut full_buf = Vec::new();
        let mut flush_buf = String::new();
        let mut buf = [0u8; 4096];

        loop {
            tokio::select! {
                result = pipe.read(&mut buf) => {
                    match result {
                        Ok(0) => break,
                        Ok(n) => {
                            full_buf.extend_from_slice(&buf[..n]);
                            flush_buf.push_str(&String::from_utf8_lossy(&buf[..n]));
                            if flush_buf.len() >= FLUSH_THRESHOLD_BYTES {
                                flush_send(&mut flush_buf, &tx, is_stdout).await;
                            }
                        }
                        Err(_) => break,
                    }
                }
                _ = sleep(Duration::from_millis(FLUSH_THRESHOLD_MS)), if !flush_buf.is_empty() => {
                    flush_send(&mut flush_buf, &tx, is_stdout).await;
                }
            }
        }
        if !flush_buf.is_empty() {
            flush_send(&mut flush_buf, &tx, is_stdout).await;
        }
        Ok(full_buf)
    })
}

async fn flush_send(buf: &mut String, tx: &Option<mpsc::Sender<CommandEvent>>, is_stdout: bool) {
    if let Some(tx) = tx {
        let text = std::mem::take(buf);
        let event = if is_stdout {
            CommandEvent::Stdout(text)
        } else {
            CommandEvent::Stderr(text)
        };
        let _ = tx.send(event).await;
    } else {
        buf.clear();
    }
}

async fn collect_pipe(task: Option<JoinHandle<io::Result<Vec<u8>>>>) -> io::Result<Vec<u8>> {
    match task {
        Some(task) => task.await.map_err(io::Error::other)?,
        None => Ok(Vec::new()),
    }
}

fn render_command_result(
    command: &str,
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    duration_ms: u64,
) -> ToolResultEnvelope {
    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    let stderr = String::from_utf8_lossy(&stderr).into_owned();
    let stdout_lines = line_count(&stdout);
    let stderr_lines = line_count(&stderr);
    let exit_code = status.code();
    let rendered = render_command_output(&stdout, &stderr);
    let (model_content, truncated) = truncate_text(
        rendered,
        RUN_COMMAND_MAX_CHARS,
        "(Command output truncated. Run a narrower command or inspect files with dedicated tools.)",
    );
    let mut summary = format!(
        "command exited {} in {}ms, stdout {} line{}, stderr {} line{}",
        exit_code.map_or_else(|| "unknown".to_string(), |code| code.to_string()),
        duration_ms,
        stdout_lines,
        plural(stdout_lines),
        stderr_lines,
        plural(stderr_lines),
    );
    if truncated {
        summary.push_str(", output truncated");
    }
    let structured = serde_json::json!({
        "kind": "command_result",
        "command": command,
        "exit_code": exit_code,
        "duration_ms": duration_ms,
        "stdout_lines": stdout_lines,
        "stderr_lines": stderr_lines,
        "timed_out": false,
        "line_numbered": false,
    });

    if truncated {
        ToolResultEnvelope::ok_truncated(summary, model_content, structured)
    } else {
        ToolResultEnvelope::ok(summary, model_content, structured)
    }
}

fn render_command_timeout_result(
    command: &str,
    timeout_seconds: u64,
    duration_ms: u64,
    stdout: &[u8],
    stderr: &[u8],
) -> ToolResultEnvelope {
    let stdout = String::from_utf8_lossy(stdout).into_owned();
    let stderr = String::from_utf8_lossy(stderr).into_owned();
    let stdout_lines = line_count(&stdout);
    let stderr_lines = line_count(&stderr);
    let rendered = render_command_output(&stdout, &stderr);
    let model_content = if rendered == "(command produced no output)" {
        format!("command timed out after {timeout_seconds}s")
    } else {
        format!("command timed out after {timeout_seconds}s\n\n{rendered}")
    };
    let (model_content, truncated) = truncate_text(
        model_content,
        RUN_COMMAND_MAX_CHARS,
        "(Command output truncated. Run a narrower command or inspect files with dedicated tools.)",
    );
    let mut summary = format!(
        "command timed out after {timeout_seconds}s, stdout {stdout_lines} line{}, stderr {stderr_lines} line{}",
        plural(stdout_lines),
        plural(stderr_lines),
    );
    if truncated {
        summary.push_str(", output truncated");
    }
    ToolResultEnvelope {
        status: "error".to_string(),
        summary,
        model_content,
        truncated,
        structured: Some(serde_json::json!({
            "kind": "command_result",
            "command": command,
            "exit_code": null,
            "duration_ms": duration_ms,
            "stdout_lines": stdout_lines,
            "stderr_lines": stderr_lines,
            "timed_out": true,
            "line_numbered": false,
        })),
    }
}

fn render_command_cancelled_result(
    command: &str,
    duration_ms: u64,
    stdout: &[u8],
    stderr: &[u8],
) -> ToolResultEnvelope {
    let stdout = String::from_utf8_lossy(stdout).into_owned();
    let stderr = String::from_utf8_lossy(stderr).into_owned();
    let stdout_lines = line_count(&stdout);
    let stderr_lines = line_count(&stderr);
    let rendered = render_command_output(&stdout, &stderr);
    let model_content = if rendered == "(command produced no output)" {
        format!("command cancelled after {duration_ms}ms")
    } else {
        format!("command cancelled after {duration_ms}ms\n\n{rendered}")
    };
    let (model_content, truncated) = truncate_text(
        model_content,
        RUN_COMMAND_MAX_CHARS,
        "(Command output truncated. Run a narrower command or inspect files with dedicated tools.)",
    );
    let mut summary = format!(
        "command cancelled after {duration_ms}ms, stdout {stdout_lines} line{}, stderr {stderr_lines} line{}",
        plural(stdout_lines),
        plural(stderr_lines),
    );
    if truncated {
        summary.push_str(", output truncated");
    }
    ToolResultEnvelope {
        status: "cancelled".to_string(),
        summary,
        model_content,
        truncated,
        structured: Some(serde_json::json!({
            "kind": "command_result",
            "command": command,
            "exit_code": null,
            "duration_ms": duration_ms,
            "stdout_lines": stdout_lines,
            "stderr_lines": stderr_lines,
            "timed_out": false,
            "cancelled": true,
            "line_numbered": false,
        })),
    }
}

fn render_command_output(stdout: &str, stderr: &str) -> String {
    let mut sections = Vec::new();
    if !stdout.is_empty() {
        sections.push(format!(
            "stdout:\n{}",
            stdout.trim_end_matches(['\r', '\n'])
        ));
    }
    if !stderr.is_empty() {
        sections.push(format!(
            "stderr:\n{}",
            stderr.trim_end_matches(['\r', '\n'])
        ));
    }
    if sections.is_empty() {
        "(command produced no output)".to_string()
    } else {
        sections.join("\n\n")
    }
}

fn line_count(text: &str) -> usize {
    text.lines().count()
}

fn truncate_text(mut text: String, max_chars: usize, truncation_message: &str) -> (String, bool) {
    if text.len() <= max_chars {
        return (text, false);
    }
    let keep = max_chars.saturating_sub(truncation_message.len() + 1);
    text.truncate(keep);
    while !text.is_char_boundary(text.len()) {
        text.pop();
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(truncation_message);
    (text, true)
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::workspace;
    use super::*;

    #[cfg(unix)]
    fn stdout_command() -> &'static str {
        "printf 'hello\nworld\n'"
    }

    #[cfg(windows)]
    fn stdout_command() -> &'static str {
        "echo hello && echo world"
    }

    #[cfg(unix)]
    fn stderr_nonzero_command() -> &'static str {
        "printf 'bad\n' >&2; exit 7"
    }

    #[cfg(windows)]
    fn stderr_nonzero_command() -> &'static str {
        "echo bad 1>&2 && exit /B 7"
    }

    #[cfg(unix)]
    fn read_marker_command() -> &'static str {
        "cat cwd-marker.txt"
    }

    #[cfg(windows)]
    fn read_marker_command() -> &'static str {
        "type cwd-marker.txt"
    }

    #[cfg(unix)]
    fn noisy_timeout_command() -> String {
        format!(
            "printf '{}'; sleep 2",
            "x".repeat(RUN_COMMAND_MAX_CHARS + 100)
        )
    }

    #[cfg(windows)]
    fn noisy_timeout_command() -> String {
        // Pre-encoded: `'x' * 80100; Start-Sleep -Seconds 3` as UTF-16LE base64.
        // Using -EncodedCommand avoids cmd.exe mangling special characters (|, >, etc.).
        let b64 = "JwB4ACcAIAAqACAAOAAwADEAMAAwADsAIABTAHQAYQByAHQALQBTAGwAZQBlAHAAIAAtAFMAZQBjAG8AbgBkAHMAIAAzAA==";
        format!("powershell -EncodedCommand {b64}")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_command_captures_stdout_from_workspace() {
        let dir = workspace();
        std::fs::write(dir.path().join("cwd-marker.txt"), "workspace marker\n").unwrap();
        let result = run_command(
            &serde_json::json!({"command": read_marker_command(), "timeout": 5, "brief": "read marker"}),
            dir.path(),
            None,
            None,
        )
        .await;

        assert_eq!(result.status, "ok");
        assert!(result.summary.contains("command exited 0"));
        assert!(result.model_content.contains("stdout:"));
        assert!(result.model_content.contains("workspace marker"));
        let structured = result.structured.unwrap();
        assert_eq!(structured["kind"], "command_result");
        assert_eq!(structured["exit_code"], 0);
        assert_eq!(structured["stdout_lines"], 1);
        assert_eq!(structured["stderr_lines"], 0);
        assert_eq!(structured["timed_out"], false);
        assert_eq!(structured["line_numbered"], false);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_command_treats_nonzero_exit_as_command_evidence() {
        let dir = workspace();
        let result = run_command(
            &serde_json::json!({"command": stderr_nonzero_command(), "timeout": 5, "brief": "test stderr"}),
            dir.path(),
            None,
            None,
        )
        .await;

        assert_eq!(result.status, "ok");
        assert!(result.summary.contains("command exited 7"));
        assert!(result.model_content.contains("stderr:"));
        assert!(result.model_content.contains("bad"));
        assert_eq!(result.structured.as_ref().unwrap()["exit_code"], 7);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_command_reports_timeout_as_tool_error() {
        let dir = workspace();
        let result = run_command(
            &serde_json::json!({"command": noisy_timeout_command(), "timeout": 1, "brief": "test timeout"}),
            dir.path(),
            None,
            None,
        )
        .await;

        assert_eq!(result.status, "error");
        assert!(result.summary.contains("command timed out after 1s"));
        assert_eq!(
            result.structured.as_ref().unwrap()["kind"],
            "command_result"
        );
        assert_eq!(result.structured.as_ref().unwrap()["timed_out"], true);
        assert!(result.truncated);
        assert!(result.model_content.len() <= RUN_COMMAND_MAX_CHARS);
        assert!(result.model_content.contains("Command output truncated"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_command_rejects_invalid_timeout_empty_command_and_dangerous_commands() {
        let dir = workspace();

        let missing = run_command(
            &serde_json::json!({"timeout": 1, "brief": "test"}),
            dir.path(),
            None,
            None,
        )
        .await;
        assert_eq!(missing.status, "error");
        assert!(missing
            .model_content
            .contains("run_command requires command"));

        let empty = run_command(
            &serde_json::json!({"command": "   ", "timeout": 1, "brief": "test"}),
            dir.path(),
            None,
            None,
        )
        .await;
        assert_eq!(empty.status, "error");
        assert!(empty.model_content.contains("command must not be empty"));

        let zero = run_command(
            &serde_json::json!({"command": stdout_command(), "timeout": 0, "brief": "test"}),
            dir.path(),
            None,
            None,
        )
        .await;
        assert_eq!(zero.status, "error");
        assert!(zero.model_content.contains("timeout must be >= 1"));

        let over_cap = run_command(
            &serde_json::json!({"command": stdout_command(), "timeout": 601, "brief": "test"}),
            dir.path(),
            None,
            None,
        )
        .await;
        assert_eq!(over_cap.status, "error");
        assert!(over_cap.model_content.contains("timeout must be <= 600"));

        let dangerous = run_command(
            &serde_json::json!({"command": "git reset --hard HEAD", "timeout": 1, "brief": "test danger"}),
            dir.path(),
            None,
            None,
        )
        .await;
        assert_eq!(dangerous.status, "blocked");
        assert!(dangerous
            .model_content
            .contains("blocked by command hard guard"));
    }

    #[test]
    fn command_hard_guard_does_not_block_git_push_or_gh_pr_create() {
        assert_eq!(blocked_command_reason("git push origin main"), None);
        assert_eq!(blocked_command_reason("gh pr create --fill"), None);
    }

    #[test]
    fn command_hard_guard_still_blocks_destructive_commands() {
        for command in [
            "git reset --hard HEAD",
            "sh -c 'git reset --hard HEAD'",
            "sh -c 'sudo git reset --hard HEAD'",
            "rm -rf target",
            "npm publish",
            "make deploy",
        ] {
            assert!(
                blocked_command_reason(command).is_some(),
                "{command} should remain hard guarded"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_command_rejects_missing_brief() {
        let dir = workspace();
        let result = run_command(
            &serde_json::json!({"command": "echo hi", "timeout": 1}),
            dir.path(),
            None,
            None,
        )
        .await;
        assert_eq!(result.status, "error");
        assert!(result.model_content.contains("run_command requires brief"));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn run_command_streams_stdout_incrementally() {
        let dir = workspace();
        let (tx, mut rx) = mpsc::channel::<CommandEvent>(64);

        // Generate >4KB to reliably trigger the byte threshold flush
        let cmd = "dd if=/dev/zero bs=1 count=5000 2>/dev/null | tr '\\0' 'x'";

        let handle = tokio::spawn(async move {
            run_command(
                &serde_json::json!({"command": cmd, "timeout": 5, "brief": "stream test"}),
                dir.path(),
                Some(tx),
                None,
            )
            .await
        });

        let mut events: Vec<CommandEvent> = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }

        let result = handle.await.unwrap();
        assert_eq!(result.status, "ok");
        assert!(
            !events.is_empty(),
            "streaming path must emit at least one event"
        );
        for event in &events {
            assert!(
                matches!(event, CommandEvent::Stdout(_)),
                "all events should be stdout, got: {event:?}"
            );
        }
        let streamed: String = events
            .iter()
            .map(|e| match e {
                CommandEvent::Stdout(t) | CommandEvent::Stderr(t) => t.as_str(),
            })
            .collect();
        assert_eq!(streamed.len(), 5000);
        assert!(streamed.chars().all(|c| c == 'x'));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn run_command_streams_stderr_separately() {
        let dir = workspace();
        let (tx, mut rx) = mpsc::channel::<CommandEvent>(64);

        // 5000 bytes to stderr triggers byte threshold, not just final flush
        let cmd = "dd if=/dev/zero bs=1 count=5000 2>/dev/null | tr '\\0' 'y' >&2";

        let handle = tokio::spawn(async move {
            run_command(
                &serde_json::json!({"command": cmd, "timeout": 5, "brief": "stderr stream"}),
                dir.path(),
                Some(tx),
                None,
            )
            .await
        });

        let mut events: Vec<CommandEvent> = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }

        let result = handle.await.unwrap();
        assert_eq!(result.status, "ok");
        assert!(!events.is_empty());
        for event in &events {
            assert!(
                matches!(event, CommandEvent::Stderr(_)),
                "all events should be stderr"
            );
        }
        let streamed: String = events
            .iter()
            .map(|e| match e {
                CommandEvent::Stdout(t) | CommandEvent::Stderr(t) => t.as_str(),
            })
            .collect();
        assert_eq!(streamed.len(), 5000);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn run_command_streams_fast_small_output_via_final_flush() {
        let dir = workspace();
        let (tx, mut rx) = mpsc::channel::<CommandEvent>(64);

        // Completes in < 50ms — too fast for the timer, must rely on final flush
        let handle = tokio::spawn(async move {
            run_command(
                &serde_json::json!({"command": "echo hello", "timeout": 5, "brief": "fast output"}),
                dir.path(),
                Some(tx),
                None,
            )
            .await
        });

        let mut events: Vec<CommandEvent> = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }

        let result = handle.await.unwrap();
        assert_eq!(result.status, "ok");
        assert_eq!(events.len(), 1, "fast output should flush exactly once");
        let text = match &events[0] {
            CommandEvent::Stdout(t) => t.clone(),
            _ => panic!("expected stdout"),
        };
        assert!(text.contains("hello"));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn run_command_streaming_no_output_produces_no_events() {
        let dir = workspace();
        let (tx, rx) = mpsc::channel::<CommandEvent>(64);

        let result = run_command(
            &serde_json::json!({"command": "true", "timeout": 5, "brief": "no output"}),
            dir.path(),
            Some(tx),
            None,
        )
        .await;

        assert_eq!(result.status, "ok");
        assert!(rx.is_empty(), "no output should produce no events");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn run_command_streaming_non_utf8_output_is_lossy() {
        let dir = workspace();
        let (tx, mut rx) = mpsc::channel::<CommandEvent>(64);

        // Outputs raw byte 0xFF which is never valid UTF-8
        let cmd = "printf '\\377'";

        let handle = tokio::spawn(async move {
            run_command(
                &serde_json::json!({"command": cmd, "timeout": 5, "brief": "binary output"}),
                dir.path(),
                Some(tx),
                None,
            )
            .await
        });

        let mut events: Vec<CommandEvent> = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }

        let result = handle.await.unwrap();
        assert_eq!(result.status, "ok");
        // Must not panic from invalid UTF-8; U+FFFD replacement is fine
        assert!(!events.is_empty());
    }
}
