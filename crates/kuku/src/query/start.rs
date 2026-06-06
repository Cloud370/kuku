use std::sync::Arc;

use crate::error::{Error, Result};
use crate::event::{EventPayload, EventStore};
use crate::log::{
    runtime_log_path, session_log_path, BufferedLogWriter, LogLevel, LogRecord, LogScope,
};
use crate::session::{
    current_workspace, kuku_home, new_session_id, project_policy_path, session_events_path,
    validate_session_id,
};

use super::helpers::{next_turn, now_timestamp, validate_existing_session};
use super::types::{
    PendingPermission, PendingRun, Query, QueuedToolCall, Run, RunOutput, RunState, UiEvent,
};

impl Query {
    pub async fn start(self) -> Result<Run> {
        self.start_session().await
    }

    async fn start_session(mut self) -> Result<Run> {
        self.validate()?;

        let kuku_home = kuku_home()?;

        let workspace = match self.workspace_path.take() {
            Some(path) => path,
            None => current_workspace()?,
        };

        let config: Arc<crate::config::Config> = match (self.config_obj.take(), &self.config_path) {
            (Some(cfg), _) => Arc::new(cfg),
            (None, Some(path)) => {
                let file = crate::config::load_and_patch_config(path)?;
                Arc::new(file.resolve()?)
            }
            (None, None) => {
                return Err(Error::MissingProviderConfig(
                    "no config provided; set .config_path() or .config()".to_string(),
                ));
            }
        };
        let handoff_keep_turns = config.handoff().keep_turns;

        let session_id = match self.session_id.as_deref() {
            Some(session_id) => {
                validate_session_id(session_id)?;
                session_id.to_string()
            }
            None => new_session_id(),
        };
        validate_session_id(&session_id)?;

        let events_path = session_events_path(&kuku_home, &workspace, &session_id)?;
        let policy_path = project_policy_path(&kuku_home, &workspace)?;
        let existing_events = EventStore::replay(&events_path)?;
        validate_existing_session(&existing_events)?;
        let is_new_session = existing_events.is_empty();
        let lifecycle = if is_new_session {
            None
        } else {
            Some(super::lifecycle::reduce_lifecycle(&existing_events))
        };
        reject_interrupted_open_tools(lifecycle.as_ref(), &session_id)?;
        let resumed_permission = lifecycle
            .as_ref()
            .and_then(|state| state.pending_permissions.first());
        let turn = resumed_permission
            .map(|pending| pending.turn)
            .unwrap_or_else(|| next_turn(&existing_events));

        let mut store = EventStore::open(&events_path)?;
        if is_new_session {
            let created_at = now_timestamp()?;
            store.append(EventPayload::SessionMeta {
                ts: created_at.clone(),
                schema_version: 1,
                session_id: session_id.clone(),
                created_at,
                kuku_version: env!("CARGO_PKG_VERSION").to_string(),
            })?;
        }

        let prompts_dir = self.prompts_dir.take();
        let skill_body = self.skill_body.take();
        let subagent_registry = self.subagent_registry.clone();
        let tool_registry_override = self.tool_registry_override.clone();

        let plugin_registry_opt = if config.plugin.enabled {
            Some(
                crate::plugin::PluginRegistry::builder()
                    .load_packages(&kuku_home, &workspace)?
                    .build()?,
            )
        } else {
            None
        };

        if resumed_permission.is_none() {
            store.append(EventPayload::TurnStart {
                turn,
                ts: now_timestamp()?,
            })?;
            store.append(EventPayload::UserInput {
                turn,
                ts: now_timestamp()?,
                text: self.prompt.clone(),
            })?;

            if let (Ok(session_log_path), Ok(ts)) =
                (session_log_path(&kuku_home, &session_id), now_timestamp())
            {
                let mut record = LogRecord::new(ts, LogLevel::Info, LogScope::Session);
                record.kind = "session.turn_start".to_string();
                record.message = format!("starting turn {turn}");
                record.session_id = Some(session_id.clone());
                record.run_id = Some(session_id.clone());
                record.workspace = Some(workspace.display().to_string());
                record.turn = Some(turn);
                let mut session_log_writer =
                    BufferedLogWriter::with_flush_every(session_log_path, 1);
                let _ = session_log_writer.push(record);
            }
        }

        if let (None, Some(ref plugin_reg)) = (&resumed_permission, &plugin_registry_opt) {
            let hooks = plugin_reg.hooks_for(crate::plugin::HookEvent::SessionStart);
            if !hooks.is_empty() {
                let input = crate::plugin::executor::HookInput {
                    event: "session.start".to_string(),
                    session_dir: events_path.parent().unwrap().to_string_lossy().to_string(),
                    extra: serde_json::json!({}),
                };
                let session_dir = events_path.parent().unwrap().to_path_buf();
                let results =
                    crate::plugin::executor::execute_hooks(hooks, &input, &session_dir, &workspace)
                        .await?;
                for r in &results {
                    if r.output.block || r.exit_code == 2 {
                        let reason = if r.stderr.is_empty() {
                            "blocked by plugin hook".to_string()
                        } else {
                            r.stderr.clone()
                        };
                        return Err(crate::error::Error::PluginValidation(reason));
                    }
                }
                super::tool_exec::record_plugin_hooks(
                    &events_path,
                    turn,
                    "session.start",
                    &results,
                )?;
            }
        }

        let skill_registry = if self.disable_skills {
            (None, None)
        } else {
            let builder = crate::skill::registry::SkillRegistry::builder()
                .build_with_discovery(&workspace, &config.discovery);
            match builder {
                Ok(mut b) => {
                    if let Some(ref reg) = plugin_registry_opt {
                        for (skill_dir, tier) in reg.skill_dirs() {
                            b = b.load_from_dir(skill_dir, (*tier).into())?;
                        }
                    }
                    let reg = b.build();
                    let hash = reg.hash().to_string();
                    (Some(reg), Some(hash))
                }
                Err(_) => (None, None),
            }
        };
        let plugin_registry = plugin_registry_opt.map(std::sync::Arc::new);
        let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());
        let lock_path = crate::session::session_lock_path(&kuku_home, &workspace, &session_id);
        crate::session::acquire_lock(&lock_path)?;
        let (slot_event_tx, slot_event_rx) =
            tokio::sync::mpsc::channel::<(String, super::types::SlotEvent)>(256);
        let catalog = if let Some(dir) = &prompts_dir {
            crate::prompt::PromptCatalog::load_from_dir(dir)
                .unwrap_or_else(|_| crate::prompt::builtin_prompt_catalog())
        } else {
            crate::prompt::builtin_prompt_catalog()
        };
        let logs_config = config.logs();
        let runtime_log_path =
            runtime_log_path(&kuku_home, &super::helpers::current_date_string())?;
        maybe_prune_logs_on_startup(&kuku_home, &logs_config, &runtime_log_path);
        let runtime_log_writer = BufferedLogWriter::new(&runtime_log_path).with_post_flush_every(
            32,
            Box::new({
                let kuku_home = kuku_home.clone();
                let active_path = runtime_log_path.clone();
                move || {
                    let _ = crate::log::prune_logs(
                        &kuku_home,
                        &logs_config,
                        std::time::SystemTime::now(),
                        crate::log::PruneOptions::default().with_active_path(active_path.clone()),
                    );
                    Ok(())
                }
            }),
        );

