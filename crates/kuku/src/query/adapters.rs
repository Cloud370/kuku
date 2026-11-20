use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, watch, Notify};
use tokio_stream::StreamExt;

use crate::adapter::{RegisteredTool, ToolCancellation, ToolOutput, ToolStatus};
use crate::permission::ToolAuthorization;
use crate::provider::types::{
    PreparedProviderRequest, ProviderFailure, ProviderFailureKind, ResolvedProvider,
};
use crate::tool::builtin::run_command::output::{
    read_output, OutputRead, OutputStream, OutputWriter, MAX_FRAME_BYTES,
};
use crate::tool::{ToolFaults, ToolResultEnvelope};
use crate::{ModelRequest, Result};

use super::slots::RetainedOutputSpec;
use super::types::{ExecSlot, PendingRun, SlotEvent, ToolKind};

pub(super) async fn model_stream(
    pending: &mut PendingRun,
    resolved: &ResolvedProvider,
    prepared: &PreparedProviderRequest,
    trace: Option<crate::provider::trace::ProviderTraceMetadata>,
) -> std::result::Result<crate::provider::ProviderChunkStream, ProviderFailure> {
    let Some(adapter) = &pending.query.model_adapter else {
        return crate::provider::stream_prepared_provider(resolved, prepared, trace).await;
    };
    let request = ModelRequest {
        session_id: pending.session_id.clone(),
        agent: pending.conversation.as_str().into(),
        turn: pending.turn,
        request_id: format!("req_{}", pending.request_num),
        connection: prepared.connection_name.clone(),
        protocol: crate::adapter::protocol(prepared.kind),
        model: prepared.model.clone(),
        endpoint: prepared.url.clone(),
        body: prepared.body.clone(),
        settings: prepared.settings.clone(),
        correlation_id: prepared.correlation_id.clone(),
    };
    let stream = adapter.stream(request).await.map_err(model_failure)?;
    Ok(Box::pin(stream.map(|result| result.map_err(model_failure))))
}

fn model_failure(error: crate::Error) -> ProviderFailure {
    let kind = match &error {
        crate::Error::Provider { kind, .. } => *kind,
        _ => ProviderFailureKind::Internal,
    };
    ProviderFailure {
        kind,
        message: error.to_string(),
        status: None,
        provider_request_id: None,
        retry_after: None,
        retryable: kind.is_retryable(),
    }
}

pub(super) fn spawn_tool(
    registered: RegisteredTool,
    action: ToolAuthorization,
    output: RetainedOutputSpec,
    event_tx: mpsc::Sender<(String, SlotEvent)>,
) -> ExecSlot {
    let cancel = Arc::new(Notify::new());
    let slot_cancel = cancel.clone();
    let tool_call_id = action.tool_call_id.clone();
    let slot_tool_call_id = tool_call_id.clone();
    let conversation = crate::conversation::address::ConversationAddress::parse(&action.agent)
        .expect("session agent address was validated");
    let label = format!("{} (host tool)", registered.spec.name);
    let workspace_ordered = registered.spec.access != crate::ToolAccess::Read;
    tokio::spawn(async move {
        let result = execute_tool(registered, action, output, slot_cancel)
            .await
            .unwrap_or_else(|error| {
                ToolResultEnvelope::error("host tool failed", error.to_string())
            });
        let _ = event_tx.send((tool_call_id, SlotEvent::Done(result))).await;
    });
    ExecSlot {
        tool_call_id: slot_tool_call_id,
        conversation: Some(conversation),
        kind: ToolKind::Simple,
        workspace_ordered,
        label,
        cancel,
        nested_permissions: Arc::new(Mutex::new(HashMap::new())),
        job: None,
    }
}

