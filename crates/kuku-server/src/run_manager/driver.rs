use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use kuku::context::observations::{ObservationBuilder, ToolObservation, ToolObservationData};
use kuku::event::{
    ActivityFact, ActivityKindFact, ActivityStatusFact, CheckFact, ExecutionScope,
    InteractionChoiceFact, InteractionFact, InteractionId, MetricFact, ObservationFact, RunId,
    SkillContextFact, TaskEvent, TaskId, WorkspaceChangesFact, WorkspaceId,
};
use tokio::sync::mpsc;

use crate::platform::WorkspaceRegistry;

use super::DomainError;

const TEXT_BATCH_BYTES: usize = 4 * 1024;
const TEXT_BATCH_DELAY: Duration = Duration::from_millis(50);

#[derive(Default)]
pub(super) struct TextBuffer {
    text: String,
    deadline: Option<tokio::time::Instant>,
}

impl TextBuffer {
    pub(super) fn push(&mut self, now: tokio::time::Instant, delta: &str) -> Vec<(String, bool)> {
        if delta.is_empty() {
            return Vec::new();
        }
        if self.text.is_empty() {
            self.deadline = Some(now + TEXT_BATCH_DELAY);
        }
        self.text.push_str(delta);
        if self.text.len() >= TEXT_BATCH_BYTES {
            self.flush(false)
        } else {
            Vec::new()
        }
    }

    pub(super) fn deadline(&self) -> Option<tokio::time::Instant> {
        self.deadline
    }

    #[cfg(test)]
    pub(super) fn is_due(&self, now: tokio::time::Instant) -> bool {
        self.deadline.is_some_and(|deadline| now >= deadline)
    }

    pub(super) fn flush(&mut self, finalized: bool) -> Vec<(String, bool)> {
        self.deadline = None;
        drain_text_chunks(&mut self.text, finalized)
    }
}

