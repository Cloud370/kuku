use std::sync::Arc;

use tokio::sync::watch;

use crate::event::{EventPayload, EventStore};
use crate::query::{PermissionChoice, UiEvent};
use crate::{Error, Result};

use super::boundary::TurnWork;
use super::record::{timestamp, Record, State};
use super::{ensure_open, Activation, AgentStatus, InputSource, Session, Worker};

enum DriveFailure {
    Execution(Error),
    Cleanup(Error),
}

impl From<Error> for DriveFailure {
    fn from(error: Error) -> Self {
        Self::Execution(error)
    }
}

impl Session {
    pub(super) fn schedule_locked(&self, state: &mut State, address: &str) -> Result<()> {
        ensure_open(state)?;
        let agent = state.agent(address)?;
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
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
            Error::InvalidArgument("activating a session requires a Tokio runtime".into())
        })?;
        let epoch = self.commit(
            state,
            Record::ExecutionStarted {
                address: address.into(),
            },
        )?;
        let agent = state.agent_mut(address)?;
        agent.active_epoch = Some(epoch);
        let mut workers = self.inner.workers.lock().unwrap();
        workers.retain(|_, worker| !worker.join.is_finished());
        let (cancel, receive_cancel) = watch::channel(false);
        let (complete, done) = watch::channel(None);
        let session = self.clone();
        let target = address.to_string();
        let join = runtime.spawn(async move {
            let result = match session.drive_agent(&target, epoch, receive_cancel).await {
                Ok(()) => Ok(()),
                Err(failure) => session.settle_worker_failure(&target, epoch, failure),
            };
            let outcome = result.as_ref().map(|_| ()).map_err(ToString::to_string);
            session.record_worker_outcome(&target, epoch, outcome.clone());
            let _ = complete.send(Some(outcome));
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
    ) -> std::result::Result<(), DriveFailure> {
        let settle_cancelled = || {
            self.settle_cancelled(address, epoch)
                .map_err(DriveFailure::Cleanup)
        };
        loop {
            if *cancel.borrow() || self.inner.state.lock().unwrap().closing {
                return settle_cancelled();
            }
            let Some(mut work) = self.accept_turn(address, epoch)? else {
                return settle_cancelled();
            };
            let execution = work.execution;
            let turn = work.turn;
            let reply = work.reply.take();
            let query = self.build_query(address, work)?;
            let mut run = query.start_nested().await?;
            if *cancel.borrow() || self.inner.state.lock().unwrap().closing {
                self.cleanup_failed_run(address, execution, turn, &mut run)
                    .await
                    .map_err(DriveFailure::Cleanup)?;
                return settle_cancelled();
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
                    run.next_with_tool_faults().await
                } else {
                    tokio::select! {
                        biased;
                        _ = cancel.changed() => { cancelling = true; run.cancel(); continue }
                        result = run.next_with_tool_faults() => result,
                    }
                };
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        self.cleanup_failed_run(address, execution, turn, &mut run)
                            .await
                            .map_err(DriveFailure::Cleanup)?;
                        if matches!(error, Error::ExecutionNotActive(id) if id == execution) {
                            return settle_cancelled();
                        }
                        return Err(error.into());
                    }
                };
                let Some((event, faults)) = event else {
                    if cancelling {
                        settle_cancelled()?;
                        return cleanup_failure
                            .map_or(Ok(()), |error| Err(DriveFailure::Cleanup(error)));
                    }
                    return Err(Error::InvalidEventStream(format!(
                        "agent '{address}' execution ended without a durable completion"
                    ))
                    .into());
                };
                if !matches!(
                    event,
                    UiEvent::Done { .. } | UiEvent::PermissionRequested { .. }
                ) {
                    self.publish(address, execution, turn, event.clone());
                }
                let output_failure = faults
                    .output_persistence_failed
                    .then(|| self.remember_output_failure(address));
                if faults.cleanup_incomplete {
                    let message = format!(
                        "agent '{address}' tool cleanup did not finish; owned resources may remain"
                    );
                    let mut state = self.inner.state.lock().unwrap();
                    let result = self.commit(
                        &mut state,
                        Record::CleanupFailed {
                            address: address.into(),
                            message: message.clone(),
                        },
                    );
                    cleanup_failure.get_or_insert_with(|| {
                        result.err().unwrap_or(Error::InvalidEventStream(message))
                    });
                    cancelling = true;
                    continue;
                }
                if let Some(error) = output_failure {
                    self.cleanup_failed_run(address, execution, turn, &mut run)
                        .await
                        .map_err(DriveFailure::Cleanup)?;
                    return Err(DriveFailure::Cleanup(error));
                }
                match event {
                    UiEvent::Done { output, .. } => {
                        let mut state = self.inner.state.lock().unwrap();
                        if state.agent_mut(address)?.active_epoch != Some(epoch) {
                            return Ok(());
                        }
                        cancelling |= state.agents[address].snapshot.status == AgentStatus::Stopped;
                        if cancelling || state.closing {
                            drop(state);
                            return settle_cancelled();
                        }
                        let waiting = state.agents[address].snapshot.question.is_some();
                        let usage = state.agents[address]
                            .model_usage
                            .turn_totals(Some(output.turn));
                        self.commit(
                            &mut state,
                            Record::TurnSettled {
                                address: address.into(),
                                status: if waiting {
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
                        self.publish_locked(
                            &mut state,
                            address,
                            execution,
                            turn,
                            UiEvent::Done {
                                output,
                                usage: None,
                                turn,
                            },
                        );
                        let pending = &state.agents[address].inbox;
                        if waiting
                            || !pending
                                .iter()
                                .any(|entry| entry.input.activation == Activation::Wake)
                        {
                            state.agent_mut(address)?.active_epoch = None;
                            return Ok(());
                        }
                        self.commit(
                            &mut state,
                            Record::ExecutionStarted {
                                address: address.into(),
                            },
                        )?;
                        break;
                    }
                    UiEvent::Cancelled { .. } => {
                        settle_cancelled()?;
                        return cleanup_failure
                            .map_or(Ok(()), |error| Err(DriveFailure::Cleanup(error)));
                    }
                    UiEvent::PermissionRequested { mut request } => {
                        request.id = format!(
                            "permission:{address}:{}:{}",
                            request.turn, request.tool_call_id
                        );
                        if self.wait_for_permission(address, epoch, request)? {
                            return Ok(());
                        }
                        self.cleanup_failed_run(address, execution, turn, &mut run)
                            .await
                            .map_err(DriveFailure::Cleanup)?;
                        return settle_cancelled();
                    }
                    UiEvent::TextDelta { .. }
                    | UiEvent::ThinkingDelta { .. }
                    | UiEvent::ModelProgress { .. }
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

    fn settle_worker_failure(
        &self,
        address: &str,
        epoch: u64,
        failure: DriveFailure,
    ) -> Result<()> {
        self.settle_failure(address, epoch, &failure)
            .map_err(|error| self.remember_settlement_failure(error))?;
        match failure {
            DriveFailure::Execution(_) => Ok(()),
            DriveFailure::Cleanup(error) => Err(self.remember_settlement_failure(error)),
        }
    }

    fn wait_for_permission(
        &self,
        address: &str,
        epoch: u64,
        request: crate::query::PermissionRequest,
    ) -> Result<bool> {
        let mut state = self.inner.state.lock().unwrap();
        let agent = state.agent(address)?;
        if state.closing
            || agent.active_epoch != Some(epoch)
            || agent.snapshot.status != AgentStatus::Running
        {
            return Ok(false);
        }
        self.commit(
            &mut state,
            Record::PermissionWaiting {
                address: address.into(),
                request,
            },
        )?;
        state.agent_mut(address)?.active_epoch = None;
        Ok(true)
    }

    fn build_query(&self, address: &str, work: TurnWork) -> Result<crate::Query> {
        let config = self.resolve_defaults(address, work.config)?;
        let mut query = crate::query(work.text)
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
        query.model_adapter = self.inner.options.model_adapter.clone();
        query.tool_adapters = self.inner.tool_adapters.clone();
        query.input_source = work.source;
        query.input_skills = work.skills;
        query.runtime_limits = self.inner.options.limits.clone();
        query.model_capacity = Some(self.inner.model_capacity.clone());
        query.model_request_budget = self.inner.model_request_budget.clone();
        query.runtime = Some(Arc::downgrade(&self.inner));
        query.runtime_execution = Some(work.execution);
        query.turn_override = Some(work.turn);
        query.events_path_override = Some(self.inner.events_path.clone());
        query.skip_user_message = work.skip_user_message;
        query.resume_input_turn = work.resume_input_turn;
        if let Some(model) = &config.model {
            query = query.model(model);
        }
        if let Some(tier) = &config.tier {
            query = query.tier(tier);
        }
        if let Some(max_output_tokens) = config.max_output_tokens {
            query = query.max_output_tokens(max_output_tokens);
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
        query.tool_registry_override = Some(self.selected_tools(&config)?);
        Ok(query)
    }

    fn settle_cancelled(&self, address: &str, epoch: u64) -> Result<()> {
        let mut state = self.inner.state.lock().unwrap();
        if state.agent_mut(address)?.active_epoch != Some(epoch) {
            return Ok(());
        }
        let agent = state.agent(address)?;
        let cancellation_error = if agent.accepted.is_empty() {
            None
        } else {
            agent
                .snapshot
                .turn
                .and_then(|turn| self.settle_cancelled_turn(address, turn).err())
        };
        let cleanup = state
            .cleanup_failures
            .get(address)
            .cloned()
            .or_else(|| cancellation_error.as_ref().map(ToString::to_string));
        let usage = agent.pending_usage();
        let turn = agent.snapshot.turn.filter(|_| !agent.accepted.is_empty());
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
                turn,
                usage,
            },
        )?;
        state.agent_mut(address)?.active_epoch = None;
        let agent = state.agent(address)?;
        if !state.closing
            && agent.snapshot.status == AgentStatus::Stopped
            && agent.stopped_at.is_some_and(|stopped| {
                agent.inbox.iter().any(|entry| {
                    entry.queued_at > stopped
                        && entry.input.activation == Activation::Wake
                        && matches!(entry.input.source, InputSource::Host(_))
                })
            })
        {
            self.resume(&mut state, address, None)?;
            self.schedule_locked(&mut state, address)?;
        }
        cancellation_error.map_or(Ok(()), Err)
    }

    fn settle_failure(&self, address: &str, epoch: u64, failure: &DriveFailure) -> Result<()> {
        let mut state = self.inner.state.lock().unwrap();
        let agent = state.agent(address)?;
        if agent.active_epoch != Some(epoch) {
            return Ok(());
        }
        if agent.snapshot.status == AgentStatus::Stopped
            && matches!(failure, DriveFailure::Execution(_))
        {
            drop(state);
            return self.settle_cancelled(address, epoch);
        }
        let (DriveFailure::Execution(error) | DriveFailure::Cleanup(error)) = failure;
        let usage = agent.pending_usage();
        let turn = agent.snapshot.turn.filter(|_| !agent.accepted.is_empty());
        if let Some(turn) = turn {
            self.interrupt_open_turn(address, turn, error.code())?;
        }
        self.commit(
            &mut state,
            Record::TurnSettled {
                address: address.into(),
                status: AgentStatus::Interrupted,
                output: None,
                error: Some(error.to_string()),
                turn,
                usage,
            },
        )?;
        state.agent_mut(address)?.active_epoch = None;
        Ok(())
    }

    fn interrupt_open_turn(&self, address: &str, turn: u64, reason: &str) -> Result<()> {
        let mut cursor = turn;
        let mut started = false;
        loop {
            let events = self.events(cursor, 128)?;
            if events.is_empty() {
                break;
            }
            for event in events {
                cursor = event.id;
                match event.payload {
                    EventPayload::TurnStarted {
                        conversation,
                        turn: target,
                        ..
                    } if conversation == address && target == turn => started = true,
                    EventPayload::TurnCompleted {
                        conversation,
                        turn: target,
                        ..
                    }
                    | EventPayload::TurnCancelled {
                        conversation,
                        turn: target,
                        ..
                    }
                    | EventPayload::TurnInterrupted {
                        conversation,
                        turn: target,
                        ..
                    } if conversation == address && target == turn => return Ok(()),
                    _ => {}
                }
            }
        }
        if started {
            EventStore::open(self.events_path())?.append(EventPayload::TurnInterrupted {
                ts: timestamp()?,
                conversation: address.into(),
                turn,
                reason: reason.into(),
            })?;
        }
        Ok(())
    }

    fn remember_output_failure(&self, address: &str) -> Error {
        self.remember_settlement_failure(Error::InvalidEventStream(format!(
            "agent '{address}': tool output could not be durably captured; execution stopped before the next model request"
        )))
    }

    async fn cleanup_failed_run(
        &self,
        address: &str,
        execution: u64,
        turn: u64,
        run: &mut crate::Run,
    ) -> Result<()> {
        run.cancel();
        let mut failure = None;
        while !run.slots.is_empty() {
            match run.next_with_tool_faults().await {
                Ok(Some((event, faults))) => {
                    if faults.output_persistence_failed {
                        failure.get_or_insert_with(|| self.remember_output_failure(address));
                    }
                    if faults.cleanup_incomplete {
                        let message = format!(
                            "agent '{address}' cleanup after an execution error was incomplete"
                        );
                        let mut state = self.inner.state.lock().unwrap();
                        let result = self.commit(
                            &mut state,
                            Record::CleanupFailed {
                                address: address.into(),
                                message: message.clone(),
                            },
                        );
                        failure.get_or_insert_with(|| {
                            result.err().unwrap_or(Error::InvalidEventStream(message))
                        });
                    }
                    self.publish(address, execution, turn, event);
                }
                Ok(None) => break,
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::config::{
        ApiKey, Config, DiscoveryConfig, HandoffConfig, LogsConfig, PluginConfig, ProviderConfig,
        ProviderFormat, ThinkLevel, TierConfig, UpdateConfig,
    };
    use crate::event::EventPayload;
    use crate::{
        AdapterFuture, AgentConfig, Error, ModelAdapter, ModelRequest, ModelStream, Session,
    };
    use tokio::sync::{watch, Notify};

    use super::super::{AgentStatus, Input, SessionOptions, Worker};
    use super::Record;

    #[derive(Debug)]
    struct GatedModel {
        started: Arc<Notify>,
        resume: Arc<Notify>,
    }

    impl ModelAdapter for GatedModel {
        fn stream(&self, _request: ModelRequest) -> AdapterFuture<'_, ModelStream> {
            let started = self.started.clone();
            let resume = self.resume.clone();
            Box::pin(async move {
                started.notify_one();
                resume.notified().await;
                Err(Error::InvalidEventStream("fixture provider failure".into()))
            })
        }
    }

    fn model_config() -> Config {
        let mut tiers = BTreeMap::new();
        tiers.insert(
            "balanced".into(),
            TierConfig {
                provider: "openai".into(),
                model: "runtime-test".into(),
                think: ThinkLevel::Off,
                reasoning_effort: None,
                service_tier: None,
                context_window: 8192,
                max_output_tokens: Some(128),
                purpose: "balanced".into(),
            },
        );
        let providers = BTreeMap::from([(
            "openai".into(),
            ProviderConfig {
                format: ProviderFormat::OpenAiChat,
                base_url: "https://example.invalid/v1".into(),
                api_key: ApiKey::Plaintext("test".into()),
                options: Default::default(),
            },
        )]);
        Config {
            tiers,
            providers,
            default_tier: "balanced".into(),
            discovery: DiscoveryConfig::default(),
            handoff: HandoffConfig::default(),
            logs: LogsConfig::default(),
            plugin: PluginConfig::default(),
            update: UpdateConfig::default(),
        }
    }

    async fn session_with_agent() -> (tempfile::TempDir, Session) {
        let directory = tempfile::tempdir().unwrap();
        let config = Config {
            tiers: BTreeMap::new(),
            providers: BTreeMap::new(),
            default_tier: String::new(),
            discovery: DiscoveryConfig::default(),
            handoff: HandoffConfig::default(),
            logs: LogsConfig::default(),
            plugin: PluginConfig::default(),
            update: UpdateConfig::default(),
        };
        let options = SessionOptions::new(directory.path(), directory.path().join("data"), config);
        let session = Session::open(options).await.unwrap();
        let mut state = session.inner.state.lock().unwrap();
        session
            .commit(
                &mut state,
                Record::AgentCreated {
                    command_id: "create-main".into(),
                    fingerprint: String::new(),
                    address: "main".into(),
                    config: AgentConfig::default(),
                },
            )
            .unwrap();
        drop(state);
        (directory, session)
    }

    async fn running_session() -> (tempfile::TempDir, Session, u64) {
        let (directory, session) = session_with_agent().await;
        let epoch = 17;
        let mut state = session.inner.state.lock().unwrap();
        session
            .commit(
                &mut state,
                Record::ExecutionStarted {
                    address: "main".into(),
                },
            )
            .unwrap();
        state.agent_mut("main").unwrap().active_epoch = Some(epoch);
        drop(state);
        (directory, session, epoch)
    }

    #[tokio::test]
    async fn wait_for_agent_returns_snapshot_when_agent_is_idle() {
        let (_directory, session) = session_with_agent().await;
        let snapshot = session.wait_for_agent("main").await.unwrap();
        assert_eq!(AgentStatus::Idle, snapshot.status);
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn wait_for_agent_reports_missing_or_empty_worker_completion() {
        let (_directory, session, _) = running_session().await;
        let missing = session.wait_for_agent("main").await.unwrap_err();
        assert!(missing
            .to_string()
            .contains("running agent has no active worker"));
        session.close().await.unwrap();

        let (_directory, session, epoch) = running_session().await;
        let (cancel, _) = watch::channel(false);
        let (complete, done) = watch::channel(None);
        let join = tokio::spawn(async { Ok::<(), Error>(()) });
        drop(complete);
        session.inner.workers.lock().unwrap().insert(
            epoch,
            Worker {
                address: "main".into(),
                cancel,
                done,
                join,
            },
        );

        let empty = session.wait_for_agent("main").await.unwrap_err();
        assert!(empty
            .to_string()
            .contains("agent worker ended without publishing a completion result"));
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn wait_for_agent_surfaces_completed_error_after_terminal_state_and_worker_pruning() {
        let (_directory, session, epoch) = running_session().await;
        {
            let mut state = session.inner.state.lock().unwrap();
            session
                .commit(
                    &mut state,
                    Record::TurnSettled {
                        address: "main".into(),
                        status: AgentStatus::Interrupted,
                        output: None,
                        error: Some("worker cleanup failure".into()),
                        turn: None,
                        usage: Default::default(),
                    },
                )
                .unwrap();
            state.agent_mut("main").unwrap().active_epoch = None;
        }
        let (cancel, _) = watch::channel(false);
        let (complete, done) = watch::channel(Some(Err("worker cleanup failure".into())));
        drop(complete);
        let join = tokio::spawn(async {
            Err::<(), _>(Error::InvalidEventStream("worker cleanup failure".into()))
        });
        session.record_worker_outcome("main", epoch, Err("worker cleanup failure".into()));
        session.inner.workers.lock().unwrap().insert(
            epoch,
            Worker {
                address: "main".into(),
                cancel,
                done,
                join,
            },
        );

        assert_eq!(
            AgentStatus::Interrupted,
            session.agent("main").unwrap().status
        );
        let error = session.wait_for_agent("main").await.unwrap_err();
        assert!(error.to_string().contains("worker cleanup failure"));

        let worker = session
            .inner
            .workers
            .lock()
            .unwrap()
            .remove(&epoch)
            .unwrap();
        let _ = worker.join.await;
        let error = session.wait_for_agent("main").await.unwrap_err();
        assert!(error.to_string().contains("worker cleanup failure"));
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn poisoned_writer_failure_reaches_waiter_through_production_worker() {
        let directory = tempfile::tempdir().unwrap();
        let started = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let mut options = SessionOptions::new(
            directory.path(),
            directory.path().join("data"),
            model_config(),
        );
        options.model_adapter = Some(Arc::new(GatedModel {
            started: started.clone(),
            resume: resume.clone(),
        }));
        let session = Session::open(options).await.unwrap();
        session
            .create_agent("create-main", "main", AgentConfig::default())
            .unwrap();
        session
            .send(Input::new("request", "main", "finish the task"))
            .unwrap();

        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("production worker did not reach the model adapter");
        session
            .inner
            .writer
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .fail_next_write_for_test();
        resume.notify_one();

        let waiter = tokio::time::timeout(Duration::from_secs(2), session.wait_for_agent("main"))
            .await
            .expect("waiter did not return promptly");
        let error = waiter.unwrap_err();
        assert!(error
            .to_string()
            .contains("injected event journal write failure"));
        assert_eq!(AgentStatus::Running, session.agent("main").unwrap().status);

        let close_error = session.close().await.unwrap_err();
        assert!(close_error
            .to_string()
            .contains("injected event journal write failure"));
        assert_eq!(AgentStatus::Running, session.agent("main").unwrap().status);
        let events = session.events(0, 256).unwrap();
        assert!(!events.iter().any(|event| matches!(
            &event.payload,
            EventPayload::TurnCompleted { conversation, .. }
                | EventPayload::TurnCancelled { conversation, .. }
                | EventPayload::TurnInterrupted { conversation, .. }
                if conversation == "main"
        ) || matches!(
            &event.payload,
            EventPayload::Unknown(value)
                if value["kind"] == "runtime.operation"
                    && value["record"]["operation"] == "turn_settled"
        )));
    }
}
