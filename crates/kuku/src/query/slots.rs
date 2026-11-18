use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, oneshot, Notify};

use crate::event::StoredEvent;
use crate::tool::ToolResultEnvelope;

use super::types::{ExecSlot, PermissionChoice, SlotEvent, ToolEvent, ToolKind};
use super::UiEvent;

pub(crate) fn requires_workspace_ordering(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "read_file"
            | "workspace_script"
            | "stat_file"
            | "remove_file"
            | "edit_file"
            | "write_file"
            | "remember_memory"
            | "forget_memory"
            | "run_command"
            | "agent"
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_simple_slot(
    tool_call_id: String,
    conversation: Option<crate::conversation::address::ConversationAddress>,
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
    capacity: crate::WorkspaceExecutionHost,
) -> ExecSlot {
    let cancel = Arc::new(Notify::new());
    let cancel_clone = cancel.clone();
    let tc_id = tool_call_id.clone();
    let dispatch_tool_call_id = tool_call_id.clone();
    let dispatch_conversation = conversation
        .clone()
        .unwrap_or(crate::conversation::address::ConversationAddress::MAIN);
    let workspace_ordered = requires_workspace_ordering(&tool_name);

    tokio::spawn(async move {
        let result = if crate::tool::dispatch::is_workspace_file(&tool_name) {
            let operations = capacity.operations();
            let permit = tokio::select! {
                biased;
                _ = cancel_clone.notified() => None,
                permit = operations.acquire_owned() => permit.ok(),
            };
            if let Some(permit) = permit {
                use std::sync::atomic::{AtomicBool, Ordering};
                let stopped = Arc::new(AtomicBool::new(false));
                let check = stopped.clone();
                let mut work = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    crate::tool::dispatch::dispatch_workspace_file(
                        &tool_name,
                        &args,
                        &workspace,
                        &dispatch_conversation,
                        &prior_events,
                        0,
                        Some(&dispatch_tool_call_id),
                        &|| check.load(Ordering::Acquire),
                    )
                    .expect("a known workspace file operation")
                });
                let outcome = tokio::select! {
                    biased;
                    _ = cancel_clone.notified() => {
                        stopped.store(true,Ordering::Release);
                        work.await
                    }
                    result = &mut work => result,
                };
                SlotEvent::Done(outcome.unwrap_or_else(|err| {
                    ToolResultEnvelope::error("workspace file worker failed", err.to_string())
                }))
            } else {
                cancelled_slot("workspace operation cancelled before dispatch")
            }
        } else {
            tokio::select! {
                biased;
                _ = cancel_clone.notified() => cancelled_slot("cancelled"),
                r = crate::tool::dispatch::dispatch(
                    &tool_name,&args,&workspace,&kuku_home,&dispatch_conversation,&prior_events,
                    0,Some(&dispatch_tool_call_id),&config,&catalog,&events_path,
                ) => SlotEvent::Done(r),
            }
        };
        let _ = event_tx.send((tc_id, result)).await;
    });

    ExecSlot {
        tool_call_id,
        conversation,
        kind: ToolKind::Simple,
        workspace_ordered,
        label: summary,
        cancel,
        nested_permissions: Arc::new(Mutex::new(HashMap::new())),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_agent_slot(
    tool_call_id: String,
    conversation: Option<crate::conversation::address::ConversationAddress>,
    dispatch: crate::agent::runtime::PreparedDispatch,
    summary: String,
    workspace: std::path::PathBuf,
    kuku_home: std::path::PathBuf,
    config: Arc<crate::config::Config>,
    prompts_dir: Option<std::path::PathBuf>,
    event_tx: mpsc::Sender<(String, SlotEvent)>,
) -> ExecSlot {
    let cancel = Arc::new(Notify::new());
    let nested_permissions: Arc<Mutex<HashMap<String, oneshot::Sender<PermissionChoice>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let cancel_clone = cancel.clone();
    let cp = nested_permissions.clone();
    let tc_id = tool_call_id.clone();
    let tool_kind = ToolKind::Agent {
        conversation: dispatch.conversation.clone(),
        binding_id: dispatch.binding.binding_id.clone(),
    };
    let dispatch_for_slot = dispatch.clone();

    tokio::spawn(async move {
        let mut child_run = match crate::agent::runtime::start_run(
            dispatch_for_slot,
            &workspace,
            &kuku_home,
            config,
            prompts_dir.as_deref(),
        )
        .await
        {
            Ok(run) => run,
            Err(_) => {
                let _ = event_tx
                    .send((
                        tc_id.clone(),
                        agent_error_event("agent: failed to start conversation".to_string()),
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
                    let _ = event_tx.send((tc_id.clone(), cancelled_slot(format!("{} cancelled", dispatch.conversation.as_str())))).await;
                    return;
                }
                result = child_run.next() => result,
            };
            match event {
                Ok(Some(UiEvent::Done { output, .. })) => {
                    let output_text = output.text.clone();
                    let _ = event_tx
                        .send((
                            tc_id.clone(),
                            SlotEvent::Done(ToolResultEnvelope::ok(
                                format!(
                                    "{} completed in {} turns",
                                    dispatch.conversation.as_str(),
                                    output.turn
                                ),
                                output.text,
                                serde_json::json!({
                                    "kind": "agent_result",
                                    "conversation": dispatch.conversation.as_str(),
                                    "binding_id": dispatch.binding.binding_id,
                                    "turns_completed": output.turn,
                                    "text": output_text,
                                }),
                            )),
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
                            agent_error_event(format!(
                                "{}: stream ended unexpectedly",
                                dispatch.conversation.as_str()
                            )),
                        ))
                        .await;
                    return;
                }
            }
        }
    });

    ExecSlot {
        tool_call_id,
        conversation,
        kind: tool_kind,
        workspace_ordered: true,
        label: summary,
        cancel,
        nested_permissions,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_command_slot(
    tool_call_id: String,
    conversation: Option<crate::conversation::address::ConversationAddress>,
    args: serde_json::Value,
    summary: String,
    workspace: PathBuf,
    event_tx: mpsc::Sender<(String, SlotEvent)>,
    output: Option<RetainedOutputSpec>,
    capacity: crate::WorkspaceExecutionHost,
) -> ExecSlot {
    let cancel = Arc::new(Notify::new());
    let cancel_cmd = cancel.clone();
    let tc_id = tool_call_id.clone();

    tokio::spawn(async move {
        let operations = capacity.operations();
        let _permit = tokio::select! {
            biased;
            _=cancel_cmd.notified()=> {
                let _=event_tx.send((tc_id,cancelled_slot("command cancelled before dispatch"))).await;
                return;
            }
            permit=operations.acquire_owned()=>permit.expect("host capacity remains open"),
        };
        let (tool_tx, mut tool_rx) = mpsc::channel::<crate::tool::builtin::CommandEvent>(64);

        let fwd_tc_id = tc_id.clone();
        let fwd_event_tx = event_tx.clone();
        let forward_handle = tokio::spawn(async move {
            while let Some(ce) = tool_rx.recv().await {
                let te = match ce {
                    crate::tool::builtin::CommandEvent::Stdout(text) => ToolEvent::Stdout { text },
                    crate::tool::builtin::CommandEvent::Stderr(text) => ToolEvent::Stderr { text },
                };
                let _ = fwd_event_tx.try_send((fwd_tc_id.clone(), SlotEvent::Output(te)));
            }
        });

        let r = match output {
            Some(mut output) => {
                let result = crate::tool::builtin::run_command::run_command_with_output(
                    &args,
                    &workspace,
                    Some(tool_tx),
                    Some(cancel_cmd),
                    &output.dir,
                    &output.id,
                    output.limits,
                )
                .await;
                if result
                    .structured
                    .as_ref()
                    .is_some_and(|facts| facts["output_ref"].is_object())
                {
                    output.retention.keep();
                }
                result
            }
            None => {
                crate::tool::builtin::run_command(
                    &args,
                    &workspace,
                    Some(tool_tx),
                    Some(cancel_cmd),
                )
                .await
            }
        };
        let _ = forward_handle.await;
        let _ = event_tx.send((tc_id, SlotEvent::Done(r))).await;
    });

    ExecSlot {
        tool_call_id,
        conversation,
        kind: ToolKind::Command { pid: None },
        workspace_ordered: true,
        label: summary,
        cancel,
        nested_permissions: Arc::new(Mutex::new(HashMap::new())),
    }
}

pub(crate) struct SlotDispatchArgs {
    pub(crate) capacity: crate::WorkspaceExecutionHost,
    pub(crate) output: Option<RetainedOutputSpec>,
    pub(crate) tool_name: String,
    pub(crate) tool_id: String,
    pub(crate) conversation: Option<crate::conversation::address::ConversationAddress>,
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

pub(crate) struct RetainedOutputSpec {
    pub(crate) dir: PathBuf,
    pub(crate) id: String,
    pub(crate) limits: crate::tool::builtin::run_command::output::OutputLimits,
    pub(crate) retention: crate::session::runtime::OutputReservation,
}

pub(crate) fn dispatch_tool_slot(args: SlotDispatchArgs) -> (ExecSlot, ToolKind) {
    if args.tool_name == "run_command" {
        let slot = spawn_command_slot(
            args.tool_id,
            args.conversation,
            args.args,
            args.summary,
            args.workspace,
            args.event_tx,
            args.output,
            args.capacity,
        );
        (slot, ToolKind::Command { pid: None })
    } else if args.tool_name == "fetch_url" {
        (spawn_fetch_slot(args), ToolKind::Simple)
    } else {
        let slot = spawn_simple_slot(
            args.tool_id,
            args.conversation,
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
            args.capacity,
        );
        (slot, ToolKind::Simple)
    }
}

fn spawn_fetch_slot(args: SlotDispatchArgs) -> ExecSlot {
    let SlotDispatchArgs {
        output,
        tool_id,
        conversation,
        args,
        summary,
        workspace,
        event_tx,
        ..
    } = args;
    let cancel = Arc::new(Notify::new());
    let fetch_cancel = cancel.clone();
    let finished_id = tool_id.clone();
    tokio::spawn(async move {
        let retention = output
            .as_ref()
            .map(|output| crate::tool::builtin::FetchRetention {
                dir: &output.dir,
                id: &output.id,
                limits: output.limits,
            });
        let result =
            crate::tool::builtin::fetch_url(&args, &workspace, retention, &fetch_cancel).await;
        if let Some(mut output) = output {
            if result
                .structured
                .as_ref()
                .is_some_and(|facts| facts["output_ref"].is_object())
            {
                output.retention.keep();
            }
        }
        let _ = event_tx.send((finished_id, SlotEvent::Done(result))).await;
    });
    ExecSlot {
        tool_call_id: tool_id,
        conversation,
        kind: ToolKind::Simple,
        workspace_ordered: false,
        label: summary,
        cancel,
        nested_permissions: Arc::new(Mutex::new(HashMap::new())),
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
        UiEvent::ModelRequest {
            conversation,
            turn,
            request_id,
            request_ordinal,
            ..
        } => Some(ToolEvent::ModelRequest {
            conversation,
            turn,
            request_id,
            request_ordinal,
        }),
        UiEvent::ModelRecovery { info } => Some(ToolEvent::ModelRecovery { info }),
        UiEvent::Done { .. } => None,
        UiEvent::TurnStart { .. }
        | UiEvent::Log { .. }
        | UiEvent::Cancelled { .. }
        | UiEvent::ModelProgress { .. } => None,
    }
}

fn agent_error_event(summary: String) -> SlotEvent {
    SlotEvent::Done(ToolResultEnvelope::error(summary, ""))
}

fn cancelled_slot(summary: impl Into<String>) -> SlotEvent {
    SlotEvent::Done(ToolResultEnvelope {
        structured: None,
        ..ToolResultEnvelope::cancelled(summary)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_event(
        id: u64,
        dir: &std::path::Path,
        conversation: Option<&str>,
        content: &str,
        start_line: usize,
    ) -> StoredEvent {
        use sha2::{Digest, Sha256};

        let canonical = dir.join("visible.txt").canonicalize().unwrap();
        let raw_text = content.lines().nth(start_line - 1).unwrap();
        StoredEvent {
            id,
            payload: crate::event::EventPayload::ToolResult {
                turn: 1,
                ts: "ts".to_string(),
                conversation: conversation.map(str::to_string),
                tool_call_id: format!("read_{id}"),
                status: "ok".to_string(),
                summary: "read".to_string(),
                model_content: format!("{start_line}\t{raw_text}"),
                truncated: false,
                files_read: Vec::new(),
                files_changed: Vec::new(),
                commands_run: Vec::new(),
                memory_changed: None,
                structured: Some(serde_json::json!({
                    "kind": "file_content",
                    "canonical_path": canonical.to_string_lossy(),
                    "content_hash": format!("sha256:{:x}", Sha256::digest(content.as_bytes())),
                    "raw_text": raw_text,
                    "read_event_id": id,
                    "start_line": start_line,
                    "line_count": 1,
                    "total_lines": content.lines().count(),
                    "is_full_file_snapshot": false,
                    "cached": false,
                })),
            },
        }
    }

    fn test_config() -> std::sync::Arc<crate::config::Config> {
        let file: crate::config::ConfigFile =
            toml::from_str(crate::config::generate_default()).unwrap();
        std::sync::Arc::new(file.resolve().unwrap())
    }

    #[test]
    fn workspace_ordering_is_limited_to_snapshot_and_mutator_slots() {
        for tool_name in [
            "read_file",
            "edit_file",
            "write_file",
            "remember_memory",
            "forget_memory",
            "run_command",
            "agent",
        ] {
            assert!(requires_workspace_ordering(tool_name), "{tool_name}");
        }
        for tool_name in ["find_files", "search_text", "fetch_url", "query_session"] {
            assert!(!requires_workspace_ordering(tool_name), "{tool_name}");
        }
    }

    #[test]
    fn agent_error_event_has_stable_structured_marker() {
        let SlotEvent::Done(result) = agent_error_event("agent failed".to_string()) else {
            panic!("expected terminal agent event");
        };

        assert_eq!("error", result.status);
        assert!(!result.truncated);
        assert_eq!(
            Some(serde_json::json!({"kind": "error"})),
            result.structured
        );
    }

    #[tokio::test]
    async fn simple_slot_scopes_file_snapshot_to_active_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("visible.txt");
        let content = "alpha\nalpha\n";
        std::fs::write(&path, content).unwrap();
        let main_read = read_event(1, dir.path(), None, content, 1);
        let review_read = read_event(2, dir.path(), Some("review"), content, 2);
        let review = crate::conversation::address::ConversationAddress::parse("review").unwrap();
        let args = serde_json::json!({
            "path": "visible.txt",
            "old_text": "alpha",
            "new_text": "omega",
            "brief": "rename visible text"
        });
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(2);

        spawn_simple_slot(
            "edit_main_snapshot".to_string(),
            Some(review.clone()),
            "edit_file".to_string(),
            args.clone(),
            "edit".to_string(),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            vec![main_read.clone()],
            event_tx.clone(),
            test_config(),
            crate::prompt::builtin_prompt_catalog(),
            dir.path().join("events.jsonl"),
            crate::WorkspaceExecutionHost::default(),
        );
        let (_, denied) = event_rx.recv().await.unwrap();
        let SlotEvent::Done(result) = denied else {
            panic!("expected completed slot");
        };
        assert_eq!("error", result.status);
        assert_eq!(content, std::fs::read_to_string(&path).unwrap());

        spawn_simple_slot(
            "edit_review_snapshot".to_string(),
            Some(review),
            "edit_file".to_string(),
            args,
            "edit".to_string(),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            vec![main_read, review_read],
            event_tx,
            test_config(),
            crate::prompt::builtin_prompt_catalog(),
            dir.path().join("events.jsonl"),
            crate::WorkspaceExecutionHost::default(),
        );
        let (_, allowed) = event_rx.recv().await.unwrap();
        let SlotEvent::Done(result) = allowed else {
            panic!("expected completed slot");
        };

        assert_eq!("ok", result.status);
        assert_eq!("alpha\nomega\n", std::fs::read_to_string(path).unwrap());
    }
}
