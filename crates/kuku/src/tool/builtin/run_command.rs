use std::io;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, Notify};
use tokio::time::{sleep_until, timeout, Duration, Instant as TokioInstant};

use crate::tool::ToolResultEnvelope;

use super::common::require_brief;
use output::{read_output, OutputLimits, OutputRead, OutputStream, OutputWriter};
use process::ProcessTree;
use render::render_command_result;

pub(crate) mod output;
mod process;
mod render;

#[cfg(all(test, windows))]
mod windows_tests;

const RUN_COMMAND_MAX_CHARS: usize = 80_000;
const RUN_COMMAND_TIMEOUT_CAP_SECONDS: u64 = 600;
const READ_BUFFER_BYTES: usize = 8192;
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(1);
static OUTPUT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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

#[derive(Default)]
struct CommandOutcome {
    exit_code: Option<i32>,
    timed_out: bool,
    cancelled: bool,
    cleanup_incomplete: bool,
    dropped_events: u64,
}

pub(crate) async fn run_command(
    args: &Value,
    workspace: &Path,
    event_tx: Option<mpsc::Sender<CommandEvent>>,
    cancel: Option<Arc<Notify>>,
) -> ToolResultEnvelope {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = OUTPUT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let output_id = format!("command-{}-{timestamp}-{sequence}", std::process::id());
    run_command_with_output(
        args,
        workspace,
        event_tx,
        cancel,
        &workspace.join(".kuku").join("outputs"),
        &output_id,
        OutputLimits::default(),
    )
    .await
}

pub(crate) async fn run_command_with_output(
    args: &Value,
    workspace: &Path,
    event_tx: Option<mpsc::Sender<CommandEvent>>,
    cancel: Option<Arc<Notify>>,
    output_dir: &Path,
    output_id: &str,
    limits: OutputLimits,
) -> ToolResultEnvelope {
    let cancel = cancel.unwrap_or_else(|| Arc::new(Notify::new()));
    let cancelled = cancel.notified();
    tokio::pin!(cancelled);
    cancelled.as_mut().enable();
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
    let mut output = match OutputWriter::create(output_dir, output_id, limits).await {
        Ok(output) => output,
        Err(error) => {
            return ToolResultEnvelope::error(
                format!("failed: output create: {error}"),
                format!(
                    "command output create failed: {error}; no output reference was acknowledged"
                ),
            )
        }
    };
    let already_cancelled = tokio::select! {
        biased;
        _ = cancelled.as_mut() => true,
        _ = std::future::ready(()) => false,
    };
    let outcome = if already_cancelled {
        CommandOutcome {
            cancelled: true,
            ..CommandOutcome::default()
        }
    } else {
        match execute_command(
            &request,
            &workspace,
            &mut output,
            event_tx,
            cancelled.as_mut(),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(result) => return result,
        }
    };
    let metadata = match output.finish(outcome.cleanup_incomplete).await {
        Ok(metadata) => metadata,
        Err(error) => return output_error("sync", error, outcome.cleanup_incomplete),
    };
    render_command_result(
        &request,
        outcome,
        metadata,
        started.elapsed(),
        output_dir,
        output_id,
        limits,
    )
}

async fn execute_command(
    request: &CommandRequest,
    workspace: &Path,
    output: &mut OutputWriter,
    event_tx: Option<mpsc::Sender<CommandEvent>>,
    cancelled: std::pin::Pin<&mut tokio::sync::futures::Notified<'_>>,
) -> Result<CommandOutcome, ToolResultEnvelope> {
    let mut command = shell_command(&request.command);
    command
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (mut child, mut tree) = ProcessTree::spawn(&mut command).map_err(|error| {
        ToolResultEnvelope::error(
            format!("failed: command execution: {error}"),
            format!("command could not start: {error}"),
        )
    })?;
    let capture =
        capture_command(&mut child, &mut tree, request, output, &event_tx, cancelled).await;
    let cleanup = tree.terminate();
    if child.id().is_some() {
        let _ = child.start_kill();
        timeout(CLEANUP_TIMEOUT, child.wait())
            .await
            .unwrap_or_else(|_| {
                Err(io::Error::other(
                    "command process did not exit during cleanup",
                ))
            })
            .map_err(cleanup_error)?;
    }
    cleanup.map_err(cleanup_error)?;
    capture
}

