use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::{mpsc, Notify};

use crate::query::types::{
    ExecSlot, PendingRun, QueuedToolCall, SlotEvent, ToolKind, WorkspaceAccess,
};
use crate::tool::{builtin::read_image, ToolResultEnvelope};
use crate::Session;

pub(super) fn spawn(
    session: Session,
    pending: &PendingRun,
    queued: &QueuedToolCall,
    event_tx: mpsc::Sender<(String, SlotEvent)>,
) -> ExecSlot {
    let cancel = Arc::new(Notify::new());
    let stop = cancel.clone();
    let id = queued.tool_call.id.clone();
    let args = queued.tool_call.args.clone();
    let agent = pending.conversation.as_str().to_owned();
    let turn = pending.turn;
    let workspace = pending.workspace.clone();
    tokio::spawn(async move {
        let path = args["path"].as_str().unwrap_or_default().to_owned();
        let operations = session.workspace_execution_host().operations();
        let result = async {
            let _permit = tokio::select! {
                biased;
                _ = stop.notified() => return Err(cancelled()),
                permit = operations.acquire_owned() => permit.map_err(|err| {
                    ToolResultEnvelope::error("workspace capacity unavailable", err.to_string())
                })?,
            };
            let stopped = Arc::new(AtomicBool::new(false));
            let check = stopped.clone();
            let mut read = tokio::task::spawn_blocking(move || {
                read_image(&args, &workspace, &|| check.load(Ordering::Acquire))
            });
            let (mime, bytes) = tokio::select! {
                biased;
                _ = stop.notified() => {
                    stopped.store(true, Ordering::Release);
                    let _ = read.await;
                    return Err(cancelled());
                }
                result = &mut read => result.map_err(|err| {
                    ToolResultEnvelope::error("image reader failed", err.to_string())
                })??,
            };
            let image = session
                .retain_model_image(&agent, turn, mime, &bytes)
                .await
                .map_err(|err| {
                    ToolResultEnvelope::error("image could not be retained", err.to_string())
                })?;
            let summary = format!("Image {path} ({} bytes, {})", bytes.len(), mime.as_str());
            let text = format!(
                "{summary}. For a fresh result missing its image attachment, try a smaller copy."
            );
            Ok(ToolResultEnvelope::ok(summary, text, image.model_facts()))
        }
        .await
        .unwrap_or_else(|result| result);
        let _ = event_tx.send((id, SlotEvent::Done(result))).await;
    });
    ExecSlot {
        tool_call_id: queued.tool_call.id.clone(),
        conversation: Some(pending.conversation.clone()),
        kind: ToolKind::Simple,
        workspace_access: WorkspaceAccess::Read,
        label: queued.display_summary.clone(),
        cancel,
        job: None,
    }
}

fn cancelled() -> ToolResultEnvelope {
    ToolResultEnvelope::cancelled("image inspection cancelled")
}