async fn execute_tool(
    registered: RegisteredTool,
    action: ToolAuthorization,
    mut output: RetainedOutputSpec,
    cancel: Arc<Notify>,
) -> Result<ToolResultEnvelope> {
    output.limits.max_preview_bytes = output
        .limits
        .max_preview_bytes
        .min(registered.spec.max_result_bytes);
    let writer = OutputWriter::create(&output.dir, &output.id, output.limits).await?;
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let adapter = registered.adapter.clone();
    let mut execute =
        tokio::spawn(async move { adapter.execute(action, ToolCancellation(cancel_rx)).await });
    let (result, cancelled) = tokio::select! {
        biased;
        _ = cancel.notified() => {
            let _ = cancel_tx.send(true);
            (execute.await, true)
        }
        result = &mut execute => (result, false),
    };
    let mut result = match result {
        Ok(result) => result.unwrap_or_else(|error| ToolOutput {
            status: ToolStatus::Error,
            summary: "host tool failed".into(),
            content: error.to_string(),
            data: None,
            cleanup_incomplete: false,
        }),
        Err(error) => ToolOutput {
            status: ToolStatus::Error,
            summary: "host tool task failed".into(),
            content: error.to_string(),
            data: None,
            cleanup_incomplete: true,
        },
    };
    if cancelled {
        result.status = ToolStatus::Cancelled;
    }
    let cleanup_incomplete = result.cleanup_incomplete;
    Ok(capture_tool_result(registered, result, writer, output)
        .await
        .unwrap_or_else(|error| {
            ToolResultEnvelope::output_failure(
                "host tool output failed",
                error.to_string(),
                "host_tool_result",
                cleanup_incomplete,
            )
        }))
}

async fn capture_tool_result(
    registered: RegisteredTool,
    mut result: ToolOutput,
    mut writer: OutputWriter,
    mut output: RetainedOutputSpec,
) -> Result<ToolResultEnvelope> {
    write_bytes(&mut writer, result.content.as_bytes()).await?;
    let data = match result.data.take() {
        Some(data) => {
            let encoded = serde_json::to_vec(&data)?;
            if encoded.len() > output.limits.max_preview_bytes / 2 {
                write_bytes(&mut writer, b"\n\nStructured result:\n").await?;
                write_bytes(&mut writer, &encoded).await?;
                None
            } else {
                Some(data)
            }
        }
        None => None,
    };
    let metadata = writer.finish(result.cleanup_incomplete).await?;
    output.retention.keep();
    let chunk = read_output(
        &output.dir,
        &output.id,
        OutputRead {
            offset: 0,
            limit: output.limits.max_preview_bytes,
            search: None,
        },
    )?;
    let mut content = chunk
        .segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<String>();
    let reference_prefix = format!("\n\nOutput: {} ({} captured bytes; read by merged byte offset). capture_incomplete={}; preview_truncated=",
        output.id, metadata.stored_bytes, metadata.quota_truncated || metadata.cleanup_incomplete);
    let preview_truncated = chunk.has_more
        || content.len() + reference_prefix.len() + 5 > output.limits.max_preview_bytes;
    let reference = format!("{reference_prefix}{preview_truncated}");
    let truncated = preview_truncated || metadata.quota_truncated || metadata.cleanup_incomplete;
    truncate(
        &mut content,
        output
            .limits
            .max_preview_bytes
            .saturating_sub(reference.len()),
    );
    content.push_str(&reference);
    truncate(&mut result.summary, 1024);
    Ok(ToolResultEnvelope {
        status: result.status.as_str().into(),
        summary: result.summary,
        model_content: content,
        truncated,
        faults: ToolFaults {
            cleanup_incomplete: metadata.cleanup_incomplete,
            ..Default::default()
        },
        supplied_full_write: false,
        structured: Some(
            serde_json::json!({"kind":"host_tool_result","tool":registered.spec.name,"data":data,"output_id":output.id,
            "output_ref":{"id":output.id,"offset":0,"bytes":metadata.stored_bytes},"capture_complete":!metadata.quota_truncated && !metadata.cleanup_incomplete,
            "quota_truncated":metadata.quota_truncated,"cleanup_incomplete":metadata.cleanup_incomplete,"preview_truncated":preview_truncated}),
        ),
    })
}

async fn write_bytes(writer: &mut OutputWriter, bytes: &[u8]) -> std::io::Result<()> {
    for chunk in bytes.chunks(MAX_FRAME_BYTES) {
        writer.write(OutputStream::Stdout, chunk).await?;
    }
    Ok(())
}

fn truncate(text: &mut String, limit: usize) {
    if text.len() > limit {
        let mut boundary = limit;
        while !text.is_char_boundary(boundary) {
            boundary -= 1;
        }
        text.truncate(boundary);
    }
}
