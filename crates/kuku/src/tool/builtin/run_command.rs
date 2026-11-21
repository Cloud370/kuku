use std::io;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Child;
use tokio::sync::{mpsc, Notify};
use tokio::time::{sleep_until, timeout, Duration, Instant as TokioInstant};

use crate::execution::CommandTermination;
use crate::permission::gate::blocked_command_reason;
use crate::tool::ToolResultEnvelope;

use output::{read_output, OutputLimits, OutputRead, OutputStream, OutputWriter};
use process::ProcessTree;
use render::render_command_result;
use request::{run_command_request, CommandRequest};

pub(crate) use request::{command_permission_candidate, prepare_command_args};
pub(crate) use shell::available_shells;

pub(crate) mod encoding;
#[cfg(unix)]
mod native;
pub(crate) mod output;
mod process;
mod render;
mod reports;
mod request;
mod shell;

#[cfg(all(test, windows))]
mod windows_tests;

const RUN_COMMAND_MAX_CHARS: usize = 80_000;
const READ_BUFFER_BYTES: usize = 8192;
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(1);
static OUTPUT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub(crate) enum CommandEvent {
    Stdout(String),
    Stderr(String),
}

#[derive(Default)]
struct CommandOutcome {
    exit_status: Option<ExitStatus>,
    started: bool,
    start_error: Option<(CommandTermination, String)>,
    timed_out: bool,
    cancelled: bool,
    cleanup_incomplete: bool,
    dropped_events: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelOutputAccess {
    Unavailable,
    Read,
    Script,
}

enum CommandView {
    Model(ModelOutputAccess),
    Script { host_bytes: usize },
}

pub(crate) struct CommandCapture<'a> {
    dir: &'a Path,
    id: &'a str,
    limits: OutputLimits,
    view: CommandView,
}

impl<'a> CommandCapture<'a> {
    pub(crate) fn model(
        dir: &'a Path,
        id: &'a str,
        limits: OutputLimits,
        access: ModelOutputAccess,
    ) -> Self {
        Self {
            dir,
            id,
            limits,
            view: CommandView::Model(access),
        }
    }
}

