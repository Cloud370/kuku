use std::sync::Arc;

use crate::error::{Error, Result};
use crate::event::{EventPayload, EventStore};
use crate::session::{
    current_workspace, kuku_home, new_session_id, project_policy_path, session_events_path,
    validate_session_id,
};

use super::helpers::{next_turn, now_timestamp, validate_existing_session};
use super::types::{PendingRun, Query, Run, RunOutput, RunState, UiEvent};

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
        let turn = next_turn(&existing_events);
        let is_new_session = existing_events.is_empty();

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

        store.append(EventPayload::TurnStart {
            turn,
            ts: now_timestamp()?,
        })?;
        store.append(EventPayload::UserInput {
            turn,
            ts: now_timestamp()?,
            text: self.prompt.clone(),
        })?;

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

        if let Some(ref plugin_reg) = plugin_registry_opt {
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
        Ok(Run {
            session_id: session_id.clone(),
            state: RunState::Pending(Box::new(PendingRun {
                session_id,
                query: self,
                events_path,
                kuku_home,
                workspace,
                policy_path,
                turn,
                request_num: 0,
                cumulative: super::types::CumulativeUsage::default(),
                resolved: None,
                queued_tool_calls: std::collections::VecDeque::new(),
                pending_events: std::collections::VecDeque::new(),
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
            })),
            slots: std::collections::HashMap::new(),
            slot_event_tx,
            slot_event_rx,
            cancel_token,
            lock_path,
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
}
