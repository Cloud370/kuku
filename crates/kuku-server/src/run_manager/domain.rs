use std::collections::BTreeMap;

use kuku::event::{
    ActivityFact, ActivityKindFact, ActivityStatusFact, ChangeKindFact, ChangesAvailabilityFact,
    FileReferenceFact, InteractionFact, InteractionId, MessageFact, MessageRoleFact,
    ReviewSubmissionRecorded, RunFact, RunId, RunState, TaskEvent, TaskId, TaskLedgerRecord,
    TaskRevision, TaskState, WorkspaceId,
};

use crate::api::{
    ActivityKind, ActivityProjection, ActivityStatus, AnnotationStatus, ApiError, ApiErrorCode,
    ApiVersion, ChangeEntry, ChangeKind, ChangesAvailability, CheckProjection,
    CompletionProjection, FileReferenceProjection, InteractionChoiceProjection,
    InteractionProjection, InteractionStatus, LoadedSkillProjection, MessageProjection,
    MessageRole, MetricProjection, PageCursor, ReviewSnapshot, ReviewSubmissionProjection,
    ReviewSubmissionsChanged, ReviewSummaryProjection, RunProjection, SubmittedReviewNote,
    TaskChange, TaskProjection, TaskSummary, TimelineItemProjection,
};

const MAX_TASK_DTO_BYTES: usize = 16 * 1024 * 1024;

pub(super) fn ensure_bounded(value: &impl serde::Serialize) -> Result<(), DomainError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| DomainError::LedgerCorrupt)?
        .len();
    if bytes > MAX_TASK_DTO_BYTES {
        return Err(DomainError::PayloadTooLarge);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainError {
    TaskNotCreated,
    TaskNotFound,
    WorkspaceNotFound,
    InvalidTransition { from: RunState, to: RunState },
    TaskBusy,
    IdempotencyConflict,
    StaleCommand,
    InteractionNotPending,
    LedgerCorrupt,
    StorageExhausted,
    InvalidRequest,
    PayloadTooLarge,
    RunNotActive,
    ServerBusy,
    CursorAhead,
    StreamLimit,
}

impl std::fmt::Display for DomainError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TaskNotCreated => formatter.write_str("task has not been created"),
            Self::TaskNotFound => formatter.write_str("task does not exist"),
            Self::WorkspaceNotFound => formatter.write_str("workspace does not exist"),
            Self::InvalidTransition { from, to } => {
                write!(
                    formatter,
                    "invalid state transition from {from:?} to {to:?}"
                )
            }
            Self::TaskBusy => formatter.write_str("task already has an active run"),
            Self::IdempotencyConflict => {
                formatter.write_str("idempotency key was reused with different input")
            }
            Self::StaleCommand => formatter.write_str("command revision is stale"),
            Self::InteractionNotPending => formatter.write_str("interaction is not pending"),
            Self::LedgerCorrupt => formatter.write_str("task ledger is corrupt"),
            Self::StorageExhausted => formatter.write_str("storage exhausted"),
            Self::InvalidRequest => formatter.write_str("invalid request"),
            Self::PayloadTooLarge => formatter.write_str("payload too large"),
            Self::RunNotActive => formatter.write_str("run is not active"),
            Self::ServerBusy => formatter.write_str("run queue is full"),
            Self::CursorAhead => formatter.write_str("requested cursor is ahead of the task"),
            Self::StreamLimit => formatter.write_str("task stream limit reached"),
        }
    }
}

impl std::error::Error for DomainError {}

impl DomainError {
    pub fn code(&self) -> ApiErrorCode {
        match self {
            Self::TaskNotCreated | Self::TaskNotFound => ApiErrorCode::TaskNotFound,
            Self::WorkspaceNotFound => ApiErrorCode::WorkspaceNotFound,
            Self::InvalidTransition { .. } | Self::LedgerCorrupt => ApiErrorCode::Internal,
            Self::TaskBusy => ApiErrorCode::TaskBusy,
            Self::IdempotencyConflict => ApiErrorCode::IdempotencyConflict,
            Self::StaleCommand => ApiErrorCode::StaleCommand,
            Self::InteractionNotPending => ApiErrorCode::InteractionNotPending,
            Self::StorageExhausted => ApiErrorCode::StorageExhausted,
            Self::InvalidRequest => ApiErrorCode::InvalidRequest,
            Self::PayloadTooLarge => ApiErrorCode::PayloadTooLarge,
            Self::RunNotActive => ApiErrorCode::RunNotActive,
            Self::ServerBusy => ApiErrorCode::ServerBusy,
            Self::CursorAhead => ApiErrorCode::CursorAhead,
            Self::StreamLimit => ApiErrorCode::StreamLimit,
        }
    }

