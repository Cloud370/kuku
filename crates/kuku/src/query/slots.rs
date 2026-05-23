use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, oneshot, Notify};

use super::types::{ExecSlot, PermissionChoice, PermissionMode, SlotEvent, ToolEvent, ToolKind};
use super::UiEvent;

#[allow(dead_code)]
pub(crate) fn spawn_simple_slot(
    tool_call_id: String,
    tool_name: String,
    args: serde_json::Value,
    summary: String,
    workspace: PathBuf,
    kuku_home: PathBuf,
    slot_index: usize,
    event_tx: mpsc::Sender<(usize, SlotEvent)>,
) -> ExecSlot {
    let cancel = Arc::new(Notify::new());
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        let result = tokio::select! {
            biased;
            _ = cancel_clone.notified() => SlotEvent::Done {
                status: "cancelled".into(),
                summary: "cancelled".into(),
                result: None,
            },
            r = crate::tool::dispatch::dispatch(
                &tool_name, &args, &workspace, &kuku_home, &[], 0, None,
            ) => SlotEvent::Done {
                status: r.status,
                summary: r.summary,
                result: r.structured,
            },
        };
        let _ = event_tx.send((slot_index, result)).await;
    });

    ExecSlot {
        tool_call_id,
        kind: ToolKind::Simple,
        label: summary,
        cancel,
        child_permissions: Arc::new(Mutex::new(HashMap::new())),
    }
}

#[allow(dead_code, clippy::too_many_arguments)]
pub(crate) async fn spawn_agent_slot(
    tool_call_id: String,
    agent_name: String,
    prompt: String,
    summary: String,
    definition: &crate::subagent::definition::SubagentDefinition,
    parent_session_dir: &std::path::Path,
    workspace: &std::path::Path,
    kuku_home: &std::path::Path,
    config: Arc<crate::config::Config>,
    prompts_dir: Option<&std::path::Path>,
    child_session_id: String,
    child_session_count: u32,
    slot_index: usize,
    event_tx: mpsc::Sender<(usize, SlotEvent)>,
) -> crate::error::Result<ExecSlot> {

    let cancel = Arc::new(Notify::new());
    let child_permissions: Arc<Mutex<HashMap<String, oneshot::Sender<PermissionChoice>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let _child_session_count = child_session_count;

    let mut child_run = crate::subagent::session::start_child_session(
        parent_session_dir,
        &child_session_id,
        definition,
        &prompt,
        workspace,
        kuku_home,
        config,
        prompts_dir,
        PermissionMode::AutoAllow,
    )
    .await?;

    let cancel_clone = cancel.clone();
    let cp = child_permissions.clone();
    let agent_name_clone = agent_name.clone();
    let child_session_id_clone = child_session_id.clone();
    let event_tx_clone = event_tx.clone();

    tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                biased;
                _ = cancel_clone.notified() => {
                    child_run.cancel();
                    let _ = event_tx_clone.send((slot_index, SlotEvent::Done {
                        status: "cancelled".into(),
                        summary: format!("{agent_name_clone} cancelled"),
                        result: None,
                    })).await;
                    return;
                }
                result = child_run.next() => result,
            };
            match event {
                Ok(Some(UiEvent::Done { output, .. })) => {
                    let _ = event_tx_clone
                        .send((
                            slot_index,
                            SlotEvent::Done {
                                status: "ok".into(),
                                summary: format!(
                                    "{agent_name_clone} completed in {} turns",
                                    output.turn
                                ),
                                result: Some(serde_json::json!({
                                    "kind": "subagent_result",
                                    "child_session_id": child_session_id_clone,
                                    "turns_completed": output.turn,
                                })),
                            },
                        ))
                        .await;
                    return;
                }
                Ok(Some(UiEvent::PermissionRequested { request })) => {
                    let request_id = request.id.clone();
                    let (ptx, prx) = oneshot::channel();
                    cp.lock().unwrap().insert(request_id.clone(), ptx);
                    let _ = event_tx_clone
                        .send((
                            slot_index,
                            SlotEvent::Output(ToolEvent::PermissionRequested { request }),
                        ))
                        .await;
                    let choice = prx.await.unwrap_or(PermissionChoice::Deny);
                    let _ = child_run.decide(&request_id, choice, None).await;
                }
                Ok(Some(child_event)) => {
                    if let Some(te) = map_ui_to_tool_event(child_event) {
                        if event_tx_clone
                            .send((slot_index, SlotEvent::Output(te)))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    let _ = event_tx_clone
                        .send((
                            slot_index,
                            SlotEvent::Done {
                                status: "error".into(),
                                summary: format!(
                                    "{agent_name_clone}: stream ended unexpectedly"
                                ),
                                result: None,
                            },
                        ))
                        .await;
                    return;
                }
            }
        }
    });

    Ok(ExecSlot {
        tool_call_id,
        kind: ToolKind::Agent {
            child_session_id,
        },
        label: summary,
        cancel,
        child_permissions,
    })
}

#[allow(dead_code)]
pub(crate) fn spawn_command_slot(
    tool_call_id: String,
    command: String,
    timeout: u64,
    summary: String,
    workspace: PathBuf,
    kuku_home: PathBuf,
    slot_index: usize,
    event_tx: mpsc::Sender<(usize, SlotEvent)>,
) -> ExecSlot {
    let cancel = Arc::new(Notify::new());
    let cancel_clone = cancel.clone();

    let summary_clone = summary.clone();
    tokio::spawn(async move {
        let args = serde_json::json!({"command": command, "timeout": timeout, "brief": summary_clone});
        let result = tokio::select! {
            biased;
            _ = cancel_clone.notified() => SlotEvent::Done {
                status: "cancelled".into(),
                summary: "cancelled".into(),
                result: None,
            },
            r = crate::tool::dispatch::dispatch(
                "run_command",
                &args,
                &workspace,
                &kuku_home,
                &[],
                0,
                None,
            ) => SlotEvent::Done {
                status: r.status,
                summary: r.summary,
                result: r.structured,
            },
        };
        let _ = event_tx.send((slot_index, result)).await;
    });

    ExecSlot {
        tool_call_id,
        kind: ToolKind::Command { pid: None },
        label: summary,
        cancel,
        child_permissions: Arc::new(Mutex::new(HashMap::new())),
    }
}

#[allow(dead_code)]
pub(crate) fn map_ui_to_tool_event(event: crate::query::UiEvent) -> Option<ToolEvent> {
    use crate::query::UiEvent;
    match event {
        UiEvent::TextDelta { text } => Some(ToolEvent::TextDelta { text }),
        UiEvent::ThinkingDelta { text } => Some(ToolEvent::ThinkingDelta { text }),
        UiEvent::ToolStart {
            id,
            tool,
            summary,
            kind,
        } => Some(ToolEvent::ToolStart {
            id,
            tool,
            summary,
            kind,
        }),
        UiEvent::ToolOutput { id, event } => {
            Some(ToolEvent::ToolOutput {
                id,
                event: Box::new(event),
            })
        }
        UiEvent::ToolEnd {
            id, status, summary, ..
        } => Some(ToolEvent::ToolEnd {
            id,
            status,
            summary,
        }),
        UiEvent::PermissionRequested { request } => {
            Some(ToolEvent::PermissionRequested { request })
        }
        UiEvent::Error { code, message } => Some(ToolEvent::Error { code, message }),
        UiEvent::Done { .. } => None,
        UiEvent::TurnStart { .. } | UiEvent::ModelRequest { .. } => None,
    }
}