        let resumed_state = resumed_state(lifecycle.as_ref(), turn);

        let pending = PendingRun {
            session_id: session_id.clone(),
            query: self,
            events_path,
            kuku_home,
            workspace,
            policy_path,
            turn,
            request_num: resumed_request_num(&existing_events, turn),
            cumulative: super::types::CumulativeUsage::default(),
            resolved: None,
            queued_tool_calls: resumed_state.queued_tool_calls,
            resumed_permission_requests: resumed_state.resumed_permission_requests,
            pending_events: std::collections::VecDeque::new(),
            pending_error: None,
            config,
            prompts_dir,
            subagent_registry,
            skill_body,
            skill_registry: skill_registry.0,
            skill_content_hash: skill_registry.1,
            child_session_count: 0,
            tool_registry_override,
            catalog,
            cancel_token: cancel_token.clone(),
            handoff_triggered: false,
            handoff_keep_turns,
            plugin_registry,
            hook_context: Vec::new(),
            force_continue_count: 0,
            model_request_count: resumed_model_request_count(&existing_events, turn),
            tool_rounds: resumed_tool_rounds(&existing_events, turn),
            tool_calls: 0,
            tool_names: Vec::new(),
            tool_denied: 0,
            tool_errors: 0,
            thinking_duration_ms: 0,
            runtime_log_writer,
        };

        let state = if let Some(request) = resumed_state.first_request {
            RunState::WaitingForPermission(Box::new(PendingPermission { pending, request }))
        } else {
            RunState::Pending(Box::new(pending))
        };

        Ok(Run {
            session_id: session_id.clone(),
            state,
            slots: std::collections::HashMap::new(),
            slot_event_tx,
            slot_event_rx,
            cancel_token,
            lock_path,
            deferred_runtime_logs: std::collections::VecDeque::new(),
        })
    }

    pub async fn run(self) -> Result<RunOutput> {
        let mut run = self.start_session().await?;
        loop {
            match run.next().await? {
                Some(UiEvent::PermissionRequested { .. }) => {
                    run.deny_pending().await?;
                }
                Some(UiEvent::Done { output, .. }) => return Ok(output),
                Some(_) => continue,
                None => {
                    return Err(Error::InvalidEventStream(
                        "run ended without producing Done".to_string(),
                    ))
                }
            }
        }
    }

    pub async fn run_with_permission_choice(
        self,
        choice: super::types::PermissionChoice,
    ) -> Result<RunOutput> {
        let mut run = self.start_session().await?;
        loop {
            match run.next().await? {
                Some(UiEvent::PermissionRequested { request }) => {
                    run.decide(&request.id, choice, None).await?;
                }
                Some(UiEvent::Done { output, .. }) => return Ok(output),
                Some(_) => continue,
                None => {
                    return Err(Error::InvalidEventStream(
                        "run ended without producing Done".to_string(),
                    ))
                }
            }
        }
    }
}