    pub fn into_api_error(self, trace_id: impl Into<String>) -> ApiError {
        ApiError::new(self.code(), self.to_string(), trace_id)
    }
}

#[derive(Debug, Clone)]
struct RunEntry {
    fact: RunFact,
}

#[derive(Debug, Clone)]
pub struct TaskAggregate {
    task_id: Option<TaskId>,
    workspace_id: Option<WorkspaceId>,
    title: String,
    created_at: String,
    updated_at: String,
    revision: TaskRevision,
    cursor: kuku::event::Cursor,
    runs: BTreeMap<RunId, RunEntry>,
    latest_run_id: Option<RunId>,
    interactions: BTreeMap<InteractionId, InteractionFact>,
    timeline: Vec<TimelineItemProjection>,
    loaded_skills: Vec<LoadedSkillProjection>,
    selected_tier_id: String,
    review_total: u32,
    latest_submission_id: Option<kuku::event::ReviewSubmissionId>,
}

impl Default for TaskAggregate {
    fn default() -> Self {
        Self {
            task_id: None,
            workspace_id: None,
            title: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            revision: TaskRevision::try_new(0).expect("zero is valid"),
            cursor: kuku::event::Cursor::try_new(0).expect("zero is valid"),
            runs: BTreeMap::new(),
            latest_run_id: None,
            interactions: BTreeMap::new(),
            timeline: Vec::new(),
            loaded_skills: Vec::new(),
            selected_tier_id: String::new(),
            review_total: 0,
            latest_submission_id: None,
        }
    }
}

impl TaskAggregate {
    pub fn task_id(&self) -> Option<&TaskId> {
        self.task_id.as_ref()
    }

    pub fn workspace_id(&self) -> Option<&WorkspaceId> {
        self.workspace_id.as_ref()
    }

    pub(super) fn run_state(&self, run_id: &RunId) -> Option<RunState> {
        self.runs.get(run_id).map(|entry| entry.fact.state)
    }

    pub fn revision(&self) -> TaskRevision {
        self.revision
    }

    pub fn cursor(&self) -> kuku::event::Cursor {
        self.cursor
    }

    pub fn timeline_items(&self) -> &[TimelineItemProjection] {
        &self.timeline
    }

    pub fn interaction_is_pending(&self, interaction_id: &InteractionId) -> bool {
        self.interactions
            .get(interaction_id)
            .is_some_and(|interaction| interaction.selected_choice_id.is_none())
    }

    pub(super) fn interaction_accepts_choice(
        &self,
        interaction_id: &InteractionId,
        choice_id: &str,
    ) -> bool {
        self.interactions
            .get(interaction_id)
            .is_some_and(|interaction| {
                interaction.selected_choice_id.is_none()
                    && interaction
                        .choices
                        .iter()
                        .any(|choice| choice.choice_id == choice_id)
            })
    }

    pub(super) fn message_needs_finalization(&self, message_id: &str) -> bool {
        self.timeline.iter().any(|item| {
            matches!(
                item,
                TimelineItemProjection::Message(message)
                    if message.message_id == message_id && !message.finalized
            )
        })
    }

    pub fn set_updated_at(&mut self, updated_at: String) {
        self.updated_at = updated_at;
    }

    pub fn advance_cursor(&mut self, cursor: kuku::event::Cursor) {
        self.cursor = cursor;
    }

    pub fn apply_record(
        &mut self,
        cursor: kuku::event::Cursor,
        record: &TaskLedgerRecord,
    ) -> Result<Vec<TaskChange>, DomainError> {
        if cursor.get() <= self.cursor.get() {
            return Err(DomainError::LedgerCorrupt);
        }
        let mut next = self.clone();
        next.cursor = cursor;
        let changes = next.apply_record_inner(record)?;
        *self = next;
        Ok(changes)
    }

