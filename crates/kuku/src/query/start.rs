use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::event::{EventPayload, EventStore};
use crate::log::{
    BufferedLogWriter, LogLevel, LogRecord, LogScope, runtime_log_path, session_log_path,
};
use crate::session::{
    current_workspace, kuku_home, new_session_id, session_events_path, validate_session_id,
};
use crate::skill::bundle;
use crate::skill::session::{
    TurnSkillSnapshot, build_registry_snapshot, previous_snapshot_before_turn,
    restore_turn_snapshot,
};

use super::helpers::{
    append_interrupted_active_turn, append_message_user_with_sender, append_turn_started,
    next_turn, now_timestamp, validate_existing_session,
};
use super::types::{
    PendingPermission, PendingRun, Query, QueuedToolCall, Run, RunOutput, RunState,
    ToolPreparation, UiEvent,
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
        models: BTreeMap::new(),
        providers: BTreeMap::new(),
        default_model: crate::config::ModelRef::default(),
        discovery: crate::config::DiscoveryConfig::default(),
        handoff: crate::config::HandoffConfig::default(),
        logs: crate::config::LogsConfig::default(),
        plugin: crate::config::PluginConfig::default(),
        update: crate::config::UpdateConfig::default(),
    }
}

impl Query {
    pub async fn start(self) -> Result<Run> {
        self.start_session_with_lock(true, false, None).await
    }

    pub(crate) async fn start_nested(
        self,
        stop: &tokio::sync::watch::Receiver<bool>,
    ) -> Result<Run> {
        self.start_session_with_lock(false, false, Some(stop)).await
    }

    pub(crate) async fn compact_context(
        self,
        closed_turn: bool,
        cancel: std::sync::Arc<tokio::sync::Notify>,
    ) -> Result<()> {
        let mut run = self.start_session_with_lock(false, true, None).await?;
        let RunState::Pending(mut pending) =
            std::mem::replace(&mut run.state, RunState::Done(None))
        else {
            return Err(Error::InvalidArgument(
                "this Agent has work that must settle before compaction".into(),
            ));
        };
        pending.cancel_token = cancel;
        super::provider::compact_context(*pending, closed_turn).await
    }

