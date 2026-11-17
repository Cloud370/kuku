use std::sync::Arc;

use tokio::sync::watch;

use crate::query::{PermissionChoice, UiEvent};
use crate::{Error, Result};

use super::record::Record;
use super::{
    ensure_open, Activation, AgentConfig, AgentStatus, InputSource, Session, UsageTotals, Worker,
};

impl Session {
    pub(super) fn schedule(&self, address: &str) -> Result<()> {
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        let agent = state.agent_mut(address)?;
        if matches!(
            agent.snapshot.status,
            AgentStatus::Stopped | AgentStatus::Interrupted | AgentStatus::Waiting
        ) {
            return Ok(());
        }
        if agent.active_epoch.is_some() {
            return Ok(());
        }
        if agent.inbox.is_empty() && agent.permission_reply.is_none() {
            return Ok(());
        }
        state.next_epoch = state
            .next_epoch
            .checked_add(1)
            .ok_or_else(|| Error::InvalidEventStream("execution epoch overflow".into()))?;
        let epoch = state.next_epoch;
        let agent = state.agent_mut(address)?;
        agent.active_epoch = Some(epoch);
        agent.snapshot.status = AgentStatus::Running;
        let mut workers = self.inner.workers.lock().unwrap();
        workers.retain(|_, worker| !worker.join.is_finished());
        let (cancel, receive_cancel) = watch::channel(false);
        let (complete, done) = watch::channel(None);
        let session = self.clone();
        let target = address.to_string();
        let join = tokio::spawn(async move {
            let mut result = session.drive_agent(&target, epoch, receive_cancel).await;
            if let Err(error) = &result {
                if let Err(commit_error) = session.settle_failure(&target, epoch, error) {
                    result = Err(commit_error);
                }
            }
            let _ = complete.send(Some(
                result.as_ref().map(|_| ()).map_err(ToString::to_string),
            ));
            result
        });
        workers.insert(
            epoch,
            Worker {
                address: address.into(),
                cancel,
                done,
                join,
            },
        );
        Ok(())
    }