    fn apply_record_inner(
        &mut self,
        record: &TaskLedgerRecord,
    ) -> Result<Vec<TaskChange>, DomainError> {
        let mut changes = Vec::new();
        match record {
            TaskLedgerRecord::Control(transaction) => {
                let expected = if self.task_id.is_none() {
                    TaskRevision::try_new(0).map_err(|_| DomainError::StorageExhausted)?
                } else {
                    self.revision
                        .checked_next()
                        .map_err(|_| DomainError::StorageExhausted)?
                };
                if transaction.task_revision() != expected {
                    return Err(DomainError::LedgerCorrupt);
                }
                self.revision = transaction.task_revision();
                for event in transaction.events() {
                    self.apply_event(event, &mut changes)?;
                }
            }
            TaskLedgerRecord::Activity(batch) => {
                for event in batch.events() {
                    self.apply_event(event, &mut changes)?;
                }
            }
        }
        Ok(changes)
    }

    fn apply_event(
        &mut self,
        event: &TaskEvent,
        changes: &mut Vec<TaskChange>,
    ) -> Result<(), DomainError> {
        match (self.task_id.as_ref(), event_task_id(event)) {
            (None, Some(_)) if !matches!(event, TaskEvent::TaskCreated { .. }) => {
                return Err(DomainError::LedgerCorrupt);
            }
            (Some(expected), Some(actual)) if expected != actual => {
                return Err(DomainError::LedgerCorrupt);
            }
            _ => {}
        }
        match event {
            TaskEvent::TaskCreated {
                task_id,
                workspace_id,
                title,
                created_at,
            } => {
                if self.task_id.is_some() {
                    return Err(DomainError::LedgerCorrupt);
                }
                self.task_id = Some(task_id.clone());
                self.workspace_id = Some(workspace_id.clone());
                self.title = title.clone();
                self.created_at = created_at.clone();
                self.updated_at = created_at.clone();
            }
            TaskEvent::TaskTitleChanged { title } => self.title = title.clone(),
            TaskEvent::RunQueued { run }
            | TaskEvent::RunStarted { run }
            | TaskEvent::RunNeedsAttention { run }
            | TaskEvent::RunStopping { run }
            | TaskEvent::RunCompleted { run }
            | TaskEvent::RunStopped { run }
            | TaskEvent::RunFailed { run }
            | TaskEvent::RunInterrupted { run } => self.apply_run(run, changes)?,
            TaskEvent::InteractionOpened { interaction } => {
                self.interactions
                    .insert(interaction.interaction_id.clone(), interaction.clone());
                self.set_run_state(&interaction.run_id, RunState::NeedsAttention)?;
                changes.push(self.run_state_change());
                let projected = interaction_projection(interaction, self.cursor);
                self.timeline
                    .push(TimelineItemProjection::Interaction(projected.clone()));
                changes.push(TaskChange::InteractionUpserted {
                    interaction: projected,
                });
            }
            TaskEvent::InteractionResolved {
                interaction_id,
                choice_id,
            } => {
                let (run_id, interaction) = {
                    let interaction = self
                        .interactions
                        .get_mut(interaction_id)
                        .ok_or(DomainError::InteractionNotPending)?;
                    if interaction.selected_choice_id.is_some() {
                        return Err(DomainError::InteractionNotPending);
                    }
                    interaction.selected_choice_id = Some(choice_id.clone());
                    (interaction.run_id.clone(), interaction.clone())
                };
                if !self
                    .interactions
                    .values()
                    .any(|value| value.run_id == run_id && value.selected_choice_id.is_none())
                {
                    self.set_run_state(&run_id, RunState::Running)?;
                    changes.push(self.run_state_change());
                }
                let projected = interaction_projection(&interaction, self.cursor);
                self.replace_interaction_timeline(projected.clone());
                changes.push(TaskChange::InteractionUpserted {
                    interaction: projected,
                });
            }
            TaskEvent::InteractionCancelled { interaction_id } => {
                let interaction = {
                    let interaction = self
                        .interactions
                        .get_mut(interaction_id)
                        .ok_or(DomainError::InteractionNotPending)?;
                    interaction.selected_choice_id = Some("cancelled".to_string());
                    interaction.clone()
                };
                let projected = interaction_projection(&interaction, self.cursor);
                self.replace_interaction_timeline(projected.clone());
                changes.push(TaskChange::InteractionUpserted {
                    interaction: projected,
                });
            }
            TaskEvent::MessageAppended { message } => {
                if self.title == "New task" && message.role == MessageRoleFact::User {
                    if let Some(title) = first_message_title(&message.text) {
                        self.title = title;
                    }
                }
                let item =
                    TimelineItemProjection::Message(message_projection(message, self.cursor));
                self.timeline.push(item.clone());
                changes.push(TaskChange::MessageAppended { item });
            }
            TaskEvent::MessagePatched {
                message_id,
                append_text,
                finalized,
                request_ids,
            } => {
                if let Some(TimelineItemProjection::Message(message)) = self.timeline.iter_mut().find(|item| {
                    matches!(item, TimelineItemProjection::Message(value) if value.message_id == *message_id)
                }) {
                    message.text.push_str(append_text);
                    message.finalized = *finalized;
                    if let Some(request_ids) = request_ids {
                        message.request_ids = request_ids.clone();
                    }
                }
                changes.push(TaskChange::MessagePatched {
                    message_id: message_id.clone(),
                    append_text: append_text.clone(),
                    finalized: *finalized,
                    request_ids: request_ids.clone(),
                });
            }
            TaskEvent::ActivityUpserted { activity } => {
                let projection = activity_projection(activity, self.cursor);
                self.timeline.retain(|item| {
                    !matches!(item, TimelineItemProjection::Activity(value) if value.activity_id == projection.activity_id)
                });
                self.timeline
                    .push(TimelineItemProjection::Activity(projection.clone()));
                changes.push(TaskChange::ActivityUpserted {
                    activity: projection,
                });
            }
            TaskEvent::SkillsChanged { selection } => {
                self.selected_tier_id = selection.tier_id.clone();
                self.loaded_skills = selection
                    .skill_ids
                    .iter()
                    .map(|skill_id| LoadedSkillProjection {
                        skill_id: skill_id.clone(),
                        name: skill_id.clone(),
                        description: String::new(),
                        loaded_by: "you".to_string(),
                    })
                    .collect();
                changes.push(TaskChange::SkillsChanged {
                    selected_tier_id: self.selected_tier_id.clone(),
                    loaded_skills: self.loaded_skills.clone(),
                });
            }
            TaskEvent::SkillLoaded(skill) => {
                if !self
                    .loaded_skills
                    .iter()
                    .any(|item| item.skill_id == skill.skill_id)
                {
                    self.loaded_skills.push(LoadedSkillProjection {
                        skill_id: skill.skill_id.clone(),
                        name: skill.skill_id.clone(),
                        description: String::new(),
                        loaded_by: format!("{:?}", skill.origin).to_lowercase(),
                    });
                }
                changes.push(TaskChange::SkillsChanged {
                    selected_tier_id: self.selected_tier_id.clone(),
                    loaded_skills: self.loaded_skills.clone(),
                });
            }
            TaskEvent::ReviewSubmissionReferenced { submission } => {
                self.review_total = self.review_total.saturating_add(1);
                self.latest_submission_id = Some(submission.submission_id.clone());
            }
            TaskEvent::ReviewSubmissionRecorded(recorded) => {
                changes.push(TaskChange::ReviewSubmissionsChanged {
                    change: ReviewSubmissionsChanged {
                        submission: review_submission_projection(recorded),
                        total_submissions: self.review_total,
                    },
                });
            }
            TaskEvent::RequestSnapshot(_)
            | TaskEvent::RequestStarted(_)
            | TaskEvent::RequestCompleted(_)
            | TaskEvent::RequestFailed(_)
            | TaskEvent::ObservationRecorded(_) => {}
        }
        Ok(())
    }