    async fn start_session_with_lock(
        mut self,
        acquire_lock: bool,
        compact_only: bool,
        stop: Option<&tokio::sync::watch::Receiver<bool>>,
    ) -> Result<Run> {
        self.validate()?;

        let kuku_home = match self.captured_kuku_home.take() {
            Some(path) => path,
            None => kuku_home()?,
        };

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
        let startup_lock = acquire_lock
            .then(|| StartupLockGuard::acquire(lock_path.clone()))
            .transpose()?;
        let events_path = match &self.events_path_override {
            Some(path) => path.clone(),
            None => session_events_path(&kuku_home, &workspace, &session_id)?,
        };
        let runtime = self
            .runtime
            .as_ref()
            .map(|runtime| {
                runtime
                    .upgrade()
                    .map(|inner| crate::Session { inner })
                    .ok_or_else(|| Error::InvalidArgument("session resources are closed".into()))
            })
            .transpose()?;
        let existing_events = if let Some(runtime) = &runtime {
            runtime.read_agent_events(self.conversation.as_str())?
        } else {
            EventStore::replay(&events_path)?
        };
        validate_existing_session(&existing_events)?;
        let is_new_session = existing_events.is_empty();
        let lifecycle = if is_new_session {
            None
        } else {
            Some(super::lifecycle::reduce_lifecycle(&existing_events))
        };
        let conversation = self.conversation.clone();
        let resumed_permission = lifecycle.as_ref().and_then(|state| {
            state
                .pending_permissions
                .iter()
                .find(|pending| pending.request.conversation == conversation)
        });
        let resumable_batch = resumed_permission
            .filter(|_| self.runtime.is_some())
            .map(|pending| pending.turn);
        reject_interrupted_open_tools(
            lifecycle.as_ref(),
            &session_id,
            &conversation,
            resumable_batch,
        )?;
        let mut bootstrap_skill = if resumed_permission.is_some() {
            None
        } else {
            self.bootstrap_skill.take()
        };
        if self.disable_skills
            && bootstrap_skill
                .as_ref()
                .is_some_and(|skill| skill.name.as_deref() == Some(bundle::NAME))
        {
            bootstrap_skill = None;
        }
        let turn = resumed_permission
            .map(|pending| pending.turn)
            .unwrap_or_else(|| {
                self.turn_override
                    .unwrap_or_else(|| next_turn(&existing_events))
            });
        let mut store = EventStore::open(&events_path)?;
        if is_new_session {
            let created_at = now_timestamp()?;
            store.append(EventPayload::SessionCreated {
                ts: created_at.clone(),
                schema_version: 2,
                session_id: session_id.clone(),
                created_at,
                kuku_version: env!("CARGO_PKG_VERSION").to_string(),
                default_model: None,
            })?;
        }
        let prompts_dir = self.prompts_dir.take();
        let tool_registry_override = self.tool_registry_override.clone();

        let plugin_registry_opt = if config.plugin.enabled && !compact_only {
            Some(
                crate::plugin::PluginRegistry::builder()
                    .load_packages(&kuku_home, &workspace)?
                    .build()?,
            )
        } else {
            None
        };

        let needs_skill_paths = !self.input_skills.is_empty()
            || bootstrap_skill.is_some()
            || tool_registry_override.as_ref().is_none_or(|tools| {
                tools
                    .iter()
                    .any(|tool| super::helpers::is_skill_tool(&tool.name))
            });
        let mut skill_record = None;
        let mut skill_catalog_id = None;
        let (skill_registry, previous_skill_registry) = if self.disable_skills || compact_only {
            (None, None)
        } else {
            // A paused Turn keeps its original discovery snapshot. A new Turn
            // discovers the current workspace instead of recycling that snapshot.
            let previous = self
                .runtime
                .is_none()
                .then(|| {
                    previous_snapshot_before_turn(
                        &kuku_home,
                        &existing_events,
                        conversation.as_str(),
                        turn,
                    )
                })
                .transpose()?
                .flatten();
            let registry = if let Some(snapshot) =
                restore_turn_snapshot(&kuku_home, &existing_events, conversation.as_str(), turn)?
            {
                let mut snapshot = snapshot;
                if needs_skill_paths && snapshot.registry.materialize_docs(&kuku_home, stop).await?
                {
                    let catalog = if let Some(runtime) = &runtime {
                        runtime.persist_skill_catalog(&snapshot.registry)?
                    } else {
                        crate::skill::session::persist_catalog(
                            &kuku_home,
                            &snapshot.registry,
                            None,
                        )?
                    };
                    snapshot.catalog_id = catalog["snapshot"].as_str().map(str::to_owned);
                    skill_record = Some((catalog, snapshot.bootstrap_loaded.clone()));
                }
                bootstrap_skill = restore_bootstrap_skill(&snapshot).or(bootstrap_skill);
                skill_catalog_id = snapshot.catalog_id;
                Some(snapshot.registry)
            } else {
                match build_registry_snapshot(
                    &workspace,
                    &config.discovery,
                    plugin_registry_opt.as_ref(),
                ) {
                    Ok(mut registry) => {
                        if needs_skill_paths {
                            registry.materialize_docs(&kuku_home, stop).await?;
                        }
                        let bootstrap_loaded = bootstrap_loaded_names(bootstrap_skill.as_ref());
                        if let Some(skill) = bootstrap_skill.as_mut()
                            && skill.body.is_empty()
                            && let Some(name) = skill.name.as_deref()
                            && let Some(def) = registry.get(name)
                            && def.source == crate::skill::definition::SkillSource::Builtin
                            && let Some(body) =
                                crate::skill::session::admission_body(&registry, name)
                        {
                            skill.body = body;
                        }
                        let catalog = if let Some(runtime) = &runtime {
                            runtime.persist_skill_catalog(&registry)?
                        } else {
                            crate::skill::session::persist_catalog(&kuku_home, &registry, None)?
                        };
                        skill_catalog_id = catalog["snapshot"].as_str().map(str::to_owned);
                        skill_record = Some((catalog, bootstrap_loaded));
                        Some(registry)
                    }
                    Err(_) => None,
                }
            };
            (registry, previous.map(|snapshot| snapshot.registry))
        };

        // A missing explicitly selected Skill must not consume an input or a Turn.
        for name in &self.input_skills {
            if self.disable_skills {
                return Err(Error::InvalidArgument(
                    "selected input skill cannot load while skills are disabled".into(),
                ));
            }
            if skill_registry
                .as_ref()
                .and_then(|registry| registry.get(name))
                .is_none()
            {
                return Err(Error::InvalidArgument(format!(
                    "selected input skill '{name}' was not found"
                )));
            }
        }

        if resumed_permission.is_none() && !compact_only {
            append_interrupted_active_turn(
                &events_path,
                &existing_events,
                &conversation,
                "resume_before_new_turn",
            )?;
            append_turn_started(&events_path, &conversation, turn)?;
            if !self.skip_user_message {
                append_message_user_with_sender(
                    &events_path,
                    &conversation,
                    turn,
                    &self.prompt,
                    self.input_source.agent_address(),
                    &self.input_source,
                    std::mem::take(&mut self.input_images),
                )?;
            }

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

        if let Some((registry, bootstrap_loaded)) = skill_record {
            store.append(EventPayload::ContextSkills {
                conversation: conversation.as_str().to_string(),
                turn,
                ts: now_timestamp()?,
                registry,
                bootstrap_loaded,
            })?;
        }

        if let (None, Some(plugin_reg)) = (&resumed_permission, &plugin_registry_opt) {
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
        let frozen_catalog = if self.runtime.is_some() {
            crate::context::runtime::read_frozen_catalog(&events_path, conversation.as_str())?
        } else {
            None
        };
        let catalog = if let Some(catalog) = frozen_catalog {
            catalog
        } else if let Some(dir) = &prompts_dir {
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

        let mut resumed_state = resumed_state(lifecycle.as_ref(), turn);
        if let (Some(lifecycle), Some(turn)) = (lifecycle.as_ref(), resumable_batch) {
            for open in lifecycle
                .open_tools
                .iter()
                .filter(|open| is_undispatched_batch_call(open, &conversation, turn))
            {
                resumed_state
                    .queued_tool_calls
                    .push_back(QueuedToolCall::new(
                        open.tool_call.clone(),
                        super::helpers::display_summary(
                            &open.tool_call.name,
                            &open.tool_call.args,
                            None,
                        ),
                    ));
            }
        }
        {
            for queued in &mut resumed_state.queued_tool_calls {
                if let Some(preparation) =
                    existing_events
                        .iter()
                        .rev()
                        .find_map(|event| match &event.payload {
                            EventPayload::Unknown(value)
                                if value["kind"] == "runtime.tool_prepared"
                                    && value["agent"] == conversation.as_str()
                                    && value["turn"] == turn
                                    && value["tool_call_id"] == queued.tool_call.id =>
                            {
                                Some(ToolPreparation {
                                    args: value["args"].clone(),
                                    definition: value["definition"].clone(),
                                })
                            }
                            _ => None,
                        })
                {
                    queued.tool_call.args.clone_from(&preparation.args);
                    queued.preparation = Some(preparation);
                }
            }
        }
        let (admitted_tool_call_ids, tool_rounds) =
            resumed_tools(&existing_events, turn, &conversation);

        let context_cursor = if resumed_permission.is_some()
            && self.runtime.is_some()
            && skill_registry.is_some()
            && resumed_state
                .queued_tool_calls
                .iter()
                .any(|call| super::helpers::is_skill_tool(&call.tool_call.name))
        {
            let mut cursor = Box::new(crate::context::runtime::ContextCursor::default());
            cursor.refresh(
                &events_path,
                conversation.as_str(),
                self.runtime_limits.context_replay_bytes(),
            )?;
            Some(cursor)
        } else {
            None
        };
        let cleanup_context = Some((events_path.clone(), turn));
        let pending = PendingRun {
            context_cursor,
            failed_compaction: None,
            session_id: session_id.clone(),
            conversation: conversation.clone(),
            query: self,
            events_path,
            kuku_home,
            workspace,
            turn,
            request_num: resumed_request_num(&existing_events, turn, &conversation),
            cumulative: super::types::CumulativeUsage::default(),
            resolved: None,
            admitted_tool_call_ids,
            queued_tool_calls: resumed_state.queued_tool_calls,
            resumed_permission_requests: resumed_state.resumed_permission_requests,
            pending_events: std::collections::VecDeque::new(),
            pending_error: None,
            waiting_for_job_input: false,
            config,
            prompts_dir,
            bootstrap_skill,
            skill_registry,
            skill_catalog_id,
            previous_skill_registry,
            frozen_turn_prefix: super::types::TurnPrefixFreeze::default(),
            tool_registry_override,
            catalog,
            cancel_token: cancel_token.clone(),
            handoff_triggered: false,
            handoff_keep_turns,
            plugin_registry,
            hook_context: Vec::new(),
            force_continue_count: 0,
            model_request_count: resumed_model_request_count(&existing_events, turn, &conversation),
            tool_rounds,
            tool_calls: 0,
            tool_names: Vec::new(),
            tool_denied: 0,
            tool_errors: 0,
            thinking_duration_ms: 0,
            runtime_log_writer,
            request_base: None,
            recovery_count: 0,
            tool_argument_retries: super::tool_response::resumed_tool_argument_retries(
                &existing_events,
                turn,
                conversation.as_str(),
            ),
        };

        let state = if let Some(request) = resumed_state.first_request {
            RunState::WaitingForPermission(Box::new(PendingPermission { pending, request }))
        } else {
            RunState::Pending(Box::new(pending))
        };

        Ok(Run {
            agent: conversation,
            cleanup_context,
            session_id: session_id.clone(),
            state,
            slots: std::collections::HashMap::new(),
            slot_event_tx,
            slot_event_rx,
            cancel_token,
            lock_path: startup_lock.map_or_else(
                || lock_path.with_extension("nested"),
                StartupLockGuard::into_path,
            ),
            deferred_runtime_logs: std::collections::VecDeque::new(),
        })
    }

    pub async fn run(self) -> Result<RunOutput> {
        let mut run = self.start_session_with_lock(true, false, None).await?;
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
                    ));
                }
            }
        }
    }

    pub async fn run_with_permission_choice(
        self,
        choice: super::types::PermissionChoice,
    ) -> Result<RunOutput> {
        let mut run = self.start_session_with_lock(true, false, None).await?;
        loop {
            match run.next().await? {
                Some(UiEvent::PermissionRequested { request }) => {
                    run.decide(&request.id, choice).await?;
                }
                Some(UiEvent::Done { output, .. }) => return Ok(output),
                Some(_) => continue,
                None => {
                    return Err(Error::InvalidEventStream(
                        "run ended without producing Done".to_string(),
                    ));
                }
            }
        }
    }
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
        restored.push(crate::skill::session::admission_body(
            &snapshot.registry,
            skill_name,
        )?);
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
        queued_tool_calls.push_back(QueuedToolCall::new(
            pending.tool_call.clone(),
            pending.request.summary.clone(),
        ));
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
    resumable_batch: Option<u64>,
) -> Result<()> {
    let Some(lifecycle) = lifecycle else {
        return Ok(());
    };
    let Some(open_tool) = lifecycle.open_tools.iter().find(|open| {
        &open.conversation == conversation
            && !resumable_batch
                .is_some_and(|turn| is_undispatched_batch_call(open, conversation, turn))
    }) else {
        return Ok(());
    };

    Err(Error::InterruptedOpenTool(format!(
        "session {session_id} has unresolved tool call {} from turn {}; review the session before resuming",
        open_tool.tool_call.id, open_tool.turn
    )))
}

