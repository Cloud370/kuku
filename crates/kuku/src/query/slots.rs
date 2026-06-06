use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, oneshot, Notify};

use crate::event::StoredEvent;

use super::types::{ExecSlot, PermissionChoice, PermissionMode, SlotEvent, ToolEvent, ToolKind};
use super::UiEvent;

pub(crate) fn requires_ordered_simple_execution(tool_name: &str) -> bool {
    matches!(tool_name, "read_file" | "edit_file" | "write_file")
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_simple_slot(
    tool_call_id: String,
    tool_name: String,
    args: serde_json::Value,
    summary: String,
    workspace: PathBuf,
    kuku_home: PathBuf,
    prior_events: Vec<StoredEvent>,
    event_tx: mpsc::Sender<(String, SlotEvent)>,
    config: std::sync::Arc<crate::config::Config>,
    catalog: crate::prompt::PromptCatalog,
    events_path: PathBuf,
) -> ExecSlot {
    let cancel = Arc::new(Notify::new());
    let cancel_clone = cancel.clone();
    let tc_id = tool_call_id.clone();
    let dispatch_tool_call_id = tool_call_id.clone();
    let ordered_with_simple_tools = requires_ordered_simple_execution(&tool_name);

    tokio::spawn(async move {
        let result = tokio::select! {
            biased;
            _ = cancel_clone.notified() => SlotEvent::Done {
                status: "cancelled".into(),
                summary: "cancelled".into(),
                model_content: String::new(),
                result: None,
            },
            r = crate::tool::dispatch::dispatch(
                &tool_name,
                &args,
                &workspace,
                &kuku_home,
                &prior_events,
                0,
                Some(&dispatch_tool_call_id),
                &config,
                &catalog,
                &events_path,
            ) => SlotEvent::Done {
                status: r.status,
                summary: r.summary,
                model_content: r.model_content,
                result: r.structured,
            },
        };
        let _ = event_tx.send((tc_id, result)).await;
    });

    ExecSlot {
        tool_call_id,
        kind: ToolKind::Simple,
        ordered_with_simple_tools,
        label: summary,
        cancel,
        child_permissions: Arc::new(Mutex::new(HashMap::new())),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_agent_slot(
    tool_call_id: String,
    agent_name: String,
    prompt: String,
    summary: String,
    definition: crate::subagent::definition::SubagentDefinition,
    parent_session_dir: std::path::PathBuf,
    workspace: std::path::PathBuf,
    kuku_home: std::path::PathBuf,
    config: Arc<crate::config::Config>,
    prompts_dir: Option<std::path::PathBuf>,
    child_session_id: String,
    child_session_count: u32,
    event_tx: mpsc::Sender<(String, SlotEvent)>,
) -> ExecSlot {
    let cancel = Arc::new(Notify::new());
    let child_permissions: Arc<Mutex<HashMap<String, oneshot::Sender<PermissionChoice>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let cancel_clone = cancel.clone();
    let cp = child_permissions.clone();
    let tc_id = tool_call_id.clone();
    let child_session_id_for_slot = child_session_id.clone();

    tokio::spawn(async move {
        let mut child_run = match crate::subagent::session::start_child_session(
            &parent_session_dir,
            &child_session_id_for_slot,
            &definition,
            &prompt,
            &workspace,
            &kuku_home,
            config,
            prompts_dir.as_deref(),
            PermissionMode::AutoAllow,
            child_session_count,
        )
        .await
        {
            Ok(run) => run,
            Err(_) => {
                let _ = event_tx
                    .send((
                        tc_id.clone(),
                        SlotEvent::Done {
                            status: "error".into(),
                            summary: format!("{agent_name}: failed to start child session"),
                            model_content: String::new(),
                            result: None,
                        },
                    ))
                    .await;
                return;
            }
        };

        loop {
            let event = tokio::select! {
                biased;
                _ = cancel_clone.notified() => {
                    child_run.cancel();
                    let _ = event_tx.send((tc_id.clone(), SlotEvent::Done {
                        status: "cancelled".into(),
                        summary: format!("{agent_name} cancelled"),
                        model_content: String::new(),
                        result: None,
                    })).await;
                    return;
                }
                result = child_run.next() => result,
            };
            match event {
                Ok(Some(UiEvent::Done { output, .. })) => {
                    let _ = event_tx
                        .send((
                            tc_id.clone(),
                            SlotEvent::Done {
                                status: "ok".into(),
                                summary: format!("{agent_name} completed in {} turns", output.turn),
                                model_content: String::new(),
                                result: Some(serde_json::json!({
                                    "kind": "subagent_result",
                                    "child_session_id": child_session_id_for_slot,
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
                    let _ = event_tx
                        .send((
                            tc_id.clone(),
                            SlotEvent::Output(ToolEvent::PermissionRequested { request }),
                        ))
                        .await;
                    let choice = prx.await.unwrap_or(PermissionChoice::Deny);
                    let _ = child_run.decide(&request_id, choice, None).await;
                }
                Ok(Some(child_event)) => {
                    if let Some(te) = map_ui_to_tool_event(child_event) {
                        if event_tx
                            .send((tc_id.clone(), SlotEvent::Output(te)))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    let _ = event_tx
                        .send((
                            tc_id.clone(),
                            SlotEvent::Done {
                                status: "error".into(),
                                summary: format!("{agent_name}: stream ended unexpectedly"),
                                model_content: String::new(),
                                result: None,
                            },
                        ))
                        .await;
                    return;
                }
            }
        }
    });

    ExecSlot {
        tool_call_id,
        kind: ToolKind::Agent { child_session_id },
        ordered_with_simple_tools: false,
        label: summary,
        cancel,
        child_permissions,
    }
}

pub(crate) fn spawn_command_slot(
    tool_call_id: String,
    args: serde_json::Value,
    summary: String,
    workspace: PathBuf,
    event_tx: mpsc::Sender<(String, SlotEvent)>,
) -> ExecSlot {
    let cancel = Arc::new(Notify::new());
    let cancel_cmd = cancel.clone();
    let tc_id = tool_call_id.clone();

    tokio::spawn(async move {
        let (tool_tx, mut tool_rx) = mpsc::channel::<crate::tool::builtin::CommandEvent>(64);

        let fwd_tc_id = tc_id.clone();
        let fwd_event_tx = event_tx.clone();
        let forward_handle = tokio::spawn(async move {
            while let Some(ce) = tool_rx.recv().await {
                let te = match ce {
                    crate::tool::builtin::CommandEvent::Stdout(text) => ToolEvent::Stdout { text },
                    crate::tool::builtin::CommandEvent::Stderr(text) => ToolEvent::Stderr { text },
                };
                if fwd_event_tx
                    .send((fwd_tc_id.clone(), SlotEvent::Output(te)))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        let r =
            crate::tool::builtin::run_command(&args, &workspace, Some(tool_tx), Some(cancel_cmd))
                .await;
        let _ = forward_handle.await;
        let result = SlotEvent::Done {
            status: r.status,
            summary: r.summary,
            model_content: r.model_content,
            result: r.structured,
        };
        let _ = event_tx.send((tc_id, result)).await;
    });

    ExecSlot {
        tool_call_id,
        kind: ToolKind::Command { pid: None },
        ordered_with_simple_tools: false,
        label: summary,
        cancel,
        child_permissions: Arc::new(Mutex::new(HashMap::new())),
    }
}

pub(crate) struct SlotDispatchArgs {
    pub(crate) tool_name: String,
    pub(crate) tool_id: String,
    pub(crate) args: serde_json::Value,
    pub(crate) summary: String,
    pub(crate) workspace: PathBuf,
    pub(crate) kuku_home: PathBuf,
    pub(crate) prior_events: Vec<StoredEvent>,
    pub(crate) event_tx: mpsc::Sender<(String, SlotEvent)>,
    pub(crate) config: std::sync::Arc<crate::config::Config>,
    pub(crate) catalog: crate::prompt::PromptCatalog,
    pub(crate) events_path: PathBuf,
}

pub(crate) fn dispatch_tool_slot(args: SlotDispatchArgs) -> (ExecSlot, ToolKind) {
    if args.tool_name == "run_command" {
        let slot = spawn_command_slot(
            args.tool_id,
            args.args,
            args.summary,
            args.workspace,
            args.event_tx,
        );
        (slot, ToolKind::Command { pid: None })
    } else {
        let slot = spawn_simple_slot(
            args.tool_id,
            args.tool_name,
            args.args,
            args.summary,
            args.workspace,
            args.kuku_home,
            args.prior_events,
            args.event_tx,
            args.config,
            args.catalog,
            args.events_path,
        );
        (slot, ToolKind::Simple)
    }
}

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
        UiEvent::ToolOutput { id, event } => Some(ToolEvent::ToolOutput {
            id,
            event: Box::new(event),
        }),
        UiEvent::ToolEnd {
            id,
            status,
            summary,
            ..
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
        UiEvent::TurnStart { .. }
        | UiEvent::ModelRequest { .. }
        | UiEvent::Log { .. }
        | UiEvent::Cancelled { .. } => None,
    }
}