    fn apply_run(
        &mut self,
        run: &RunFact,
        changes: &mut Vec<TaskChange>,
    ) -> Result<(), DomainError> {
        let previous = self.runs.get(&run.run_id).map(|entry| entry.fact.state);
        if let Some(from) = previous {
            if !legal_transition(from, run.state) {
                return Err(DomainError::InvalidTransition {
                    from,
                    to: run.state,
                });
            }
        }
        if run.state.is_active()
            && self
                .runs
                .values()
                .any(|entry| entry.fact.state.is_active() && entry.fact.run_id != run.run_id)
        {
            return Err(DomainError::TaskBusy);
        }
        self.runs
            .insert(run.run_id.clone(), RunEntry { fact: run.clone() });
        if previous.is_none() {
            self.latest_run_id = Some(run.run_id.clone());
        }
        changes.push(TaskChange::RunStateChanged {
            task: self.summary(),
            active_run: self.active_run_projection().map(Box::new),
            latest_run: self.latest_run_projection().map(Box::new),
        });
        Ok(())
    }

    fn replace_interaction_timeline(&mut self, interaction: InteractionProjection) {
        if let Some(TimelineItemProjection::Interaction(current)) = self.timeline.iter_mut().find(
            |item| {
                matches!(item, TimelineItemProjection::Interaction(value) if value.interaction_id == interaction.interaction_id)
            },
        ) {
            let mut interaction = interaction;
            interaction.order_key = current.order_key;
            *current = interaction;
        }
    }