fn is_undispatched_batch_call(
    open: &super::lifecycle::OpenToolLifecycle,
    conversation: &crate::conversation::address::ConversationAddress,
    turn: u64,
) -> bool {
    &open.conversation == conversation
        && open.turn == turn
        && open.kind == super::lifecycle::OpenToolKind::Interrupted
}

fn resumed_model_request_count(
    events: &[crate::event::StoredEvent],
    turn: u64,
    conversation: &crate::conversation::address::ConversationAddress,
) -> u64 {
    let mut request_ids = std::collections::BTreeSet::new();
    for event in events {
        match &event.payload {
            EventPayload::ModelResponse {
                conversation: event_conversation,
                turn: event_turn,
                request_id,
                ..
            }
            | EventPayload::ModelError {
                conversation: event_conversation,
                turn: event_turn,
                request_id,
                ..
            } if *event_turn == turn
                && event_matches_conversation(event_conversation.as_deref(), conversation) =>
            {
                request_ids.insert(request_id.as_str());
            }
            EventPayload::ModelRecovery {
                conversation: event_conversation,
                turn: event_turn,
                to_request_id,
                ..
            } if *event_turn == turn
                && event_matches_conversation(Some(event_conversation), conversation) =>
            {
                request_ids.insert(to_request_id.as_str());
            }
            EventPayload::Unknown(value)
                if matches!(
                    value["kind"].as_str(),
                    Some("runtime.compaction_started" | "runtime.model_rejected")
                ) && value["agent"] == conversation.as_str()
                    && value["turn"] == turn =>
            {
                if let Some(id) = value["request_id"].as_str() {
                    request_ids.insert(id);
                }
            }
            _ => {}
        }
    }
    request_ids.len() as u64
}