struct ResumedState {
    queued_tool_calls: std::collections::VecDeque<QueuedToolCall>,
    resumed_permission_requests: std::collections::VecDeque<super::types::PermissionRequest>,
    first_request: Option<super::types::PermissionRequest>,
}

fn resumed_state(lifecycle: Option<&super::lifecycle::LifecycleState>, turn: u64) -> ResumedState {
    let mut queued_tool_calls = std::collections::VecDeque::new();
    let mut resumed_permission_requests = std::collections::VecDeque::new();
    let Some(lifecycle) = lifecycle else {
        return ResumedState {
            queued_tool_calls,
            resumed_permission_requests,
            first_request: None,
        };
    };

    let mut first_request = None;
    for pending in lifecycle
        .pending_permissions
        .iter()
        .filter(|pending| pending.turn == turn)
    {
        if first_request.is_none() {
            first_request = Some(pending.request.clone());
        } else {
            resumed_permission_requests.push_back(pending.request.clone());
        }
        queued_tool_calls.push_back(QueuedToolCall {
            tool_call: pending.tool_call.clone(),
            display_summary: pending.request.summary.clone(),
        });
    }

    ResumedState {
        queued_tool_calls,
        resumed_permission_requests,
        first_request,
    }
}

fn reject_interrupted_open_tools(
    lifecycle: Option<&super::lifecycle::LifecycleState>,
    session_id: &str,
) -> Result<()> {
    let Some(lifecycle) = lifecycle else {
        return Ok(());
    };
    let Some(open_tool) = lifecycle.open_tools.first() else {
        return Ok(());
    };

    Err(Error::InterruptedOpenTool(format!(
        "session {session_id} has unresolved tool call {} from turn {}; review the session before resuming",
        open_tool.tool_call.id, open_tool.turn
    )))
}

fn resumed_model_request_count(events: &[crate::event::StoredEvent], turn: u64) -> u64 {
    events
        .iter()
        .filter(|event| {
            matches!(
                &event.payload,
                EventPayload::ModelResponse { turn: event_turn, .. }
                    | EventPayload::ModelError { turn: event_turn, .. }
                    if *event_turn == turn
            )
        })
        .count() as u64
}

fn resumed_tool_rounds(events: &[crate::event::StoredEvent], turn: u64) -> u64 {
    let mut request_ids = Vec::<&str>::new();
    for event in events {
        if let EventPayload::ToolCall {
            turn: event_turn,
            request_id,
            ..
        } = &event.payload
        {
            if *event_turn == turn && !request_ids.iter().any(|id| *id == request_id) {
                request_ids.push(request_id);
            }
        }
    }
    request_ids.len() as u64
}

fn resumed_request_num(events: &[crate::event::StoredEvent], turn: u64) -> u64 {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ModelResponse {
                turn: event_turn,
                request_id,
                ..
            }
            | EventPayload::ModelError {
                turn: event_turn,
                request_id,
                ..
            } if *event_turn == turn => Some(request_num_from_id(request_id)),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

fn request_num_from_id(request_id: &str) -> u64 {
    request_id
        .strip_prefix("req_")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
}

fn maybe_prune_logs_on_startup(
    kuku_home: &std::path::Path,
    logs_config: &crate::config::LogsConfig,
    active_path: &std::path::Path,
) {
    static STARTUP_PRUNE_GATE: std::sync::OnceLock<std::sync::Mutex<crate::log::StartupPruneGate>> =
        std::sync::OnceLock::new();
    let gate = STARTUP_PRUNE_GATE.get_or_init(|| {
        std::sync::Mutex::new(crate::log::StartupPruneGate::new(
            std::time::Duration::from_secs(24 * 60 * 60),
        ))
    });
    let mut gate = gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if !gate.should_prune(kuku_home, std::time::SystemTime::now()) {
        return;
    }
    let kuku_home = kuku_home.to_path_buf();
    let logs_config = logs_config.clone();
    let active_path = active_path.to_path_buf();
    std::thread::spawn(move || {
        let _ = crate::log::prune_logs(
            &kuku_home,
            &logs_config,
            std::time::SystemTime::now(),
            startup_prune_options(&active_path),
        );
    });
}

fn startup_prune_options(active_path: &std::path::Path) -> crate::log::PruneOptions {
    crate::log::PruneOptions::default().with_active_path(active_path.to_path_buf())
}

#[cfg(test)]
mod startup_prune_tests {
    use std::path::Path;

    #[test]
    fn startup_prune_options_exclude_active_runtime_path() {
        let active_path = Path::new("/tmp/kuku/logs/runtime/2026-06-06.jsonl");

        let options = super::startup_prune_options(active_path);

        assert!(options.excludes_active_path(active_path));
    }
}