    fn set_run_state(&mut self, run_id: &RunId, state: RunState) -> Result<(), DomainError> {
        let run = self
            .runs
            .get(run_id)
            .ok_or(DomainError::TaskNotFound)?
            .fact
            .clone();
        if !legal_transition(run.state, state) {
            return Err(DomainError::InvalidTransition {
                from: run.state,
                to: state,
            });
        }
        let mut updated = run;
        updated.state = state;
        self.runs.insert(run_id.clone(), RunEntry { fact: updated });
        Ok(())
    }

    fn run_state_change(&self) -> TaskChange {
        TaskChange::RunStateChanged {
            task: self.summary(),
            active_run: self.active_run_projection().map(Box::new),
            latest_run: self.latest_run_projection().map(Box::new),
        }
    }

    pub fn summary(&self) -> TaskSummary {
        let state = self
            .active_run_projection()
            .map(|run| run.state)
            .or_else(|| self.latest_run_projection().map(|run| run.state))
            .map(task_state)
            .unwrap_or(TaskState::Draft);
        TaskSummary {
            task_id: self.task_id.clone().unwrap_or_else(|| {
                TaskId::parse("tsk_000000000000000000000000").expect("valid placeholder")
            }),
            workspace_id: self.workspace_id.clone().unwrap_or_else(|| {
                WorkspaceId::parse("wsp_000000000000000000000000").expect("valid placeholder")
            }),
            title: self.title.clone(),
            state,
            updated_at: self.updated_at.clone(),
            active_run_id: self.active_run_projection().map(|run| run.run_id),
            latest_run_id: self.latest_run_projection().map(|run| run.run_id),
        }
    }

    pub fn projection(&self) -> Result<TaskProjection, DomainError> {
        if self.task_id.is_none() {
            return Err(DomainError::TaskNotCreated);
        }
        let bounded = bounded_timeline_suffix(
            &self.timeline,
            self.timeline.len(),
            500,
            |candidate_start| {
                let cursor = self.timeline_cursor(candidate_start)?;
                serde_json::to_vec(&self.projection_value(Vec::new(), cursor))
                    .map(|encoded| encoded.len())
                    .map_err(|_| DomainError::LedgerCorrupt)
            },
        );
        let (start, timeline) = match bounded {
            Ok(window) => window,
            Err(DomainError::PayloadTooLarge) => (self.timeline.len(), Vec::new()),
            Err(error) => return Err(error),
        };
        Ok(self.projection_value(timeline, self.timeline_cursor(start)?))
    }

    pub(super) fn validate_timeline_changes(
        &self,
        changes: &[TaskChange],
    ) -> Result<(), DomainError> {
        for change in changes {
            let item = match change {
                TaskChange::MessageAppended { item } => Some(item.clone()),
                TaskChange::MessagePatched { message_id, .. } => self
                    .timeline
                    .iter()
                    .find(|item| {
                        matches!(item, TimelineItemProjection::Message(message) if message.message_id == *message_id)
                    })
                    .cloned(),
                TaskChange::ActivityUpserted { activity } => {
                    Some(TimelineItemProjection::Activity(activity.clone()))
                }
                TaskChange::InteractionUpserted { interaction } => {
                    Some(TimelineItemProjection::Interaction(interaction.clone()))
                }
                _ => None,
            };
            let Some(item) = item else {
                continue;
            };
            let start = self.timeline.len().saturating_sub(1);
            let projection = self.projection_value(vec![item], self.timeline_cursor(start)?);
            ensure_bounded(&projection)?;
        }
        Ok(())
    }