fn resumed_tools(
    events: &[crate::event::StoredEvent],
    turn: u64,
    conversation: &crate::conversation::address::ConversationAddress,
) -> (HashSet<String>, u64) {
    let mut tool_call_ids = HashSet::new();
    let mut request_ids = HashSet::new();
    for event in events {
        if let EventPayload::ToolCall {
            turn: event_turn,
            conversation: event_conversation,
            request_id,
            tool_call_id,
            ..
        } = &event.payload
        {
            if *event_turn == turn
                && event_matches_conversation(event_conversation.as_deref(), conversation)
            {
                tool_call_ids.insert(tool_call_id.clone());
                request_ids.insert(request_id.as_str());
            }
        }
    }
    (tool_call_ids, request_ids.len() as u64)
}

fn resumed_request_num(
    events: &[crate::event::StoredEvent],
    turn: u64,
    conversation: &crate::conversation::address::ConversationAddress,
) -> u64 {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ModelResponse {
                conversation: event_conversation,
                turn: event_turn,
                request_id,
                ..
            }
            | EventPayload::ModelError {
                conversation: event_conversation,
                turn: event_turn,
                request_id,
                ..
            } if *event_turn == turn
                && event_matches_conversation(event_conversation.as_deref(), conversation) =>
            {
                Some(request_num_from_id(request_id))
            }
            EventPayload::ModelRecovery {
                conversation: event_conversation,
                turn: event_turn,
                to_request_id: request_id,
                ..
            } if *event_turn == turn
                && event_matches_conversation(Some(event_conversation), conversation) =>
            {
                Some(request_num_from_id(request_id))
            }
            EventPayload::Unknown(value)
                if value["kind"] == "runtime.model_rejected"
                    && value["agent"] == conversation.as_str()
                    && value["turn"] == turn =>
            {
                value["request_id"].as_str().map(request_num_from_id)
            }
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

fn event_matches_conversation(
    event_conversation: Option<&str>,
    conversation: &crate::conversation::address::ConversationAddress,
) -> bool {
    event_conversation.map_or_else(
        || conversation.is_main(),
        |value| value == conversation.as_str(),
    )
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
mod recovery_resume_tests;
#[cfg(test)]
mod startup_prune_tests;
