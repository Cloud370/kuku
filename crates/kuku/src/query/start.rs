use std::collections::BTreeMap;
use std::path::PathBuf;
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
use crate::skill::session::{
    build_registry_snapshot, build_registry_snapshot_with_capability,
    previous_snapshot_before_turn, restore_turn_snapshot, TurnSkillSnapshot,
};

use super::helpers::{
    append_interrupted_active_turn, append_message_user_with_sender, append_turn_started,
    next_turn, now_timestamp, validate_existing_session,
};
use super::types::{
    PendingPermission, PendingRun, Query, QueuedToolCall, Run, RunOutput, RunState, UiEvent,
};

struct StartupLockGuard {
    lock_path: std::path::PathBuf,
    held: bool,
}

impl StartupLockGuard {
    fn acquire(lock_path: std::path::PathBuf) -> Result<Self> {
        crate::session::acquire_lock(&lock_path)?;
        Ok(Self {
            lock_path,
            held: true,
        })
    }

    fn into_path(mut self) -> std::path::PathBuf {
        self.held = false;
        self.lock_path.clone()
    }
}

impl Drop for StartupLockGuard {
    fn drop(&mut self) {
        if self.held {
            crate::session::release_lock(&self.lock_path);
        }
    }
}

fn has_builder_provider_config(query: &Query) -> bool {
    query.provider.is_some()
        && query.model.is_some()
        && query.base_url.is_some()
        && query.api_key.is_some()
}

fn synthetic_builder_config() -> crate::config::Config {
    crate::config::Config {
        tiers: BTreeMap::new(),
        providers: BTreeMap::new(),
        default_tier: String::new(),
        discovery: crate::config::DiscoveryConfig::default(),
        handoff: crate::config::HandoffConfig::default(),
        logs: crate::config::LogsConfig::default(),
        plugin: crate::config::PluginConfig::default(),
        update: crate::config::UpdateConfig::default(),
    }
}

impl Query {
    pub async fn start(self) -> Result<Run> {
        self.start_session_with_lock(true).await
    }

    pub(crate) async fn start_nested(self) -> Result<Run> {
        self.start_session_with_lock(false).await
    }