    fn timeline_cursor(&self, start: usize) -> Result<Option<PageCursor>, DomainError> {
        if start == 0 {
            return Ok(None);
        }
        Ok(Some(super::store::timeline_cursor(
            super::store::TimelineCursor {
                task_id: self
                    .task_id
                    .as_ref()
                    .expect("created task has an id")
                    .clone(),
                limit: 500,
                snapshot_cursor: self.cursor.get(),
                end: start,
            },
        )?))
    }

    fn projection_value(
        &self,
        timeline: Vec<TimelineItemProjection>,
        timeline_next_cursor: Option<PageCursor>,
    ) -> TaskProjection {
        TaskProjection {
            api_version: ApiVersion,
            task_revision: self.revision,
            cursor: self.cursor,
            task: self.summary(),
            selected_tier_id: self.selected_tier_id.clone(),
            timeline,
            timeline_next_cursor,
            loaded_skills: self.loaded_skills.clone(),
            active_run: self.active_run_projection(),
            latest_run: self.latest_run_projection(),
            context_summary: None,
            review_summary: ReviewSummaryProjection {
                total_submissions: self.review_total,
                latest_submission_id: self.latest_submission_id.clone(),
            },
        }
    }
}

fn event_task_id(event: &TaskEvent) -> Option<&TaskId> {
    match event {
        TaskEvent::TaskCreated { task_id, .. } => Some(task_id),
        TaskEvent::RunQueued { run }
        | TaskEvent::RunStarted { run }
        | TaskEvent::RunNeedsAttention { run }
        | TaskEvent::RunStopping { run }
        | TaskEvent::RunCompleted { run }
        | TaskEvent::RunStopped { run }
        | TaskEvent::RunFailed { run }
        | TaskEvent::RunInterrupted { run } => Some(&run.task_id),
        TaskEvent::MessageAppended { message } => Some(&message.task_id),
        TaskEvent::SkillLoaded(skill) => Some(&skill.execution.task_id),
        TaskEvent::ReviewSubmissionReferenced { submission } => Some(&submission.task_id),
        TaskEvent::ReviewSubmissionRecorded(submission) => Some(&submission.task_id),
        TaskEvent::RequestSnapshot(snapshot) => Some(&snapshot.scope.execution.task_id),
        TaskEvent::RequestStarted(request) => Some(&request.scope.execution.task_id),
        TaskEvent::RequestCompleted(request) => Some(&request.scope.execution.task_id),
        TaskEvent::RequestFailed(request) => Some(&request.scope.execution.task_id),
        TaskEvent::ObservationRecorded(observation) => Some(&observation.scope.execution.task_id),
        TaskEvent::TaskTitleChanged { .. }
        | TaskEvent::InteractionOpened { .. }
        | TaskEvent::InteractionResolved { .. }
        | TaskEvent::InteractionCancelled { .. }
        | TaskEvent::MessagePatched { .. }
        | TaskEvent::ActivityUpserted { .. }
        | TaskEvent::SkillsChanged { .. } => None,
    }
}

pub(super) fn bounded_timeline_suffix(
    items: &[TimelineItemProjection],
    end: usize,
    limit: usize,
    mut envelope_size: impl FnMut(usize) -> Result<usize, DomainError>,
) -> Result<(usize, Vec<TimelineItemProjection>), DomainError> {
    if end > items.len() {
        return Err(DomainError::InvalidRequest);
    }
    let mut start = end;
    let mut encoded_bytes = 2_usize;
    while start > 0 && end - start < limit {
        let item_bytes = serde_json::to_vec(&items[start - 1])
            .map_err(|_| DomainError::LedgerCorrupt)?
            .len();
        let separator = usize::from(start < end);
        let candidate_start = start - 1;
        let candidate_bytes = encoded_bytes + separator + item_bytes;
        let envelope_bytes = envelope_size(candidate_start)?;
        if envelope_bytes - 2 + candidate_bytes > MAX_TASK_DTO_BYTES {
            break;
        }
        encoded_bytes = candidate_bytes;
        start = candidate_start;
    }
    if start == end && end > 0 {
        return Err(DomainError::PayloadTooLarge);
    }
    Ok((start, items[start..end].to_vec()))
}

