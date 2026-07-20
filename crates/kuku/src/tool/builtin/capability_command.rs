use std::sync::Arc;

use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::Duration;

use crate::tool::ToolResultEnvelope;

use super::run_command::{
    blocked_command_reason, render_command_cancelled_result, render_command_parts,
    render_command_timeout_result, run_command_request, CommandEvent,
};

const RUN_COMMAND_MAX_CHARS: usize = 80_000;

pub(crate) async fn run_command_with_capability(
    args: &Value,
    capability: Arc<dyn crate::query::WorkspaceQueryCapability>,
    event_tx: Option<mpsc::Sender<CommandEvent>>,
    cancellation: crate::query::WorkspaceCommandCancellation,
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
    let (capability_tx, forward_handle) = match event_tx {
        Some(event_tx) => {
            let (tx, mut rx) = mpsc::channel(64);
            let forward_cancellation = cancellation.clone();
            let handle = tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    let event = match event {
                        crate::query::WorkspaceCommandEvent::Stdout(bytes) => {
                            CommandEvent::Stdout(String::from_utf8_lossy(&bytes).into_owned())
                        }
                        crate::query::WorkspaceCommandEvent::Stderr(bytes) => {
                            CommandEvent::Stderr(String::from_utf8_lossy(&bytes).into_owned())
                        }
                    };
                    tokio::select! {
                        biased;
                        _ = forward_cancellation.cancelled() => break,
                        result = event_tx.send(event) => {
                            if result.is_err() {
                                forward_cancellation.cancel();
                                break;
                            }
                        }
                    }
                }
            });
            (Some(tx), Some(handle))
        }
        None => (None, None),
    };
    let output = capability
        .run_command(
            crate::query::WorkspaceCommandRequest {
                command: request.command.clone(),
                timeout: Duration::from_secs(request.timeout_seconds),
                max_output_bytes: RUN_COMMAND_MAX_CHARS,
            },
            capability_tx,
            cancellation,
        )
        .await;
    if let Some(handle) = forward_handle {
        let _ = handle.await;
    }
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return ToolResultEnvelope::error(
                format!("failed: command could not run: {error}"),
                format!("command could not run: {error}"),
            )
        }
    };
    if output.cancelled {
        return render_command_cancelled_result(
            &request.command,
            output.duration_ms,
            &output.stdout,
            &output.stderr,
        );
    }
    if output.timed_out {
        return render_command_timeout_result(
            &request.command,
            request.timeout_seconds,
            output.duration_ms,
            &output.stdout,
            &output.stderr,
        );
    }
    render_command_parts(
        &request.command,
        output.exit_code,
        output.stdout,
        output.stderr,
        output.duration_ms,
    )
}