#[derive(Debug, Clone)]
pub struct DriverStart {
    pub task_id: TaskId,
    pub run_id: RunId,
    pub workspace_id: WorkspaceId,
    pub prompt: String,
    pub tier_id: String,
    pub selected_skills: Vec<SkillContextFact>,
    pub agent_message_id: String,
    pub execution_scope: ExecutionScope,
    pub event_store: kuku::event::EventStore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverCommand {
    Stop,
    Resolve {
        interaction_id: InteractionId,
        choice_id: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum DriverEvent {
    Started,
    Activity(Vec<TaskEvent>),
    InteractionOpened(InteractionFact),
    InteractionClosed(InteractionId),
    Completed(RunResult),
    Stopped,
    Failed(RunFailure),
}

impl Eq for DriverEvent {}

#[derive(Debug, Clone, PartialEq)]
pub struct RunResult {
    pub summary: String,
    pub warnings: Vec<String>,
    pub checks: Option<Vec<CheckFact>>,
    pub metrics: Option<Vec<MetricFact>>,
    pub workspace_changes: Option<WorkspaceChangesFact>,
}

impl Eq for RunResult {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunFailure {
    pub summary: String,
}

pub struct DriverHandle {
    pub commands: mpsc::Sender<DriverCommand>,
    pub events: mpsc::Receiver<DriverEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingDecision {
    request_id: String,
    parent_tool_id: Option<String>,
}

impl PendingDecision {
    fn new(request_id: String, parent_tool_id: Option<String>) -> Self {
        Self {
            request_id,
            parent_tool_id,
        }
    }
}

pub trait RunDriverFactory: Send + Sync {
    fn start(
        &self,
        start: DriverStart,
    ) -> Pin<Box<dyn Future<Output = Result<DriverHandle, DomainError>> + Send>>;
}

pub struct KukuDriverFactory {
    workspaces: Arc<WorkspaceRegistry>,
    config: DriverConfig,
}

enum DriverConfig {
    Static(Arc<kuku::config::Config>),
    Platform(Arc<crate::platform::ConfigService>),
}

impl KukuDriverFactory {
    pub fn new(workspaces: Arc<WorkspaceRegistry>, config: Arc<kuku::config::Config>) -> Self {
        Self {
            workspaces,
            config: DriverConfig::Static(config),
        }
    }

    pub fn from_platform(
        workspaces: Arc<WorkspaceRegistry>,
        config: Arc<crate::platform::ConfigService>,
    ) -> Self {
        Self {
            workspaces,
            config: DriverConfig::Platform(config),
        }
    }
}

fn resolve_product_tier(
    config: &kuku::config::Config,
    tier_id: &str,
) -> Result<String, DomainError> {
    let name = tier_id
        .strip_prefix("tier:")
        .filter(|name| !name.is_empty())
        .ok_or(DomainError::InvalidRequest)?;
    let name = if name == "default" {
        config.default_tier()
    } else {
        name
    };
    config
        .tier(name)
        .map(|_| name.to_owned())
        .ok_or(DomainError::InvalidRequest)
}

pub(super) fn selected_skill_facts(
    store: &kuku::event::EventStore,
    execution_scope: &ExecutionScope,
    current_ids: &[String],
) -> Result<Vec<SkillContextFact>, DomainError> {
    let current = current_ids.iter().collect::<HashSet<_>>();
    if current.len() != current_ids.len() {
        return Err(DomainError::InvalidRequest);
    }
    let mut found = HashMap::<String, Vec<SkillContextFact>>::new();
    for stored in store.read_all().map_err(|_| DomainError::LedgerCorrupt)? {
        let kuku::event::EventPayload::TaskLedger(record) = stored.payload else {
            continue;
        };
        let events = match &record {
            kuku::event::TaskLedgerRecord::Control(transaction) => transaction.events(),
            kuku::event::TaskLedgerRecord::Activity(batch) => batch.events(),
        };
        for event in events {
            let TaskEvent::SkillLoaded(skill) = event else {
                continue;
            };
            if skill.execution != *execution_scope || !current.contains(&skill.skill_id) {
                continue;
            }
            found
                .entry(skill.skill_id.clone())
                .or_default()
                .push(SkillContextFact {
                    skill_id: skill.skill_id.clone(),
                    source: skill.source.clone(),
                    origin: skill.origin,
                    content_hash: skill.content_hash.clone(),
                });
        }
    }
    current_ids
        .iter()
        .map(|skill_id| match found.remove(skill_id).as_deref() {
            Some([fact]) => Ok(fact.clone()),
            _ => Err(DomainError::InvalidRequest),
        })
        .collect()
}

impl RunDriverFactory for KukuDriverFactory {
    fn start(
        &self,
        start: DriverStart,
    ) -> Pin<Box<dyn Future<Output = Result<DriverHandle, DomainError>> + Send>> {
        let workspaces = self.workspaces.clone();
        let config = match &self.config {
            DriverConfig::Static(config) => DriverConfig::Static(config.clone()),
            DriverConfig::Platform(config) => DriverConfig::Platform(config.clone()),
        };
        Box::pin(async move {
            let config = match config {
                DriverConfig::Static(config) => config,
                DriverConfig::Platform(config) => config
                    .snapshot()
                    .await
                    .map_err(|_| DomainError::InvalidRequest)?
                    .resolved
                    .ok_or(DomainError::InvalidRequest)?,
            };
            let tier = resolve_product_tier(&config, &start.tier_id)?;
            let capability = workspaces
                .capability(&start.workspace_id)
                .map_err(|_| DomainError::WorkspaceNotFound)?;
            let query = capability
                .query(
                    start.prompt.clone(),
                    start.execution_scope.clone(),
                    start.event_store.clone(),
                    start.selected_skills.clone(),
                )
                .map_err(|_| DomainError::InvalidRequest)?
                .config((*config).clone())
                .tier(tier);
            let run = query
                .start()
                .await
                .map_err(|_| DomainError::InvalidRequest)?;
            let (command_tx, command_rx) = mpsc::channel(8);
            let (event_tx, event_rx) = mpsc::channel(64);
            event_tx
                .send(DriverEvent::Started)
                .await
                .map_err(|_| DomainError::RunNotActive)?;
            tokio::spawn(run_kuku_driver(start, run, command_rx, event_tx));
            Ok(DriverHandle {
                commands: command_tx,
                events: event_rx,
            })
        })
    }
}

async fn run_kuku_driver(
    start: DriverStart,
    mut run: kuku::Run,
    mut commands: mpsc::Receiver<DriverCommand>,
    events: mpsc::Sender<DriverEvent>,
) {
    let mut pending = HashMap::<InteractionId, PendingDecision>::new();
    let mut activities = HashMap::<String, ActivityFact>::new();
    let mut text = TextBuffer::default();
    let delay = tokio::time::sleep(TEXT_BATCH_DELAY);
    tokio::pin!(delay);
    loop {
        tokio::select! {
            command = commands.recv() => {
                match command {
                    Some(DriverCommand::Stop) => run.cancel(),
                    Some(DriverCommand::Resolve { interaction_id, choice_id }) => {
                        if let Some(decision) = pending.get(&interaction_id).cloned() {
                            let Some(choice) = permission_choice(&choice_id) else {
                                let _ = events.send(DriverEvent::Failed(RunFailure {
                                    summary: format!("invalid interaction choice: {choice_id}"),
                                })).await;
                                return;
                            };
                            if let Err(error) = run
                                .decide(
                                    &decision.request_id,
                                    choice,
                                    decision.parent_tool_id.as_deref(),
                                )
                                .await
                            {
                                let _ = events.send(DriverEvent::Failed(RunFailure {
                                    summary: format!("interaction resolution failed: {error}"),
                                })).await;
                                return;
                            }
                            pending.remove(&interaction_id);
                            if events.send(DriverEvent::InteractionClosed(interaction_id)).await.is_err() {
                                return;
                            }
                        }
                    }
                    None => return,
                }
            }
            _ = &mut delay, if text.deadline().is_some() => {
                if !flush_text(&events, &start.agent_message_id, &mut text, false).await {
                    return;
                }
            }
            next = run.next(), if pending.is_empty() => {
                match next {
                    Ok(Some(event)) if permission_metadata(&event).is_some() => {
                        if !flush_text(&events, &start.agent_message_id, &mut text, false).await {
                            return;
                        }
                        let (request, parent_tool_id) = permission_metadata(&event)
                            .expect("permission event was matched by the guard");
                        if !open_permission(
                            &events,
                            &start.run_id,
                            &mut pending,
                            request.clone(),
                            parent_tool_id.map(str::to_owned),
                        ).await {
                            return;
                        }
                    }
                    Ok(Some(kuku::UiEvent::TextDelta { text: delta })) => {
                        let chunks = text.push(tokio::time::Instant::now(), &delta);
                        if !send_text_chunks(&events, &start.agent_message_id, chunks).await {
                            return;
                        }
                        if let Some(deadline) = text.deadline() {
                            delay.as_mut().reset(deadline);
                        }
                    }
                    Ok(Some(kuku::UiEvent::ToolStart { id, tool, summary, kind })) => {
                        let activity = started_activity(&start.run_id, id.clone(), tool, summary, kind);
                        activities.insert(id, activity.clone());
                        if events.send(DriverEvent::Activity(vec![
                            TaskEvent::ActivityUpserted { activity },
                        ])).await.is_err() {
                            return;
                        }
                    }
                    Ok(Some(kuku::UiEvent::ToolEnd { id, status, summary, model_content, .. })) => {
                        let mut task_events = Vec::new();
                        if let Some(activity) = activities.remove(&id) {
                            let activity = finished_activity(activity, &status, summary, model_content.as_deref());
                            task_events.push(TaskEvent::ActivityUpserted { activity });
                        }
                        match read_file_observation(
                            &start.event_store,
                            &start.execution_scope,
                            &id,
                        ) {
                            Ok(Some(observation)) => {
                                task_events.push(TaskEvent::ObservationRecorded(observation));
                            }
                            Ok(None) => {}
                            Err(error) => {
                                let _ = events.send(DriverEvent::Failed(RunFailure {
                                    summary: error.to_string(),
                                })).await;
                                return;
                            }
                        }
                        if !task_events.is_empty()
                            && events.send(DriverEvent::Activity(task_events)).await.is_err()
                        {
                            return;
                        }
                    }
                    Ok(Some(kuku::UiEvent::Done { output, .. })) => {
                        if !flush_text(&events, &start.agent_message_id, &mut text, true).await {
                            return;
                        }
                        let _ = events.send(DriverEvent::Completed(RunResult {
                            summary: output.text,
                            warnings: Vec::new(),
                            checks: None,
                            metrics: None,
                            workspace_changes: None,
                        })).await;
                        return;
                    }
                    Ok(Some(kuku::UiEvent::Cancelled { .. })) => {
                        let _ = flush_text(&events, &start.agent_message_id, &mut text, true).await;
                        let _ = events.send(DriverEvent::Stopped).await;
                        return;
                    }
                    Ok(Some(kuku::UiEvent::Error { message, .. })) => {
                        let _ = flush_text(&events, &start.agent_message_id, &mut text, true).await;
                        let _ = events.send(DriverEvent::Failed(RunFailure { summary: message })).await;
                        return;
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        let _ = flush_text(&events, &start.agent_message_id, &mut text, true).await;
                        let _ = events.send(DriverEvent::Failed(RunFailure {
                            summary: "driver event stream ended before completion".to_owned(),
                        })).await;
                        return;
                    }
                    Err(error) => {
                        let _ = flush_text(&events, &start.agent_message_id, &mut text, true).await;
                        let _ = events.send(DriverEvent::Failed(RunFailure {
                            summary: error.to_string(),
                        })).await;
                        return;
                    }
                }
            }
        }
    }
}

fn read_file_observation(
    store: &kuku::event::EventStore,
    execution: &ExecutionScope,
    tool_call_id: &str,
) -> Result<Option<ObservationFact>, DomainError> {
    let mut call = None;
    let mut result = None;
    for stored in store.read_all().map_err(|_| DomainError::LedgerCorrupt)? {
        match stored.payload {
            kuku::event::EventPayload::ToolCall {
                request,
                tool_call_id: stored_id,
                tool,
                ..
            } if request.execution == *execution && stored_id == tool_call_id => {
                call = Some((request, tool));
            }
            kuku::event::EventPayload::ToolResult {
                execution: stored_execution,
                tool_call_id: stored_id,
                status,
                summary,
                truncated,
                structured,
                ..
            } if stored_execution == *execution && stored_id == tool_call_id => {
                result = Some((status, summary, truncated, structured));
            }
            _ => {}
        }
    }
    let Some((scope, tool)) = call else {
        return Ok(None);
    };
    let Some((status, summary, truncated, Some(structured))) = result else {
        return Ok(None);
    };
    if tool != "read_file" || status != "ok" || structured["kind"] != "file_content" {
        return Ok(None);
    }
    let (Some(path), Some(observed_hash), Some(start_line), Some(line_count)) = (
        structured["path"].as_str(),
        structured["content_hash"].as_str(),
        structured["start_line"].as_u64(),
        structured["line_count"].as_u64(),
    ) else {
        return Ok(None);
    };
    ObservationBuilder::from_tool(
        scope,
        tool_call_id,
        ToolObservation {
            summary,
            truncated,
            summarized: false,
            data: ToolObservationData::FileRead {
                path: path.to_owned(),
                observed_hash: Some(observed_hash.to_owned()),
                start_line,
                line_count,
            },
        },
    )
    .map(Some)
    .map_err(|_| DomainError::LedgerCorrupt)
}

fn permission_metadata(event: &kuku::UiEvent) -> Option<(&kuku::PermissionRequest, Option<&str>)> {
    match event {
        kuku::UiEvent::PermissionRequested { request } => Some((request, None)),
        kuku::UiEvent::ToolOutput {
            id,
            event: kuku::ToolEvent::PermissionRequested { request },
        } => Some((request, Some(id))),
        _ => None,
    }
}

async fn open_permission(
    events: &mpsc::Sender<DriverEvent>,
    run_id: &RunId,
    pending: &mut HashMap<InteractionId, PendingDecision>,
    request: kuku::PermissionRequest,
    parent_tool_id: Option<String>,
) -> bool {
    let interaction_id = match InteractionId::try_new() {
        Ok(value) => value,
        Err(_) => {
            let _ = events
                .send(DriverEvent::Failed(RunFailure {
                    summary: "interaction identity exhausted".to_owned(),
                }))
                .await;
            return false;
        }
    };
    pending.insert(
        interaction_id.clone(),
        PendingDecision::new(request.id, parent_tool_id),
    );
    events
        .send(DriverEvent::InteractionOpened(InteractionFact {
            interaction_id,
            run_id: run_id.clone(),
            prompt: request.summary,
            choices: permission_choices(),
            selected_choice_id: None,
        }))
        .await
        .is_ok()
}

async fn flush_text(
    events: &mpsc::Sender<DriverEvent>,
    message_id: &str,
    text: &mut TextBuffer,
    finalized: bool,
) -> bool {
    send_text_chunks(events, message_id, text.flush(finalized)).await
}

async fn send_text_chunks(
    events: &mpsc::Sender<DriverEvent>,
    message_id: &str,
    chunks: Vec<(String, bool)>,
) -> bool {
    for (append_text, finalized) in chunks {
        if events
            .send(DriverEvent::Activity(vec![TaskEvent::MessagePatched {
                message_id: message_id.to_owned(),
                append_text,
                finalized,
                request_ids: None,
            }]))
            .await
            .is_err()
        {
            return false;
        }
    }
    true
}

pub(super) fn drain_text_chunks(text: &mut String, finalized: bool) -> Vec<(String, bool)> {
    if text.is_empty() {
        return finalized
            .then(|| (String::new(), true))
            .into_iter()
            .collect();
    }
    let mut chunks = Vec::new();
    while !text.is_empty() {
        let mut end = text.len().min(TEXT_BATCH_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let chunk = text.drain(..end).collect::<String>();
        chunks.push((chunk, finalized && text.is_empty()));
    }
    chunks
}

fn permission_choices() -> Vec<InteractionChoiceFact> {
    [
        ("once", "Allow once"),
        ("session", "Allow for session"),
        ("project", "Allow for project"),
        ("deny", "Deny"),
    ]
    .into_iter()
    .map(|(choice_id, label)| InteractionChoiceFact {
        choice_id: choice_id.to_owned(),
        label: label.to_owned(),
    })
    .collect()
}

pub(super) fn permission_choice(choice_id: &str) -> Option<kuku::PermissionChoice> {
    match choice_id {
        "once" => Some(kuku::PermissionChoice::Once),
        "session" => Some(kuku::PermissionChoice::Session),
        "project" => Some(kuku::PermissionChoice::Project),
        "deny" => Some(kuku::PermissionChoice::Deny),
        _ => None,
    }
}

fn activity_status(status: &str) -> ActivityStatusFact {
    match status {
        "completed" | "success" | "ok" => ActivityStatusFact::Completed,
        _ => ActivityStatusFact::Failed,
    }
}

fn started_activity(
    run_id: &RunId,
    activity_id: String,
    title: String,
    summary: String,
    kind: kuku::ToolKind,
) -> ActivityFact {
    let (kind, detail, conversation_id, agent, tier, result_in_main) = match kind {
        kuku::ToolKind::Agent {
            conversation_id,
            agent,
            tier,
        } => (
            ActivityKindFact::DelegatedAgent,
            None,
            Some(conversation_id),
            Some(agent),
            Some(tier),
            Some(false),
        ),
        _ => (
            ActivityKindFact::Tool,
            Some(summary),
            None,
            None,
            None,
            None,
        ),
    };
    ActivityFact {
        activity_id,
        run_id: run_id.clone(),
        title,
        kind,
        status: ActivityStatusFact::Running,
        detail,
        conversation_id,
        agent,
        tier,
        result_in_main,
        file_references: Vec::new(),
    }
}

fn finished_activity(
    mut activity: ActivityFact,
    status: &str,
    summary: String,
    model_content: Option<&str>,
) -> ActivityFact {
    activity.status = activity_status(status);
    if activity.kind == ActivityKindFact::DelegatedAgent {
        activity.detail = None;
        activity.result_in_main = Some(model_content.is_some());
    } else {
        activity.detail = Some(summary);
    }
    activity
}

#[cfg(test)]
#[path = "driver_selected_skill_tests.rs"]
mod selected_skill_tests;

#[cfg(test)]
#[path = "driver_tests.rs"]
mod activity_tests;