fn first_message_title(message: &str) -> Option<String> {
    let line = message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    Some(line.chars().take(80).collect())
}

pub(super) fn system_time_rfc3339(time: std::time::SystemTime) -> Result<String, DomainError> {
    let elapsed = time
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| DomainError::StorageExhausted)?;
    let seconds = elapsed.as_secs();
    let days = (seconds / 86_400) as i64;
    let second_of_day = seconds % 86_400;
    let (year, month, day) = civil_date(days);
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:09}Z",
        second_of_day / 3_600,
        second_of_day % 3_600 / 60,
        second_of_day % 60,
        elapsed.subsec_nanos()
    ))
}

fn civil_date(days_since_epoch: i64) -> (i64, u32, u32) {
    let days = days_since_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month as u32, day as u32)
}

fn legal_transition(from: RunState, to: RunState) -> bool {
    matches!(
        (from, to),
        (
            RunState::Queued,
            RunState::Running | RunState::Stopping | RunState::Failed | RunState::Interrupted
        ) | (
            RunState::Running,
            RunState::NeedsAttention
                | RunState::Stopping
                | RunState::Completed
                | RunState::Failed
                | RunState::Interrupted
        ) | (
            RunState::NeedsAttention,
            RunState::Running | RunState::Stopping | RunState::Failed | RunState::Interrupted
        ) | (
            RunState::Stopping,
            RunState::Stopped | RunState::Failed | RunState::Interrupted
        )
    )
}

fn task_state(state: RunState) -> TaskState {
    match state {
        RunState::Queued => TaskState::Queued,
        RunState::Running => TaskState::Running,
        RunState::NeedsAttention => TaskState::NeedsAttention,
        RunState::Stopping => TaskState::Stopping,
        RunState::Completed => TaskState::Completed,
        RunState::Stopped => TaskState::Stopped,
        RunState::Failed => TaskState::Failed,
        RunState::Interrupted => TaskState::Interrupted,
    }
}

impl TaskAggregate {
    fn active_run_projection(&self) -> Option<RunProjection> {
        self.runs
            .values()
            .find(|entry| entry.fact.state.is_active())
            .map(|entry| run_projection(&entry.fact))
    }

    fn latest_run_projection(&self) -> Option<RunProjection> {
        self.latest_run_id
            .as_ref()
            .and_then(|run_id| self.runs.get(run_id))
            .map(|entry| run_projection(&entry.fact))
    }
}

fn run_projection(run: &RunFact) -> RunProjection {
    RunProjection {
        run_id: run.run_id.clone(),
        state: run.state,
        started_at: run.started_at.clone(),
        finished_at: run.finished_at.clone(),
        completion: run.summary.as_ref().map(|summary| CompletionProjection {
            summary: summary.clone(),
            warnings: run.warnings.clone(),
            checks: run.checks.as_ref().map(|checks| {
                checks
                    .iter()
                    .map(|check| CheckProjection {
                        name: check.name.clone(),
                        passed: check.passed,
                        detail: check.detail.clone(),
                    })
                    .collect()
            }),
            metrics: run.metrics.as_ref().map(|metrics| {
                metrics
                    .iter()
                    .map(|metric| MetricProjection {
                        name: metric.name.clone(),
                        value: metric.value.get(),
                        unit: metric.unit.clone(),
                    })
                    .collect()
            }),
            workspace_changes: run
                .workspace_changes
                .as_ref()
                .map(|changes| ReviewSnapshot {
                    api_version: ApiVersion,
                    workspace_id: changes.workspace_id.clone(),
                    revision: changes.revision.clone(),
                    availability: match changes.availability {
                        ChangesAvailabilityFact::Available => ChangesAvailability::Available,
                        ChangesAvailabilityFact::NotGitRepository => {
                            ChangesAvailability::NotGitRepository
                        }
                        ChangesAvailabilityFact::GitUnavailable => {
                            ChangesAvailability::GitUnavailable
                        }
                        ChangesAvailabilityFact::WorkspaceRootMismatch => {
                            ChangesAvailability::WorkspaceRootMismatch
                        }
                    },
                    entries: changes
                        .entries
                        .iter()
                        .map(|entry| ChangeEntry {
                            path: entry.path.as_str().to_owned(),
                            old_path: entry.old_path.as_ref().map(|path| path.as_str().to_owned()),
                            kind: match entry.kind {
                                ChangeKindFact::Added => ChangeKind::Added,
                                ChangeKindFact::Modified => ChangeKind::Modified,
                                ChangeKindFact::Deleted => ChangeKind::Deleted,
                                ChangeKindFact::Untracked => ChangeKind::Untracked,
                                ChangeKindFact::Renamed => ChangeKind::Renamed,
                                ChangeKindFact::Copied => ChangeKind::Copied,
                                ChangeKindFact::TypeChanged => ChangeKind::TypeChanged,
                                ChangeKindFact::Conflicted => ChangeKind::Conflicted,
                            },
                            staged: entry.staged,
                            worktree: entry.worktree,
                            binary: entry.binary,
                            additions: entry.additions,
                            deletions: entry.deletions,
                            revision: entry.revision.clone(),
                        })
                        .collect(),
                    next_cursor: changes
                        .next_cursor
                        .as_ref()
                        .and_then(|cursor| PageCursor::try_new(cursor.clone()).ok()),
                }),
        }),
    }
}