async fn capture_command(
    child: &mut Child,
    tree: &mut ProcessTree,
    request: &CommandRequest,
    output: &mut OutputWriter,
    event_tx: &Option<mpsc::Sender<CommandEvent>>,
    mut cancelled: std::pin::Pin<&mut tokio::sync::futures::Notified<'_>>,
) -> Result<CommandOutcome, ToolResultEnvelope> {
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let mut stdout_buf = [0; READ_BUFFER_BYTES];
    let mut stderr_buf = [0; READ_BUFFER_BYTES];
    let mut outcome = CommandOutcome::default();
    let mut exited = false;
    let mut stopping = false;
    let deadline = TokioInstant::now() + Duration::from_secs(request.timeout_seconds);
    let mut cleanup_deadline = None;
    while !exited || stdout.is_some() || stderr.is_some() {
        tokio::select! {
            _ = cancelled.as_mut(), if !stopping && !exited => {
                outcome.cancelled = true;
                stopping = true;
                tree.terminate().map_err(cleanup_error)?;
                let _ = child.start_kill();
                cleanup_deadline = Some(TokioInstant::now() + CLEANUP_TIMEOUT);
            }
            _ = sleep_until(deadline), if !stopping && !exited => {
                outcome.timed_out = true;
                stopping = true;
                tree.terminate().map_err(cleanup_error)?;
                let _ = child.start_kill();
                cleanup_deadline = Some(TokioInstant::now() + CLEANUP_TIMEOUT);
            }
            status = child.wait(), if !exited => {
                outcome.exit_code = status.map_err(capture_error)?.code();
                exited = true;
                tree.terminate().map_err(cleanup_error)?;
                cleanup_deadline.get_or_insert(TokioInstant::now() + CLEANUP_TIMEOUT);
            }
            read = read_stream(&mut stdout, &mut stdout_buf), if stdout.is_some() => {
                let bytes = read.map_err(capture_error)?;
                if bytes == 0 { stdout = None; } else {
                    output.write(OutputStream::Stdout, &stdout_buf[..bytes]).await.map_err(capture_error)?;
                    outcome.dropped_events += emit_event(event_tx, OutputStream::Stdout, &stdout_buf[..bytes]);
                }
            }
            read = read_stream(&mut stderr, &mut stderr_buf), if stderr.is_some() => {
                let bytes = read.map_err(capture_error)?;
                if bytes == 0 { stderr = None; } else {
                    output.write(OutputStream::Stderr, &stderr_buf[..bytes]).await.map_err(capture_error)?;
                    outcome.dropped_events += emit_event(event_tx, OutputStream::Stderr, &stderr_buf[..bytes]);
                }
            }
            _ = wait_cleanup(cleanup_deadline), if cleanup_deadline.is_some() => {
                outcome.cleanup_incomplete = !exited || stdout.is_some() || stderr.is_some();
                break;
            }
        }
    }
    Ok(outcome)
}

async fn read_stream<P: AsyncRead + Unpin>(
    pipe: &mut Option<P>,
    buffer: &mut [u8],
) -> io::Result<usize> {
    match pipe {
        Some(pipe) => pipe.read(buffer).await,
        None => std::future::pending().await,
    }
}

async fn wait_cleanup(deadline: Option<TokioInstant>) {
    match deadline {
        Some(deadline) => sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

fn emit_event(tx: &Option<mpsc::Sender<CommandEvent>>, stream: OutputStream, bytes: &[u8]) -> u64 {
    let Some(tx) = tx else {
        return 0;
    };
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    let shortened = text.len() > READ_BUFFER_BYTES;
    let mut keep = READ_BUFFER_BYTES.min(text.len());
    while !text.is_char_boundary(keep) {
        keep -= 1;
    }
    text.truncate(keep);
    let event = match stream {
        OutputStream::Stdout => CommandEvent::Stdout(text),
        OutputStream::Stderr => CommandEvent::Stderr(text),
    };
    u64::from(tx.try_send(event).is_err() || shortened)
}

fn capture_error(error: io::Error) -> ToolResultEnvelope {
    output_error("capture", error, false)
}

fn cleanup_error(error: io::Error) -> ToolResultEnvelope {
    output_error("cleanup", error, true)
}

fn output_error(operation: &str, error: io::Error, cleanup_incomplete: bool) -> ToolResultEnvelope {
    ToolResultEnvelope::output_failure(
        format!("failed: output {operation}: {error}"),
        format!("command output {operation} failed: {error}; no output reference was acknowledged"),
        "error",
        cleanup_incomplete,
    )
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

#[cfg(test)]
mod tests;