    async fn start_session_with_lock(mut self, acquire_lock: bool) -> Result<Run> {
        self.validate()?;

        let task_context = self.task_context.clone();

        let kuku_home = match self.captured_kuku_home.take() {
            Some(path) => path,
            None => kuku_home()?,
        };

        let workspace = match task_context.as_ref() {
            Some(context) => {
                if context.workspace.workspace_id() != context.execution_scope.workspace_id.as_str()
                {
                    return Err(Error::InvalidTaskContext(
                        "workspace capability does not match execution scope".to_string(),
                    ));
                }
                context.workspace.verify_identity()?;
                PathBuf::from(context.execution_scope.workspace_id.as_str())
            }
            None => match self.workspace_path.take() {
                Some(path) => path,
                None => current_workspace()?,
            },
        };

        let config: Arc<crate::config::Config> = match (self.config_obj.take(), &self.config_path) {
            (Some(cfg), _) => Arc::new(cfg),
            (None, Some(path)) => {
                let file = crate::config::load_and_patch_config(path)?;
                Arc::new(file.resolve()?)
            }
            (None, None) => {
                if has_builder_provider_config(&self) {
                    Arc::new(synthetic_builder_config())
                } else {
                    return Err(Error::MissingProviderConfig(
                        "no config provided; set .config_path() or .config()".to_string(),
                    ));
                }
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

        let lock_path = crate::session::session_lock_path(&kuku_home, &workspace, &session_id);
        let run_lock_path = if task_context.is_some() {
            std::path::PathBuf::new()
        } else if acquire_lock {
            StartupLockGuard::acquire(lock_path.clone())?.into_path()
        } else {
            lock_path.with_extension("nested")
        };
        let events_path = match task_context.as_ref() {
            Some(context) => context.event_store.path().to_path_buf(),
            None => session_events_path(&kuku_home, &workspace, &session_id)?,
        };
        let policy_path = project_policy_path(&kuku_home, &workspace)?;
        let existing_events = match task_context.as_ref() {
            Some(context) => context.event_store.read_all()?,
            None => EventStore::replay(&events_path)?,
        };
        if let Some(context) = task_context.as_ref() {
            validate_task_ledger(&existing_events, &context.execution_scope)?;
        } else {
            validate_existing_session(&existing_events)?;
        }
        let is_new_session = task_context.is_none() && existing_events.is_empty();
        let lifecycle = if is_new_session {
            None
        } else {
            Some(super::lifecycle::reduce_lifecycle(&existing_events))
        };
        let conversation = self.conversation.clone();
        reject_interrupted_open_tools(lifecycle.as_ref(), &session_id, &conversation)?;
        let resumed_permission = lifecycle
            .as_ref()
            .and_then(|state| state.pending_permissions.first());
        let mut bootstrap_skill = if resumed_permission.is_some() {
            None
        } else {
            self.bootstrap_skill.take()
        };
        let turn = resumed_permission
            .map(|pending| pending.turn)
            .unwrap_or_else(|| next_turn(&existing_events));
        let execution_scope = match task_context.as_ref() {
            Some(context) => context.execution_scope.clone(),
            None => match self.execution_scope.clone() {
                Some(scope) => scope,
                None => crate::event::ExecutionScope {
                    workspace_id: crate::event::WorkspaceId::try_new()?,
                    task_id: crate::event::TaskId::try_new()?,
                    run_id: crate::event::RunId::try_new()?,
                    turn_id: crate::event::TurnId::try_new()?,
                    conversation_id: crate::event::ConversationId::try_new()?,
                    turn_index: turn,
                },
            },
        };
        self.execution_scope = Some(execution_scope.clone());
        let restored_skill_snapshot =
            restore_turn_snapshot(&existing_events, conversation.as_str(), turn);
        let capability_skill_registry = if self.disable_skills || restored_skill_snapshot.is_some()
        {
            None
        } else {
            task_context
                .as_ref()
                .map(|context| {
                    build_registry_snapshot_with_capability(
                        context.workspace.as_ref(),
                        &config.discovery,
                        &context.selected_skills,
                    )
                })
                .transpose()?
        };
        let mut store = match task_context.as_ref() {
            Some(context) => context.event_store.clone(),
            None => EventStore::open(&events_path)?,
        };
        if is_new_session {
            let created_at = now_timestamp()?;
            store.append(EventPayload::SessionCreated {
                ts: created_at.clone(),
                schema_version: 2,
                session_id: session_id.clone(),
                created_at,
                kuku_version: env!("CARGO_PKG_VERSION").to_string(),
            })?;
        }
        let has_conversation = existing_events.iter().any(|event| {
            matches!(
                &event.payload,
                EventPayload::ConversationOpened { conversation, .. }
                    if conversation == self.conversation.as_str()
            )
        });
        if !has_conversation {
            store.append(EventPayload::ConversationOpened {
                ts: now_timestamp()?,
                conversation: self.conversation.as_str().to_string(),
            })?;
            if let Some(binding_id) = self.agent_binding_id.as_ref() {
                store.append(EventPayload::ConversationBound {
                    ts: now_timestamp()?,
                    conversation: self.conversation.as_str().to_string(),
                    binding_id: binding_id.clone(),
                })?;
            }
        }

        let prompts_dir = self.prompts_dir.take();
        let agent_registry = self.agent_registry.clone();
        let tool_registry_override = self.tool_registry_override.clone();

        let plugin_registry_opt = if task_context.is_some() {
            None
        } else if config.plugin.enabled {
            Some(
                crate::plugin::PluginRegistry::builder()
                    .load_packages(&kuku_home, &workspace)?
                    .build()?,
            )
        } else {
            None
        };

        if resumed_permission.is_none() {
            append_interrupted_active_turn(
                &store,
                &existing_events,
                &conversation,
                "resume_before_new_turn",
            )?;
            append_turn_started(&store, &execution_scope, &conversation, turn)?;
            append_message_user_with_sender(
                &store,
                &execution_scope,
                &conversation,
                turn,
                &self.prompt,
                self.message_from.as_ref(),
                self.via_tool_call_id.as_deref(),
            )?;

            if let (Ok(session_log_path), Ok(ts)) =
                (session_log_path(&kuku_home, &session_id), now_timestamp())
            {
                let mut record = LogRecord::new(ts, LogLevel::Info, LogScope::Session);
                record.kind = "session.turn_start".to_string();
                record.message = format!("starting turn {turn}");
                record.session_id = Some(session_id.clone());
                record.run_id = Some(execution_scope.run_id.to_string());
                record.workspace = Some(workspace.display().to_string());
                record.turn = Some(turn);
                let mut session_log_writer =
                    BufferedLogWriter::with_flush_every(session_log_path, 1);
                let _ = session_log_writer.push(record);
            }
        }

        let previous_skill_snapshot =
            previous_snapshot_before_turn(&existing_events, conversation.as_str(), turn);
        let (skill_registry, previous_skill_registry) = if self.disable_skills {
            (None, None)
        } else if let Some(snapshot) = restored_skill_snapshot {
            bootstrap_skill = restore_bootstrap_skill(&snapshot).or(bootstrap_skill);
            (
                Some(snapshot.registry),
                previous_skill_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.registry.clone()),
            )
        } else {
            let discovered = match capability_skill_registry {
                Some(registry) => Ok(registry),
                None => build_registry_snapshot(
                    &workspace,
                    &config.discovery,
                    plugin_registry_opt.as_ref(),
                ),
            };
            match discovered {
                Ok(registry) => {
                    let (registry, bootstrap_loaded) =
                        if let Some(snapshot) = previous_skill_snapshot.as_ref() {
                            bootstrap_skill = restore_bootstrap_skill(snapshot).or(bootstrap_skill);
                            (snapshot.registry.clone(), snapshot.bootstrap_loaded.clone())
                        } else {
                            (registry, bootstrap_loaded_names(bootstrap_skill.as_ref()))
                        };
                    store.append(EventPayload::ContextSkills {
                        conversation: conversation.as_str().to_string(),
                        turn,
                        ts: now_timestamp()?,
                        registry: serde_json::to_value(&registry)?,
                        bootstrap_loaded,
                    })?;
                    (
                        Some(registry),
                        previous_skill_snapshot.map(|snapshot| snapshot.registry),
                    )
                }
                Err(_) => (None, None),
            }
        };

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

        let plugin_registry = plugin_registry_opt.map(std::sync::Arc::new);
        let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());
        let (slot_event_tx, slot_event_rx) =
            tokio::sync::mpsc::channel::<(String, super::types::SlotEvent)>(256);
        let catalog = if let Some(dir) = &prompts_dir {
            crate::prompt::PromptCatalog::load_from_dir(dir).map_err(|e| {
                crate::error::Error::PromptRender(format!(
                    "failed to load prompts from {}: {e}",
                    dir.display()
                ))
            })?
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
        let agent_binding_id = self.agent_binding_id.clone();

        let pending = PendingRun {
            session_id: session_id.clone(),
            conversation: conversation.clone(),
            query: self,
            event_store: store.clone(),
            events_path: events_path.clone(),
            kuku_home,
            workspace,
            workspace_capability: task_context
                .as_ref()
                .map(|context| context.workspace.clone()),
            policy_path,
            turn,
            request_num: resumed_request_num(&existing_events, turn),
            previous_request_id: resumed_previous_request_id(&existing_events, turn),
            request_evidence_recorder: std::sync::Arc::new(match task_context.as_ref() {
                Some(context) => {
                    super::provider::LifecycleOnlyRecorder::from_store(context.event_store.clone())
                }
                None => super::provider::LifecycleOnlyRecorder::new(events_path.clone()),
            }),
            cumulative: super::types::CumulativeUsage::default(),
            resolved: None,
            queued_tool_calls: resumed_state.queued_tool_calls,
            resumed_permission_requests: resumed_state.resumed_permission_requests,
            pending_events: std::collections::VecDeque::new(),
            pending_error: None,
            config,
            prompts_dir,
            agent_registry,
            bootstrap_skill,
            skill_registry,
            previous_skill_registry,
            child_session_count: 0,
            frozen_turn_prefix: super::types::TurnPrefixFreeze::default(),
            agent_binding_id,
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
            execution_scope,
            session_id: session_id.clone(),
            state,
            slots: std::collections::HashMap::new(),
            slot_event_tx,
            slot_event_rx,
            cancel_token,
            lock_path: run_lock_path,
            deferred_runtime_logs: std::collections::VecDeque::new(),
        })
    }