fn message_projection(message: &MessageFact, order_key: kuku::event::Cursor) -> MessageProjection {
    MessageProjection {
        message_id: message.message_id.clone(),
        role: match message.role {
            MessageRoleFact::User => MessageRole::User,
            MessageRoleFact::Agent => MessageRole::Agent,
        },
        text: message.text.clone(),
        finalized: message.finalized,
        request_ids: message.request_ids.clone(),
        file_references: message
            .file_references
            .iter()
            .map(file_reference_projection)
            .collect(),
        order_key,
    }
}

fn activity_projection(
    activity: &ActivityFact,
    order_key: kuku::event::Cursor,
) -> ActivityProjection {
    ActivityProjection {
        activity_id: activity.activity_id.clone(),
        kind: match activity.kind {
            ActivityKindFact::Tool => ActivityKind::Tool,
            ActivityKindFact::DelegatedAgent => ActivityKind::DelegatedAgent,
            ActivityKindFact::System => ActivityKind::System,
        },
        title: activity.title.clone(),
        status: match activity.status {
            ActivityStatusFact::Pending => ActivityStatus::Pending,
            ActivityStatusFact::Running => ActivityStatus::Running,
            ActivityStatusFact::Completed => ActivityStatus::Completed,
            ActivityStatusFact::Failed => ActivityStatus::Failed,
        },
        detail: activity.detail.clone(),
        file_references: activity
            .file_references
            .iter()
            .map(file_reference_projection)
            .collect(),
        order_key,
    }
}

fn interaction_projection(
    interaction: &InteractionFact,
    order_key: kuku::event::Cursor,
) -> InteractionProjection {
    InteractionProjection {
        interaction_id: interaction.interaction_id.clone(),
        prompt: interaction.prompt.clone(),
        choices: interaction
            .choices
            .iter()
            .map(|choice| InteractionChoiceProjection {
                choice_id: choice.choice_id.clone(),
                label: choice.label.clone(),
            })
            .collect(),
        selected_choice_id: interaction.selected_choice_id.clone(),
        status: if interaction.selected_choice_id.is_some() {
            InteractionStatus::Resolved
        } else {
            InteractionStatus::Pending
        },
        order_key,
    }
}

fn review_submission_projection(recorded: &ReviewSubmissionRecorded) -> ReviewSubmissionProjection {
    ReviewSubmissionProjection {
        submission_id: recorded.submission_id.clone(),
        task_id: recorded.task_id.clone(),
        run_id: recorded.run_id.clone(),
        task_revision: recorded.task_revision,
        submitted_at: recorded.submitted_at.clone(),
        notes: recorded
            .notes
            .iter()
            .map(|note| SubmittedReviewNote {
                path: note.path.clone(),
                revision: note.revision.clone(),
                side: note.side,
                start_line: note.start_line,
                end_line: note.end_line,
                excerpt: note.excerpt.clone(),
                comment: note.comment.clone(),
                status: AnnotationStatus::Current,
            })
            .collect(),
    }
}

fn file_reference_projection(reference: &FileReferenceFact) -> FileReferenceProjection {
    FileReferenceProjection {
        workspace_id: reference.workspace_id.clone(),
        relative_path: reference.relative_path.as_str().to_owned(),
        label: reference.label.clone(),
    }
}