pub(crate) async fn run_command(
    args: &Value,
    workspace: &Path,
    event_tx: Option<mpsc::Sender<CommandEvent>>,
    cancel: Option<Arc<Notify>>,
) -> ToolResultEnvelope {
    let definition = crate::tool::registry::run_command_definition(&available_shells(), false);
    let mut args = args.clone();
    if let Err(result) = prepare_command_args(&mut args, workspace, &definition) {
        return result;
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = OUTPUT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let output_id = format!("command-{}-{timestamp}-{sequence}", std::process::id());
    run_command_with_output(
        &args,
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
    run_command_to(
        args,
        workspace,
        event_tx,
        cancel,
        CommandCapture::model(
            output_dir,
            output_id,
            limits,
            ModelOutputAccess::Unavailable,
        ),
    )
    .await
}

pub(crate) async fn run_command_for_script(
    args: &Value,
    workspace: &Path,
    cancel: Arc<Notify>,
    output_dir: &Path,
    output_id: &str,
    limits: OutputLimits,
    host_bytes: usize,
) -> ToolResultEnvelope {
    run_command_to(
        args,
        workspace,
        None,
        Some(cancel),
        CommandCapture {
            dir: output_dir,
            id: output_id,
            limits,
            view: CommandView::Script { host_bytes },
        },
    )
    .await
}

pub(crate) async fn run_command_to(
    args: &Value,
    workspace: &Path,
    event_tx: Option<mpsc::Sender<CommandEvent>>,
    cancel: Option<Arc<Notify>>,
    output: CommandCapture<'_>,
) -> ToolResultEnvelope {
    let cancel = cancel.unwrap_or_else(|| Arc::new(Notify::new()));
    let cancelled = cancel.notified();
    tokio::pin!(cancelled);
    cancelled.as_mut().enable();
    let request = match run_command_request(args, workspace) {
        Ok(request) => request,
        Err(result) => return result,
    };
    if let Some(reason) = blocked_command_reason(&request.invocation) {
        return ToolResultEnvelope::blocked(
            format!("blocked by command hard guard: {reason}"),
            format!("blocked by command hard guard: {reason}"),
        );
    }
    let started = Instant::now();
    let mut writer = match OutputWriter::create_command(output.dir, output.id, output.limits).await
    {
        Ok(writer) => writer,
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
        match execute_command(&request, &mut writer, event_tx, cancelled.as_mut()).await {
            Ok(outcome) => outcome,
            Err(result) => return result,
        }
    };
    let capture = match writer.finish_command(outcome.cleanup_incomplete).await {
        Ok(result) => result,
        Err(error) => return output_error("sync", error, outcome.cleanup_incomplete),
    };
    render_command_result(&request, outcome, capture, started.elapsed(), output)
}

async fn execute_command(
    request: &CommandRequest,
    output: &mut OutputWriter,
    event_tx: Option<mpsc::Sender<CommandEvent>>,
    cancelled: std::pin::Pin<&mut tokio::sync::futures::Notified<'_>>,
) -> Result<CommandOutcome, ToolResultEnvelope> {
    let mut prepared = match request.command() {
        Ok(command) => command,
        Err(error) => {
            let termination = if error.kind() == io::ErrorKind::NotFound {
                CommandTermination::ProgramMissing
            } else {
                CommandTermination::StartFailed
            };
            return Ok(CommandOutcome {
                start_error: Some((termination, error.to_string())),
                ..CommandOutcome::default()
            });
        }
    };
    prepared
        .command
        .current_dir(&request.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (mut child, mut tree) = match ProcessTree::spawn(&mut prepared.command) {
        Ok(process) => process,
        Err(error) => {
            let termination = if error.kind() == io::ErrorKind::NotFound
                && request.cwd.is_dir()
                && !prepared.executable_exists(&request.cwd)
            {
                CommandTermination::ProgramMissing
            } else {
                CommandTermination::StartFailed
            };
            return Ok(CommandOutcome {
                start_error: Some((termination, error.to_string())),
                ..CommandOutcome::default()
            });
        }
    };
    let mut capture =
        capture_command(&mut child, &mut tree, request, output, &event_tx, cancelled).await;
    if child.id().is_some() {
        let stop = tree.request_stop(&mut child);
        let status = timeout(CLEANUP_TIMEOUT, tree.wait(&mut child))
            .await
            .unwrap_or_else(|_| {
                Err(io::Error::other(
                    "command process did not exit during cleanup",
                ))
            })
            .map_err(cleanup_error)?;
        stop.map_err(cleanup_error)?;
        if let Ok(outcome) = &mut capture {
            outcome.exit_status = Some(status);
        }
    }
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
    let mut outcome = CommandOutcome {
        started: true,
        ..CommandOutcome::default()
    };
    let mut exited = false;
    let mut stopping = false;
    let deadline = request
        .timeout_seconds
        .map(|seconds| TokioInstant::now() + Duration::from_secs(u64::from(seconds)));
    let mut cleanup_deadline = None;
    while !exited || stdout.is_some() || stderr.is_some() {
        tokio::select! {
            _ = cancelled.as_mut(), if !stopping && !exited => {
                outcome.cancelled = true;
                stopping = true;
                tree.request_stop(child).map_err(cleanup_error)?;
                cleanup_deadline = Some(TokioInstant::now() + CLEANUP_TIMEOUT);
            }
            _ = wait_deadline(deadline), if !stopping && !exited => {
                outcome.timed_out = true;
                stopping = true;
                tree.request_stop(child).map_err(cleanup_error)?;
                cleanup_deadline = Some(TokioInstant::now() + CLEANUP_TIMEOUT);
            }
            status = tree.wait(child), if !exited => {
                outcome.exit_status = Some(status.map_err(cleanup_error)?);
                exited = true;
                cleanup_deadline = Some(TokioInstant::now() + CLEANUP_TIMEOUT);
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
            _ = wait_deadline(cleanup_deadline), if cleanup_deadline.is_some() => {
                if !exited {
                    if let Some(status) = tree.try_wait(child).map_err(cleanup_error)? {
                        outcome.exit_status = Some(status);
                        exited = true;
                        cleanup_deadline = Some(TokioInstant::now() + CLEANUP_TIMEOUT);
                        continue;
                    }
                }
                outcome.cleanup_incomplete = stdout.is_some() || stderr.is_some();
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

async fn wait_deadline(deadline: Option<TokioInstant>) {
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

#[cfg(test)]
mod tests;