    pub async fn run(self) -> Result<RunOutput> {
        let mut run = self.start_session_with_lock(true).await?;
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
        let mut run = self.start_session_with_lock(true).await?;
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

fn validate_task_ledger(
    events: &[crate::event::StoredEvent],
    scope: &crate::event::ExecutionScope,
) -> Result<()> {
    if events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::SessionCreated { .. }))
    {
        return Err(Error::InvalidEventStream(
            "task ledger must not contain legacy session records".to_string(),
        ));
    }
    let mut task_identity = None;
    let mut active_run = None;
    for event in events {
        let EventPayload::TaskLedger(record) = &event.payload else {
            continue;
        };
        let facts = match record {
            crate::event::TaskLedgerRecord::Control(transaction) => transaction.events(),
            crate::event::TaskLedgerRecord::Activity(batch) => batch.events(),
        };
        for fact in facts {
            match fact {
                crate::event::TaskEvent::TaskCreated {
                    task_id,
                    workspace_id,
                    ..
                } => task_identity = Some((task_id, workspace_id)),
                crate::event::TaskEvent::RunQueued { run }
                | crate::event::TaskEvent::RunStarted { run }
                | crate::event::TaskEvent::RunNeedsAttention { run }
                | crate::event::TaskEvent::RunStopping { run } => active_run = Some(run),
                crate::event::TaskEvent::RunCompleted { run }
                | crate::event::TaskEvent::RunStopped { run }
                | crate::event::TaskEvent::RunFailed { run }
                | crate::event::TaskEvent::RunInterrupted { run }
                    if active_run.is_some_and(|active| active.run_id == run.run_id) =>
                {
                    active_run = None;
                }
                _ => {}
            }
        }
    }
    let Some((task_id, workspace_id)) = task_identity else {
        return Err(Error::InvalidTaskContext(
            "task ledger has no TaskCreated fact".to_string(),
        ));
    };
    if task_id != &scope.task_id || workspace_id != &scope.workspace_id {
        return Err(Error::InvalidTaskContext(
            "task ledger identity does not match execution scope".to_string(),
        ));
    }
    let Some(run) = active_run else {
        return Err(Error::InvalidTaskContext(
            "task ledger has no active Run".to_string(),
        ));
    };
    if run.task_id != scope.task_id || run.run_id != scope.run_id {
        return Err(Error::InvalidTaskContext(
            "active Run does not match execution scope".to_string(),
        ));
    }
    Ok(())
}