    async fn drive_agent(
        &self,
        address: &str,
        epoch: u64,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<()> {
        loop {
            if *cancel.borrow() || self.inner.state.lock().unwrap().closing {
                self.settle_cancelled(address, epoch)?;
                return Ok(());
            }
            let (config, inputs, reply) = {
                let mut state = self.inner.state.lock().unwrap();
                let agent = state.agent_mut(address)?;
                if agent.active_epoch != Some(epoch) {
                    return Ok(());
                }
                let reply = agent.permission_reply.clone();
                let config = agent.snapshot.config.clone();
                let inputs = if reply.is_some() {
                    agent.accepted.clone()
                } else {
                    let mut bytes = 0;
                    let source = agent.inbox.front().map(|input| &input.source);
                    agent
                        .inbox
                        .iter()
                        .take_while(|input| {
                            bytes += input.text.len() + 2;
                            Some(&input.source) == source
                                && bytes <= self.inner.options.limits.max_input_bytes + 2
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                };
                if inputs.is_empty() && reply.is_none() {
                    agent.snapshot.status = AgentStatus::Idle;
                    agent.active_epoch = None;
                    return Ok(());
                }
                if reply.is_none() {
                    self.commit(
                        &mut state,
                        Record::InputsAccepted {
                            address: address.into(),
                            input_ids: inputs
                                .iter()
                                .map(|input| input.command_id.clone())
                                .collect(),
                        },
                    )?;
                }
                (config, inputs, reply)
            };
            let prompt = inputs
                .iter()
                .map(|input| input.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            let source = inputs
                .first()
                .map(|input| input.source.clone())
                .unwrap_or_else(|| InputSource::Host("local".into()));
            let query = self.build_query(address, &config, prompt, source)?;
            let mut run = query.start_nested().await?;
            if *cancel.borrow() || self.inner.state.lock().unwrap().closing {
                self.cleanup_failed_run(address, &mut run).await;
                self.settle_cancelled(address, epoch)?;
                return Ok(());
            }
            if let Some((request_id, allow)) = reply {
                let original_id = self
                    .agent(address)?
                    .permission
                    .map(|request| request.tool_call_id)
                    .ok_or(Error::PermissionRequestNotPending(request_id))?;
                run.decide(
                    &original_id,
                    if allow {
                        PermissionChoice::Once
                    } else {
                        PermissionChoice::Deny
                    },
                    None,
                )
                .await?;
            }
            let mut cancelling = false;
            let mut cleanup_failure = None;
            loop {
                let event = if cancelling {
                    run.next().await
                } else {
                    tokio::select! {
                        biased;
                        _ = cancel.changed() => { cancelling = true; run.cancel(); continue }
                        result = run.next() => result,
                    }
                };
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        self.cleanup_failed_run(address, &mut run).await;
                        return Err(error);
                    }
                };
                let Some(event) = event else {
                    if cancelling {
                        self.settle_cancelled(address, epoch)?;
                        return match cleanup_failure {
                            Some(error) => Err(error),
                            None => Ok(()),
                        };
                    }
                    return Err(Error::InvalidEventStream(format!(
                        "agent '{address}' execution ended without a durable completion"
                    )));
                };
                if !matches!(
                    event,
                    UiEvent::Done { .. } | UiEvent::PermissionRequested { .. }
                ) {
                    let _ = self.inner.live.send((address.into(), event.clone()));
                }
                match event {
                    UiEvent::Done { output, .. } => {
                        let usage = self.turn_usage(address, output.turn)?;
                        let mut state = self.inner.state.lock().unwrap();
                        if state.agent_mut(address)?.active_epoch != Some(epoch) {
                            return Ok(());
                        }
                        let waiting = state.agents[address].snapshot.question.is_some();
                        self.commit(
                            &mut state,
                            Record::TurnSettled {
                                address: address.into(),
                                status: if cancelling {
                                    AgentStatus::Stopped
                                } else if waiting {
                                    AgentStatus::Waiting
                                } else {
                                    AgentStatus::Idle
                                },
                                output: Some(output.text.clone()),
                                error: None,
                                turn: Some(output.turn),
                                usage,
                            },
                        )?;
                        let _ = self.inner.live.send((
                            address.into(),
                            UiEvent::Done {
                                output,
                                usage: None,
                                turn: state.agents[address].snapshot.turn.unwrap_or(0),
                            },
                        ));
                        if cancelling || state.closing || waiting {
                            state.agent_mut(address)?.active_epoch = None;
                            return Ok(());
                        }
                        let pending = &state.agents[address].inbox;
                        if !pending
                            .iter()
                            .any(|input| input.activation == Activation::Wake)
                        {
                            state.agent_mut(address)?.active_epoch = None;
                            return Ok(());
                        }
                        break;
                    }
                    UiEvent::Cancelled { .. } => {
                        self.settle_cancelled(address, epoch)?;
                        return match cleanup_failure {
                            Some(error) => Err(error),
                            None => Ok(()),
                        };
                    }
                    UiEvent::ToolEnd {
                        result: Some(result),
                        ..
                    } if result["cleanup_incomplete"] == true => {
                        let message = format!("agent '{address}' command cleanup reached its deadline; an inherited pipe or escaped process may remain");
                        let mut state = self.inner.state.lock().unwrap();
                        self.commit(
                            &mut state,
                            Record::CleanupFailed {
                                address: address.into(),
                                message: message.clone(),
                            },
                        )?;
                        cleanup_failure = Some(Error::InvalidEventStream(message));
                        cancelling = true;
                        run.cancel();
                    }
                    UiEvent::PermissionRequested { mut request } => {
                        let mut state = self.inner.state.lock().unwrap();
                        if state.agent_mut(address)?.active_epoch != Some(epoch) {
                            return Ok(());
                        }
                        request.id = format!(
                            "permission:{address}:{}:{}",
                            request.turn, request.tool_call_id
                        );
                        self.commit(
                            &mut state,
                            Record::PermissionWaiting {
                                address: address.into(),
                                request,
                            },
                        )?;
                        state.agent_mut(address)?.active_epoch = None;
                        return Ok(());
                    }
                    UiEvent::TextDelta { .. }
                    | UiEvent::ThinkingDelta { .. }
                    | UiEvent::ToolStart { .. }
                    | UiEvent::ToolOutput { .. }
                    | UiEvent::ToolEnd { .. }
                    | UiEvent::Error { .. }
                    | UiEvent::ModelRequest { .. }
                    | UiEvent::ModelRecovery { .. }
                    | UiEvent::Log { .. }
                    | UiEvent::TurnStart { .. } => {}
                }
            }
        }
    }

    fn build_query(
        &self,
        address: &str,
        config: &AgentConfig,
        prompt: String,
        source: InputSource,
    ) -> Result<crate::Query> {
        let mut query = crate::query(prompt)
            .session(&self.inner.id)
            .conversation(address)
            .workspace(
                config
                    .workspace
                    .as_ref()
                    .unwrap_or(&self.inner.options.workspace),
            )
            .kuku_home(&self.inner.options.data_dir)
            .config(self.inner.options.config.clone());
        query.connection = config.connection.clone();
        query.reasoning_effort = config.reasoning_effort.clone();
        query.service_tier = config.service_tier.clone();
        query.authorizer = Some(self.inner.options.authorizer.clone());
        query.input_source = source;
        query.runtime_limits = self.inner.options.limits.clone();
        query.model_capacity = Some(self.inner.model_capacity.clone());
        query.runtime = Some(Arc::downgrade(&self.inner));
        query.turn_override = self.agent(address)?.turn;
        query.events_path_override = Some(self.inner.events_path.clone());
        if let Some(model) = &config.model {
            query = query.model(model);
        }
        if let Some(tier) = &config.tier {
            query = query.tier(tier);
        }
        if let Some(tokens) = config.max_output_tokens {
            query = query.max_output_tokens(tokens);
        }
        if let Some(prompts_dir) = &self.inner.options.prompts_dir {
            query = query.prompts_dir(prompts_dir);
        }
        if config.no_skills {
            query = query.no_skills();
        }
        if config.no_agents {
            query = query.no_agents();
        }
        if let Some(name) = &config.bootstrap_skill {
            let registry = crate::skill::build_registry_snapshot_for_host(
                &self.inner.options.data_dir,
                config
                    .workspace
                    .as_ref()
                    .unwrap_or(&self.inner.options.workspace),
                &self.inner.options.config,
            )?;
            let definition = registry.get(name).ok_or_else(|| {
                Error::InvalidArgument(format!("bootstrap skill '{name}' was not found"))
            })?;
            query = query.bootstrap_skill(name, definition.instructions.clone());
        }
        query.agent_instructions = config.instructions.clone();
        if let Some(preset) = &config.preset {
            let catalog = match crate::context::runtime::read_frozen_catalog(
                &self.inner.events_path,
                address,
            )? {
                Some(catalog) => catalog,
                None => match &self.inner.options.prompts_dir {
                    Some(dir) => crate::PromptCatalog::load_from_dir(dir)?,
                    None => crate::builtin_prompt_catalog(),
                },
            };
            let registry = crate::agent::registry::AgentRegistry::builder()
                .builtins(&catalog)
                .build_with_discovery(
                    config
                        .workspace
                        .as_ref()
                        .unwrap_or(&self.inner.options.workspace),
                    &self.inner.options.config.discovery,
                )?
                .build();
            let definition = registry.get(preset).ok_or_else(|| {
                Error::InvalidArgument(format!("preset '{preset}' was not found"))
            })?;
            if query.agent_instructions.is_none() {
                query.agent_instructions = Some(definition.instructions.clone());
            }
            if config.tier.is_none() {
                query = query.tier(&definition.tier);
            }
            let allowed = definition.tools.clone().unwrap_or_else(|| {
                definition
                    .tool_profile
                    .allowed_tools()
                    .iter()
                    .map(|tool| (*tool).to_string())
                    .collect()
            });
            query.tool_registry_override = Some(
                crate::tool::builtin_registry(false, !config.no_skills)
                    .into_iter()
                    .filter(|tool| allowed.contains(&tool.name))
                    .collect(),
            );
        }
        Ok(query)
    }

    fn settle_cancelled(&self, address: &str, epoch: u64) -> Result<()> {
        let mut state = self.inner.state.lock().unwrap();
        if state.agent_mut(address)?.active_epoch != Some(epoch) {
            return Ok(());
        }
        let cleanup = state.cleanup_failures.get(address).cloned();
        let usage = self.pending_usage(&state.agents[address])?;
        self.commit(
            &mut state,
            Record::TurnSettled {
                address: address.into(),
                status: if cleanup.is_some() {
                    AgentStatus::Interrupted
                } else {
                    AgentStatus::Stopped
                },
                output: None,
                error: cleanup,
                turn: None,
                usage,
            },
        )?;
        state.agent_mut(address)?.active_epoch = None;
        Ok(())
    }

    fn turn_usage(&self, address: &str, turn: u64) -> Result<UsageTotals> {
        let mut totals = UsageTotals::default();
        let mut requests = std::collections::BTreeMap::<String, Option<serde_json::Value>>::new();
        let mut cursor = turn;
        loop {
            let page = crate::event::EventStore::read_page(&self.inner.events_path, cursor, 128)?;
            if page.is_empty() {
                break;
            }
            for event in page {
                cursor = event.id;
                let crate::event::EventPayload::Unknown(value) = event.payload else {
                    continue;
                };
                if value["agent"] != address || value["turn"] != turn {
                    continue;
                }
                let Some(request_id) = value["request_id"].as_str() else {
                    continue;
                };
                if value["kind"] == "runtime.step_prepared" {
                    requests.entry(request_id.into()).or_default();
                } else if value["kind"] == "runtime.model_usage" {
                    requests.insert(request_id.into(), Some(value["usage"].clone()));
                }
            }
        }
        totals.model_requests = requests.len() as u64;
        for usage in requests.into_values() {
            let Some(usage) = usage else {
                totals.unreported_requests += 1;
                continue;
            };
            if usage["input_tokens"].is_null() || usage["output_tokens"].is_null() {
                totals.unreported_requests += 1;
            }
            for (total, key) in [
                (&mut totals.input_tokens, "input_tokens"),
                (&mut totals.output_tokens, "output_tokens"),
                (
                    &mut totals.cache_read_input_tokens,
                    "cache_read_input_tokens",
                ),
                (
                    &mut totals.cache_creation_input_tokens,
                    "cache_creation_input_tokens",
                ),
            ] {
                if let Some(amount) = usage[key].as_u64() {
                    *total = Some(total.unwrap_or(0).checked_add(amount).ok_or_else(|| {
                        Error::InvalidEventStream("usage counter overflow".into())
                    })?);
                }
            }
        }
        Ok(totals)
    }

    fn settle_failure(&self, address: &str, epoch: u64, error: &Error) -> Result<()> {
        let mut state = self.inner.state.lock().unwrap();
        if state.agent_mut(address)?.active_epoch != Some(epoch) {
            return Ok(());
        }
        let usage = self.pending_usage(&state.agents[address])?;
        self.commit(
            &mut state,
            Record::TurnSettled {
                address: address.into(),
                status: AgentStatus::Interrupted,
                output: None,
                error: Some(error.to_string()),
                turn: None,
                usage,
            },
        )?;
        state.agent_mut(address)?.active_epoch = None;
        Ok(())
    }

    async fn cleanup_failed_run(&self, address: &str, run: &mut crate::Run) {
        run.cancel();
        while !run.slots.is_empty() {
            match run.next().await {
                Ok(Some(event)) => {
                    if matches!(&event, UiEvent::ToolEnd { result: Some(result), .. } if result["cleanup_incomplete"] == true)
                    {
                        let mut state = self.inner.state.lock().unwrap();
                        let _ = self.commit(&mut state, Record::CleanupFailed { address: address.into(), message: format!("agent '{address}' cleanup after an execution error was incomplete") });
                    }
                    let _ = self.inner.live.send((address.into(), event));
                }
                Ok(None) => break,
                Err(_) => {}
            }
        }
    }

    fn pending_usage(&self, agent: &super::record::AgentState) -> Result<UsageTotals> {
        if agent.accepted.is_empty() {
            return Ok(UsageTotals::default());
        }
        match agent.snapshot.turn {
            Some(turn) => self.turn_usage(&agent.snapshot.address, turn),
            None => Ok(UsageTotals::default()),
        }
    }
}
