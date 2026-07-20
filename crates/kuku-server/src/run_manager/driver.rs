use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use kuku::event::{
    ActivityFact, ActivityKindFact, ActivityStatusFact, CheckFact, ExecutionScope,
    InteractionChoiceFact, InteractionFact, InteractionId, MetricFact, RunId, TaskEvent, TaskId,
    WorkspaceChangesFact, WorkspaceId,
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
    pub skill_ids: Vec<String>,
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

pub trait RunDriverFactory: Send + Sync {
    fn start(
        &self,
        start: DriverStart,
    ) -> Pin<Box<dyn Future<Output = Result<DriverHandle, DomainError>> + Send>>;
}

pub struct KukuDriverFactory {
    workspaces: Arc<WorkspaceRegistry>,
    config: Arc<kuku::config::Config>,
}

impl KukuDriverFactory {
    pub fn new(workspaces: Arc<WorkspaceRegistry>, config: Arc<kuku::config::Config>) -> Self {
        Self { workspaces, config }
    }
}

impl RunDriverFactory for KukuDriverFactory {
    fn start(
        &self,
        start: DriverStart,
    ) -> Pin<Box<dyn Future<Output = Result<DriverHandle, DomainError>> + Send>> {
        let workspaces = self.workspaces.clone();
        let config = self.config.clone();
        Box::pin(async move {
            let capability = workspaces
                .capability(&start.workspace_id)
                .map_err(|_| DomainError::WorkspaceNotFound)?;
            let query = capability
                .query(
                    start.prompt.clone(),
                    start.execution_scope.clone(),
                    start.event_store.clone(),
                )
                .map_err(|_| DomainError::InvalidRequest)?
                .config((*config).clone())
                .tier(start.tier_id.clone());
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
    let mut pending = HashMap::<InteractionId, String>::new();
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
                        if let Some(request_id) = pending.get(&interaction_id).cloned() {
                            let Some(choice) = permission_choice(&choice_id) else {
                                let _ = events.send(DriverEvent::Failed(RunFailure {
                                    summary: format!("invalid interaction choice: {choice_id}"),
                                })).await;
                                return;
                            };
                            if let Err(error) = run.decide(&request_id, choice, None).await {
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
            next = run.next() => {
                match next {
                    Ok(Some(kuku::UiEvent::TextDelta { text: delta })) => {
                        let chunks = text.push(tokio::time::Instant::now(), &delta);
                        if !send_text_chunks(&events, &start.agent_message_id, chunks).await {
                            return;
                        }
                        if let Some(deadline) = text.deadline() {
                            delay.as_mut().reset(deadline);
                        }
                    }
                    Ok(Some(kuku::UiEvent::PermissionRequested { request })) => {
                        if !flush_text(&events, &start.agent_message_id, &mut text, false).await {
                            return;
                        }
                        let interaction_id = match InteractionId::try_new() {
                            Ok(value) => value,
                            Err(_) => {
                                let _ = events.send(DriverEvent::Failed(RunFailure {
                                    summary: "interaction identity exhausted".to_owned(),
                                })).await;
                                return;
                            }
                        };
                        pending.insert(interaction_id.clone(), request.id);
                        let interaction = InteractionFact {
                            interaction_id,
                            run_id: start.run_id.clone(),
                            prompt: request.summary,
                            choices: permission_choices(),
                            selected_choice_id: None,
                        };
                        if events.send(DriverEvent::InteractionOpened(interaction)).await.is_err() {
                            return;
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
                        if let Some(activity) = activities.remove(&id) {
                            let activity = finished_activity(activity, &status, summary, model_content.as_deref());
                            if events.send(DriverEvent::Activity(vec![
                                TaskEvent::ActivityUpserted { activity },
                            ])).await.is_err() {
                                return;
                            }
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
mod activity_tests {
    use kuku::event::{ConversationId, RunId};

    use super::{finished_activity, started_activity, ActivityKindFact, ActivityStatusFact};

    fn run_id() -> RunId {
        RunId::parse("run_0123456789abcdef01234567").unwrap()
    }

    #[test]
    fn delegated_activity_keeps_typed_identity_and_no_detail() {
        let conversation_id = ConversationId::parse("con_0123456789abcdef01234567").unwrap();
        let activity = started_activity(
            &run_id(),
            "tool_1".to_owned(),
            "delegate".to_owned(),
            "summary must not leak into detail".to_owned(),
            kuku::ToolKind::Agent {
                conversation_id: conversation_id.clone(),
                agent: "reviewer".to_owned(),
                tier: "strong".to_owned(),
            },
        );

        assert_eq!(activity.kind, ActivityKindFact::DelegatedAgent);
        assert_eq!(activity.conversation_id, Some(conversation_id));
        assert_eq!(activity.agent.as_deref(), Some("reviewer"));
        assert_eq!(activity.tier.as_deref(), Some("strong"));
        assert_eq!(activity.result_in_main, Some(false));
        assert_eq!(activity.detail, None);

        let activity = finished_activity(
            activity,
            "ok",
            "finished".to_owned(),
            Some("delegated result"),
        );
        assert_eq!(activity.status, ActivityStatusFact::Completed);
        assert_eq!(activity.result_in_main, Some(true));
        assert_eq!(activity.detail, None);
    }

    #[test]
    fn ordinary_tool_keeps_detail_and_null_delegated_fields() {
        let activity = started_activity(
            &run_id(),
            "tool_2".to_owned(),
            "read_file".to_owned(),
            "reading".to_owned(),
            kuku::ToolKind::Simple,
        );
        assert_eq!(activity.kind, ActivityKindFact::Tool);
        assert_eq!(activity.detail.as_deref(), Some("reading"));
        assert_eq!(activity.conversation_id, None);
        assert_eq!(activity.agent, None);
        assert_eq!(activity.tier, None);
        assert_eq!(activity.result_in_main, None);

        let activity = finished_activity(activity, "error", "failed".to_owned(), None);
        assert_eq!(activity.status, ActivityStatusFact::Failed);
        assert_eq!(activity.detail.as_deref(), Some("failed"));
        assert_eq!(activity.result_in_main, None);
    }
}