fn bootstrap_loaded_names(
    bootstrap_skill: Option<&crate::query::types::BootstrapSkill>,
) -> Vec<String> {
    bootstrap_skill
        .and_then(|skill| skill.name.clone())
        .into_iter()
        .collect()
}

fn restore_bootstrap_skill(
    snapshot: &TurnSkillSnapshot,
) -> Option<crate::query::types::BootstrapSkill> {
    let mut restored = Vec::new();
    for skill_name in &snapshot.bootstrap_loaded {
        let definition = snapshot.registry.get(skill_name)?;
        let skill_dir = definition.source_path.as_deref().unwrap_or("");
        restored.push(format!(
            "<!-- loaded: {skill_dir} -->\n\n{}",
            definition.instructions
        ));
    }

    if restored.is_empty() {
        return None;
    }

    let name = if snapshot.bootstrap_loaded.len() == 1 {
        snapshot.bootstrap_loaded.first().cloned()
    } else {
        None
    };

    Some(crate::query::types::BootstrapSkill {
        name,
        body: restored.join("\n\n"),
    })
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
            request: pending.request_scope.clone(),
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
    conversation: &crate::conversation::address::ConversationAddress,
) -> Result<()> {
    let Some(lifecycle) = lifecycle else {
        return Ok(());
    };
    let Some(open_tool) = lifecycle
        .open_tools
        .iter()
        .find(|open_tool| &open_tool.conversation == conversation)
    else {
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
    let mut request_ids = Vec::<&crate::event::RequestId>::new();
    for event in events {
        if let EventPayload::ToolCall {
            turn: event_turn,
            request,
            ..
        } = &event.payload
        {
            if *event_turn == turn && !request_ids.contains(&&request.request_id) {
                request_ids.push(&request.request_id);
            }
        }
    }
    request_ids.len() as u64
}

fn resumed_request_num(events: &[crate::event::StoredEvent], turn: u64) -> u64 {
    events
        .iter()
        .filter(|event| match &event.payload {
            EventPayload::ModelResponse {
                turn: event_turn, ..
            }
            | EventPayload::ModelError {
                turn: event_turn, ..
            } => *event_turn == turn,
            _ => false,
        })
        .count() as u64
}

fn resumed_previous_request_id(
    events: &[crate::event::StoredEvent],
    turn: u64,
) -> Option<crate::event::RequestId> {
    events.iter().rev().find_map(|event| match &event.payload {
        EventPayload::TaskLedger(crate::event::TaskLedgerRecord::Activity(batch)) => {
            batch.events().iter().rev().find_map(|event| match event {
                crate::event::TaskEvent::RequestStarted(started)
                    if started.scope.execution.turn_index == turn =>
                {
                    Some(started.scope.request_id.clone())
                }
                _ => None,
            })
        }
        _ => None,
    })
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
mod startup_prune_tests;
