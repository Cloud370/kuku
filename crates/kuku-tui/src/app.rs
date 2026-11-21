use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kuku_client::{
    new_id, Action, AgentConfig, AgentStatus, Error, HistoryKind, HistoryPage, ModelSelection,
    OutputReference, Snapshot, Target, UsageTotals,
};
use ratatui::layout::Rect;

use crate::config::StatusLineSettings;
use crate::contextual::{ContextFocus, ContextHit};
use crate::editor::Editor;
use crate::jobs::{job_status_label, Job, Restarted, Selection, Update};
use crate::menu::{Choice, ChoiceKind, CommandMenu, Picker, Prompt};
use crate::output::OutputViewer;
use crate::selection::TextSelection;
use crate::statusline::StatusLineEditor;
use crate::transcript::{HistoryRequest, Transcript};
use model_default::PendingDefaultSave;

mod activity;
pub(crate) mod agents;
mod auth_repair;
mod background_jobs;
mod contextual;
mod delivery;
mod history;
mod model_default;
mod model_editor;
mod navigation;
mod notice;
mod pending;
mod reading;
mod rewind;
mod thinking;
mod timing;
mod values;

pub(crate) use activity::{Activity, OutputTokens, ServiceStatus, SubmissionStatus};
use activity::{ExecutionProgress, Watch};
use agents::AgentActivities;
use delivery::{Deliveries, PendingQueueAction};
pub(crate) use delivery::{DeliveryRow, QueueAction};
use navigation::AgentNavigation;
pub(crate) use notice::Notice;
pub(crate) use pending::{PendingItem, PendingStage};
pub(crate) use reading::KeyboardFocus;
pub(crate) use thinking::ThinkingPicker;
pub(crate) use timing::TurnTiming;

const DEFAULT_AGENT: &str = "main";
const CLEANUP_REFRESH: Duration = Duration::from_secs(1);
const CACHED_PANES: usize = 4;

#[derive(Debug, Clone, Copy)]
enum SnapshotRefresh {
    Current,
    AfterCommit(u64),
    Progress,
}

impl SnapshotRefresh {
    fn needed(self, snapshot: Option<&Snapshot>) -> bool {
        match self {
            Self::Current | Self::Progress => true,
            Self::AfterCommit(id) => snapshot.is_none_or(|snapshot| snapshot.cursor < id),
        }
    }

    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Progress, _) | (_, Self::Progress) => Self::Progress,
            (Self::AfterCommit(first), Self::AfterCommit(second)) => {
                Self::AfterCommit(first.max(second))
            }
            (Self::Current, _) | (_, Self::Current) => Self::Current,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Address {
    pub(crate) session: String,
    pub(crate) agent: String,
}

impl Address {
    pub(crate) fn new(session: String, agent: String) -> Self {
        Self { session, agent }
    }

    pub(crate) fn main(session: String) -> Self {
        Self::new(session, DEFAULT_AGENT.to_owned())
    }

    fn from_target(target: &Target) -> Self {
        Self::new(target.session_id.clone(), target.agent.clone())
    }
}

#[derive(Debug, Default)]
pub(crate) struct Pane {
    pub(crate) exists: bool,
    pub(crate) title: String,
    pub(crate) draft: Editor,
    pub(crate) question_drafts: HashMap<String, Editor>,
    pub(crate) draft_revision: u64,
    pub(crate) transcript: Transcript,
    pub(crate) deliveries: Deliveries,
    pub(crate) keyboard_focus: KeyboardFocus,
    pub(crate) contextual_focus: ContextFocus,
    pub(crate) selected_queue_input: Option<String>,
    pub(crate) focused_request: Option<String>,
    pub(crate) permission_choice: Option<bool>,
    pub(crate) request_scroll: usize,
    pub(crate) request_detail_page_size: usize,
    pub(crate) snapshot: Option<Box<Snapshot>>,
    observed_timing: Option<(u64, TurnTiming)>,
    pub(crate) pending_reply_copy: Option<u64>,
    pub(crate) pending_selected_reply_copy: Option<u64>,
    pub(crate) initial_config: AgentConfig,
    initial_model: Option<ModelSelection>,
    initial_thinking_refresh: bool,
    config_receipt: Option<(u64, Box<AgentConfig>)>,
    pub(crate) model_problem: Option<String>,
    pub(crate) opening: bool,
    pub(crate) sending: bool,
    pub(crate) value_attachments: Vec<(String, OutputReference)>,
    pub(crate) value_saving: bool,
    creation_rejected: bool,
    stopping: Option<u64>,
    watch: Watch,
    progress: ExecutionProgress,
    submission: Option<(u64, SubmissionStatus)>,
    request_usage_floor: u64,
    last_event: u64,
    live_floor: u64,
    latest_reply_cursor: u64,
    retired_execution_floor: u64,
    execution: Option<u64>,
    latest_execution_started: Option<u64>,
    refresh_due: Option<SnapshotRefresh>,
    refresh_in_flight: bool,
    cleanup_at: Option<Instant>,
    history_in_flight: Option<HistoryRequest>,
    history_invalidated: bool,
}

impl Pane {
    pub(crate) fn execution(&self) -> Option<u64> {
        self.execution
    }

    fn interruption_pending(&self) -> bool {
        self.stopping.is_some() && self.stopping == self.execution
    }

    fn finish_interrupt(&mut self, execution: u64) {
        if self.stopping == Some(execution) {
            self.stopping = None;
        }
    }

    pub(crate) fn observation_problem(&self) -> Option<&str> {
        self.watch.problem()
    }

    pub(crate) fn current_request_usage(&self) -> Option<&UsageTotals> {
        self.snapshot
            .as_ref()
            .filter(|snapshot| snapshot.cursor >= self.request_usage_floor)
            .map(|snapshot| &snapshot.agent.usage)
    }

    pub(crate) fn config(&self) -> &AgentConfig {
        if let Some((_, config)) = &self.config_receipt {
            return config;
        }
        self.snapshot
            .as_ref()
            .map_or(&self.initial_config, |snapshot| &snapshot.agent.config)
    }

    pub(crate) fn reasoning(
        &self,
    ) -> Option<(&kuku::ProviderSettings, &kuku::ProviderCapabilities)> {
        if self.waiting_for_configuration() {
            return None;
        }
        self.snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .pending_selection
                    .as_ref()
                    .map(|model| (&model.settings, &model.capabilities))
            })
            .or_else(|| self.effective_reasoning())
    }

    pub(crate) fn waiting_for_configuration(&self) -> bool {
        self.config_receipt.is_some() || self.initial_thinking_refresh
    }

    pub(crate) fn effective_model_identity(&self) -> Option<(&str, &str, Option<&str>)> {
        if let Some(snapshot) = &self.snapshot {
            snapshot.selection.as_ref().map(|model| {
                (
                    model.model.as_str(),
                    model.connection.as_str(),
                    model.settings.reasoning_effort.as_deref(),
                )
            })
        } else {
            self.initial_model.as_ref().map(|model| {
                (
                    model.model.as_str(),
                    model.connection.as_str(),
                    model.settings.reasoning_effort.as_deref(),
                )
            })
        }
    }

    pub(crate) fn effective_model_label(&self) -> Option<String> {
        if let Some((model, connection, effort)) = self.effective_model_identity() {
            Some(model_label(connection, model, effort))
        } else if self.snapshot.is_none() {
            self.config()
                .tier
                .as_ref()
                .map(|tier| format!("Tier {tier}"))
        } else {
            None
        }
    }

    pub(crate) fn next_model_label(&self, width: usize) -> Option<String> {
        if self.next_thinking().is_some() {
            return None;
        }
        let model = self.snapshot.as_ref()?.pending_selection.as_ref()?;
        let route = crate::text::single_line(&format!("{}/{}", model.connection, model.model));
        let effort = crate::text::effort_label(model.settings.reasoning_effort.as_deref());
        let prefix = "Next model request · ";
        Some(format!(
            "{prefix}{}",
            crate::text::with_suffix(&route, &effort, width.saturating_sub(prefix.len()))
        ))
    }

    fn model_route_changed(&self, config: &AgentConfig) -> bool {
        let current = self.config();
        (&config.tier, &config.connection, &config.model)
            != (&current.tier, &current.connection, &current.model)
    }

    fn set_snapshot(&mut self, snapshot: Box<Snapshot>) -> Option<u64> {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|current| current.cursor > snapshot.cursor)
        {
            return None;
        }
        let mut execution_started = None;
        if snapshot.cursor >= self.last_event {
            let execution = snapshot.execution_id();
            if execution != self.execution {
                self.progress = ExecutionProgress::default();
                self.retired_execution_floor = self.retired_execution_floor.max(
                    execution.map_or(snapshot.cursor, |execution| execution.saturating_sub(1)),
                );
                self.execution = execution;
            }
            if let Some(execution) = execution {
                self.latest_execution_started = Some(
                    self.latest_execution_started
                        .map_or(execution, |latest| latest.max(execution)),
                );
                execution_started = Some(execution);
            }
        }
        if self
            .config_receipt
            .as_ref()
            .is_some_and(|(cursor, _)| snapshot.cursor >= *cursor)
        {
            self.config_receipt = None;
        }
        self.model_problem = snapshot
            .selection_error
            .as_ref()
            .or(snapshot.pending_selection_error.as_ref())
            .map(|error| error.message.clone());
        self.cleanup_at = snapshot
            .cleaning_up
            .then(|| Instant::now() + CLEANUP_REFRESH);
        self.deliveries.snapshot(&snapshot);
        self.snapshot = Some(snapshot);
        execution_started
    }
}

fn model_label(connection: &str, model: &str, effort: Option<&str>) -> String {
    let route = format!("{connection}/{model}");
    effort.map_or_else(|| route.clone(), |effort| format!("{route} · {effort}"))
}

#[derive(Debug)]
pub(crate) enum Modal {
    Help,
    Picker(Picker),
    Prompt(Prompt),
    Output(Box<OutputViewer>),
    StatusLineEditor(StatusLineEditor),
    ExitConfirmation,
}

impl Modal {
    pub(crate) fn composer_anchored(&self) -> bool {
        match self {
            Self::Picker(picker) => picker.kind.composer_anchored(),
            Self::ExitConfirmation => true,
            Self::Help | Self::Prompt(_) | Self::Output(_) | Self::StatusLineEditor(_) => false,
        }
    }
}

pub(crate) struct App {
    pub(crate) active: Address,
    pub(crate) panes: HashMap<Address, Pane>,
    pub(crate) related_agents: AgentActivities,
    pub(crate) workspace: PathBuf,
    pub(crate) endpoint: Option<String>,
    pub(crate) remote: bool,
    pub(crate) model_editor: Option<Box<crate::setup::ModelEditor>>,
    pub(crate) credential_repair: Option<Box<crate::setup::CredentialRepair>>,
    pub(crate) connection_problem: Option<String>,
    pub(crate) connecting: bool,
    pub(crate) restarting: bool,
    pub(crate) notice: Option<Notice>,
    pub(crate) alias_hint: Option<Notice>,
    pub(crate) taught_commands: HashSet<CommandMenu>,
    pub(crate) modal: Option<Modal>,
    pub(crate) statusline_settings: StatusLineSettings,
    pub(crate) text_selection: TextSelection,
    pub(crate) modal_selection: TextSelection,
    pub(crate) pending: HashMap<String, PendingItem>,
    pub(crate) withdrawal_drafts: HashMap<(Target, String), String>,
    pub(crate) rewind_drafts: HashMap<String, RewindDraft>,
    pub(crate) editor_width: usize,
    pub(crate) transcript_area: (usize, usize),
    pub(crate) delivery_area: Rect,
    pub(crate) delivery_hits: Vec<(Range<u16>, String)>,
    pub(crate) request_area: Rect,
    pub(crate) request_hit: Option<(Target, String)>,
    pub(crate) permission_choice_areas: [Rect; 2],
    pub(crate) contextual_hits: Vec<ContextHit>,
    pub(crate) reading_area: Rect,
    pub(crate) composer_area: Rect,
    pub(crate) recipient_area: Rect,
    pub(crate) modal_editor_width: usize,
    pub(crate) modal_page: usize,
    pub(crate) modal_info_rows: usize,
    pub(crate) quit: bool,
    pub(crate) effects: Vec<Job>,
    generation: u64,
    next_modal: u64,
    model_version: u64,
    pending_model_default: Option<PendingDefaultSave>,
    saving_model_default: Option<(u64, Selection)>,
    pending_queue_action: Option<PendingQueueAction>,
    checking: VecDeque<String>,
    outstanding_checks: usize,
    quit_after_save: bool,
    resolved_workspace: bool,
    cached: VecDeque<Address>,
    navigation: AgentNavigation,
    pub(crate) escape_armed_at: Option<Instant>,
    pub(crate) rewind_restore_files: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct RewindDraft {
    pub(crate) address: Address,
    pub(crate) text: String,
    pub(crate) revision: u64,
}

impl App {
    pub(crate) fn new(workspace: PathBuf, endpoint: Option<String>) -> Self {
        let active = Address::main(new_id());
        let remote = endpoint.is_some();
        Self {
            panes: HashMap::from([(active.clone(), Pane::default())]),
            related_agents: AgentActivities::default(),
            cached: VecDeque::from([active.clone()]),
            active,
            workspace,
            remote,
            endpoint,
            connection_problem: None,
            connecting: true,
            restarting: false,
            notice: None,
            alias_hint: None,
            taught_commands: HashSet::new(),
            modal: None,
            statusline_settings: StatusLineSettings::default(),
            text_selection: TextSelection::default(),
            modal_selection: TextSelection::default(),
            pending: HashMap::new(),
            withdrawal_drafts: HashMap::new(),
            rewind_drafts: HashMap::new(),
            editor_width: 80,
            transcript_area: (80, 16),
            delivery_area: Rect::ZERO,
            delivery_hits: Vec::new(),
            request_area: Rect::ZERO,
            request_hit: None,
            permission_choice_areas: [Rect::ZERO; 2],
            contextual_hits: Vec::new(),
            reading_area: Rect::ZERO,
            composer_area: Rect::ZERO,
            recipient_area: Rect::ZERO,
            modal_editor_width: 60,
            modal_page: 6,
            modal_info_rows: 4,
            quit: false,
            effects: vec![Job::Connect],
            model_editor: None,
            credential_repair: None,
            generation: 0,
            next_modal: 0,
            model_version: 0,
            pending_model_default: None,
            saving_model_default: None,
            pending_queue_action: None,
            checking: VecDeque::new(),
            outstanding_checks: 0,
            quit_after_save: false,
            resolved_workspace: false,
            navigation: AgentNavigation::default(),
            escape_armed_at: None,
            rewind_restore_files: false,
        }
    }

    pub(crate) fn pane(&self) -> &Pane {
        self.panes.get(&self.active).expect("active pane")
    }

    pub(crate) fn pane_mut(&mut self) -> &mut Pane {
        self.panes.get_mut(&self.active).expect("active pane")
    }

    pub(crate) fn available(&self) -> bool {
        !self.connecting && self.connection_problem.is_none()
    }

    pub(crate) fn resolved_workspace(&self) -> Option<&Path> {
        if !self.resolved_workspace {
            return None;
        }
        let configured = self.pane().snapshot.as_ref().and_then(|snapshot| {
            let agent = &snapshot.agent;
            agent
                .active_config
                .as_ref()
                .unwrap_or(&agent.config)
                .workspace
                .as_deref()
        });
        Some(configured.unwrap_or(&self.workspace))
    }

    pub(crate) fn selection(&self) -> Selection {
        Selection {
            generation: self.generation,
            target: self.target(&self.active),
        }
    }

    pub(crate) fn modal_id(&mut self) -> u64 {
        self.next_modal = self.next_modal.wrapping_add(1);
        self.next_modal
    }

    pub(crate) fn require_service(&mut self) -> bool {
        if self.available() {
            true
        } else {
            self.problem(if self.connecting {
                "Connecting to the kuku service…"
            } else {
                "kuku service unavailable · Ctrl+R or /server"
            });
            false
        }
    }

    pub(crate) fn new_draft(&mut self) {
        self.select(Address::main(new_id()), false, String::new());
    }

    pub(crate) fn interrupt(&mut self) {
        if self.pane().interruption_pending() {
            return;
        }
        if let Some(execution_id) = self.pane().execution() {
            if self.require_service() {
                self.pane_mut().stopping = Some(execution_id);
                self.issue(self.active.clone(), Action::Cancel { execution_id });
            }
        }
    }

    pub(crate) fn exit(&mut self) {
        if self
            .pending
            .values()
            .any(|item| matches!(item.stage, PendingStage::Saving))
            || self.panes.values().any(|pane| pane.value_saving)
        {
            self.quit_after_save = true;
            self.message("Saving pending input before leaving…");
        } else {
            self.quit = true;
        }
    }

    fn restart_prepared(
        &mut self,
        ticket: u64,
        result: Result<kuku_client::local::ServiceIdentity, Error>,
    ) {
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        if picker.kind != crate::menu::PickerKind::Restart || picker.ticket.id != ticket {
            return;
        }
        picker.loading = false;
        match result {
            Ok(identity) => {
                picker.header = format!(
                    "Restart the managed local kuku service at {}?",
                    identity.endpoint
                );
                picker.set_local(vec![
                    Choice::new(
                        "Restart local service",
                        "Restart when current work is settled",
                        ChoiceKind::ConfirmRestart(Box::new(identity)),
                    ),
                    Choice::new("Cancel", "", ChoiceKind::Close),
                ]);
                picker.selected = 1;
            }
            Err(error) => picker.problem = Some(error.to_string()),
        }
    }

    pub(crate) fn reconnect(&mut self) {
        if self.connecting || self.restarting {
            return;
        }
        self.connecting = true;
        self.connection_problem = None;
        self.notice = None;
        self.related_agents.invalidate(self.pane().last_event);
        self.generation = self.generation.wrapping_add(1);
        self.effects.push(Job::Connect);
    }

    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.notice
            .as_ref()
            .and_then(Notice::deadline)
            .into_iter()
            .chain(self.alias_hint.as_ref().and_then(Notice::deadline))
            .chain(self.pane().cleanup_at)
            .chain(
                self.model_editor
                    .as_ref()
                    .and_then(|editor| editor.probe_deadline()),
            )
            .min()
    }

    pub(crate) fn tick(&mut self) -> bool {
        let now = Instant::now();
        let expired = Notice::expire(&mut self.notice, now);
        let alias_expired = Notice::expire(&mut self.alias_hint, now);
        if self
            .pane()
            .cleanup_at
            .is_some_and(|at| at <= Instant::now())
        {
            self.pane_mut().cleanup_at = None;
            self.refresh();
        }
        let probes = self
            .model_editor
            .as_mut()
            .is_some_and(|editor| editor.poll_probes());
        expired || alias_expired || probes
    }

    pub(crate) fn update(&mut self, update: Update) {
        match update {
            Update::Connected {
                endpoint,
                workspace,
            } => {
                self.connected(endpoint, workspace);
                self.credential_repair_connected();
            }
            Update::ConnectFailed(error) => {
                self.connect_failed(error);
                self.credential_repair_connect_failed();
            }
            Update::Loaded(selection, snapshot, page) if selection == self.selection() => {
                self.loaded(snapshot, page)
            }
            Update::WatchConnected(selection) if selection == self.selection() => {
                self.pane_mut().watch = Watch::Connected;
            }
            Update::WatchReconnecting(selection, error) if selection == self.selection() => {
                self.drop_watch(Watch::Reconnecting(format!(
                    "kuku service live updates interrupted · {error} · Ctrl+R to reconnect"
                )));
            }
            Update::WatchFailed(selection, error) if selection == self.selection() => {
                self.drop_watch(Watch::Failed(format!(
                    "kuku service live updates unavailable · {error} · Ctrl+R to reconnect"
                )));
            }
            Update::Observation(selection, event) if selection == self.selection() => {
                self.observe(event)
            }
            Update::Refreshed(selection, result) if selection == self.selection() => {
                self.refreshed(result)
            }
            Update::History(selection, request, result) => {
                self.apply_history_update(selection, request, result);
            }
            Update::RewindHistory(selection, ticket, before, result) => {
                self.apply_rewind_history_update(selection, ticket, before, result);
            }
            Update::RewindSource(selection, ticket, event_id, result) => {
                self.apply_rewind_source_update(selection, ticket, event_id, result);
            }
            Update::RewindGroupHistory(selection, request, result) => {
                self.apply_rewind_group_history_update(selection, request, result);
            }
            Update::Found(ticket, result) => {
                if let Some(Modal::Picker(picker)) = &mut self.modal {
                    if picker.ticket == ticket {
                        match result {
                            Ok((choices, status)) => picker.set_results(choices, status),
                            Err(error) => {
                                picker.loading = false;
                                picker.problem = Some(error.to_string());
                            }
                        }
                    }
                }
            }
            Update::ThinkingDefaults(ticket, defaults) => self.thinking_defaults(ticket, defaults),
            Update::ModelEditorOpened(ticket, result) => self.model_editor_opened(ticket, result),
            Update::ModelEditorSaved(ticket, editor, result) => {
                self.model_editor_saved(ticket, editor, result)
            }
            Update::ModelSettingSaved(result) => self.model_setting_saved(result),
            Update::PreparedCredentialRepair(ticket, selection, result) => {
                self.credential_repair_prepared(ticket, selection, result)
            }
            Update::SavedCredentialRepair(ticket, selection, result) => {
                self.credential_repair_saved(ticket, selection, result)
            }
            Update::PreparedDefault(ticket, selection, result) => {
                self.prepared_model_default(ticket, selection, result)
            }
            Update::SavedDefault(ticket, selection, result) => {
                self.saved_model_default(ticket, selection, result)
            }
            Update::Saved(command, result) => self.saved(command, result),
            Update::ValueSaved(selection, text, result) => {
                self.value_saved(selection, text, result)
            }
            Update::BackgroundJobCancelled(selection, result) if selection == self.selection() => {
                match result {
                    Ok(job) => {
                        self.confirmation(format!("{} · {}", job.id, job_status_label(job.status)))
                    }
                    Err(error) => self.problem(format!("Cancel background job · {error}")),
                }
            }
            Update::Sent(id, result) => match result {
                Ok(ack) => self.acknowledged(&id, ack),
                Err(error) => self.command_failed(&id, error, false),
            },
            Update::Checked(id, result) => self.checked(&id, result),
            Update::Discarded(id, result) => self.discarded(&id, result),
            Update::Pending(result) => match result {
                Ok(commands) => self.recover_pending(commands),
                Err(error) => self.problem(format!("Client storage · {error} · Ctrl+R to recheck")),
            },
            Update::DefaultModel(version, result) => self.default_model_updated(version, result),
            Update::Output(selection, id, result) if selection == self.selection() => {
                if let Some(Modal::Output(viewer)) = &mut self.modal {
                    if viewer.id == id {
                        match result {
                            Ok(chunk) => viewer.apply(chunk),
                            Err(error) => {
                                viewer.loading = false;
                                viewer.problem = Some(format!("Saved output · {error} · R Retry"));
                            }
                        }
                    }
                }
            }
            Update::RestartPrepared(ticket, result) => self.restart_prepared(ticket, result),
            Update::Restarted(result) => {
                self.restarting = false;
                match result {
                    Ok(Restarted::Ready) => self.reconnect(),
                    Ok(Restarted::Busy(work)) => self.problem(format!("Restart deferred · {work}")),
                    Err(error) => self.problem(format!("Service management · {error}")),
                }
            }
            Update::Web(result) => match result {
                Ok(()) => self.message("Opened the browser"),
                Err(error) => self.problem(format!("Open browser · {error}")),
            },
            Update::ReplySource(selection, id, result) => self.copied_reply(selection, id, result),
            Update::Loaded(..)
            | Update::WatchConnected(..)
            | Update::WatchReconnecting(..)
            | Update::WatchFailed(..)
            | Update::Observation(..)
            | Update::Refreshed(..)
            | Update::BackgroundJobCancelled(..)
            | Update::Output(..) => {}
        }
        self.sync_pending_picker();
        self.sync_agent_picker();
        self.resume_queue_action();
    }

    fn target(&self, address: &Address) -> Target {
        Target {
            workspace: self.workspace.clone(),
            session_id: address.session.clone(),
            agent: address.agent.clone(),
        }
    }

    fn renew_draft_identity(&mut self) {
        let replacement = Address::main(new_id());
        let previous = std::mem::replace(&mut self.active, replacement.clone());
        let mut pane = self.panes.remove(&previous).expect("active draft");
        pane.creation_rejected = false;
        self.cached.retain(|address| address != &previous);
        self.panes.insert(replacement, pane);
        self.cache_active_pane();
    }

    fn connected(&mut self, endpoint: String, workspace: PathBuf) {
        self.connecting = false;
        self.endpoint = Some(endpoint);
        if self.resolved_workspace && self.workspace != workspace {
            self.connection_problem = Some(format!(
                "Workspace now resolves to {} · open a new terminal for this path",
                workspace.display()
            ));
            return;
        }
        self.workspace = workspace;
        self.resolved_workspace = true;
        self.connection_problem = None;
        self.begin_observing();
        self.effects.push(Job::Pending);
    }

    fn connect_failed(&mut self, error: Error) {
        self.connecting = false;
        let recovery = if self.remote {
            "Check the kuku service URL and token, then press Ctrl+R"
        } else {
            "Use Ctrl+R to reconnect, /server for status, or /restart"
        };
        self.connection_problem = Some(format!("kuku service connection · {error}\n{recovery}"));
    }

    fn begin_observing(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.model_version = self.model_version.wrapping_add(1);
        self.related_agents.invalidate(0);
        let pane = self.pane_mut();
        pane.opening = false;
        pane.watch = Watch::Detached;
        pane.refresh_in_flight = false;
        pane.refresh_due = None;
        pane.history_in_flight = None;
        pane.history_invalidated = false;
        if !self.available() {
            self.effects.push(Job::StopObserving);
        } else if self.pane().exists {
            let pane = self.pane_mut();
            pane.opening = true;
            pane.watch = Watch::Connecting;
            self.effects.push(Job::Observe(self.selection()));
        } else {
            self.effects.push(Job::StopObserving);
            self.preview_model();
        }
    }

    fn cache_active_pane(&mut self) {
        self.cached.retain(|address| address != &self.active);
        self.cached.push_back(self.active.clone());
        if self.cached.len() > CACHED_PANES {
            if let Some(old) = self
                .cached
                .pop_front()
                .and_then(|address| self.panes.get_mut(&address))
            {
                old.transcript.suspend();
                old.snapshot = None;
                old.initial_model = None;
            }
        }
    }

    fn loaded(&mut self, mut snapshot: Box<Snapshot>, page: HistoryPage) {
        let active = self.active.clone();
        if snapshot.session_id != active.session
            || snapshot.agent.address != active.agent
            || snapshot.workspace != self.workspace
        {
            self.problem("Service returned state for another recipient");
            return;
        }
        self.related_agents.invalidate(snapshot.cursor);
        self.related_agents.snapshot(&snapshot, &active.agent);
        let pane = self.pane_mut();
        pane.exists = true;
        pane.opening = false;
        let progress = ExecutionProgress::from_snapshot(snapshot.progress.as_ref());
        pane.last_event = snapshot.cursor;
        pane.live_floor = snapshot.cursor;
        pane.latest_reply_cursor = 0;
        pane.retired_execution_floor = snapshot
            .execution_id()
            .map_or(snapshot.cursor, |execution| execution.saturating_sub(1));
        if pane.snapshot.is_none() {
            for entry in &page.entries {
                if entry.kind == HistoryKind::User {
                    if let Some(text) = &entry.text {
                        pane.draft.remember(text);
                    }
                }
            }
        }
        if let Some(entry) =
            page.entries.iter().rev().find(|entry| {
                entry.kind == HistoryKind::User && entry.agent == snapshot.agent.address
            })
        {
            pane.record_submission(entry.id, SubmissionStatus::Accepted);
        }
        let reply = snapshot.reply_preview.take().filter(|reply| {
            snapshot.agent.status == AgentStatus::Running
                && snapshot.execution_id() == Some(reply.execution_id)
                && snapshot.agent.turn == Some(reply.turn)
        });
        let execution_started = pane.set_snapshot(snapshot);
        pane.progress = progress;
        pane.transcript.replace_recent(page);
        if let Some(reply) = reply {
            pane.apply_reply_preview(reply);
        }
        if pane.history_in_flight.is_none() {
            if let Some(request) = pane.transcript.restore_request() {
                pane.history_in_flight = Some(request);
                self.effects.push(Job::History(self.selection(), request));
            }
        }
        if let Some(execution_id) = execution_started {
            self.clear_execution_notice_on_start(&active, execution_id);
        }
    }

    fn drop_watch(&mut self, state: Watch) {
        self.related_agents.invalidate(self.pane().last_event);
        let pane = self.pane_mut();
        pane.opening = false;
        pane.watch = state;
        pane.progress = ExecutionProgress::from_snapshot(
            pane.snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.progress.as_ref()),
        );
        pane.transcript.clear_preview();
    }

    fn refresh(&mut self) {
        self.request_refresh(SnapshotRefresh::Current);
    }

    fn request_refresh(&mut self, refresh: SnapshotRefresh) {
        if !self.available() || !self.pane().exists {
            return;
        }
        let pane = self.pane_mut();
        if !refresh.needed(pane.snapshot.as_deref()) {
            return;
        }
        if pane.refresh_in_flight {
            pane.refresh_due = Some(pane.refresh_due.map_or(refresh, |due| due.merge(refresh)));
        } else {
            pane.refresh_in_flight = true;
            let selection = self.selection();
            self.effects.push(match refresh {
                SnapshotRefresh::Progress => Job::RefreshProgress(selection),
                SnapshotRefresh::Current | SnapshotRefresh::AfterCommit(_) => {
                    Job::Refresh(selection)
                }
            });
        }
    }

    fn refreshed(&mut self, result: Result<Box<Snapshot>, Error>) {
        let active = self.active.clone();
        if matches!(self.pane().watch, Watch::Connected | Watch::Connecting) {
            if let Ok(snapshot) = &result {
                self.related_agents.snapshot(snapshot, &active.agent);
            }
        }
        let pane = self.pane_mut();
        pane.refresh_in_flight = false;
        let refresh_due = pane.refresh_due.take();
        let execution_started = match result {
            Ok(mut snapshot) => {
                snapshot.reply_preview = None;
                pane.set_snapshot(snapshot)
            }
            Err(error) => {
                self.problem(format!("Agent state · {error} · Ctrl+R to reconnect"));
                None
            }
        };
        if let Some(refresh) = refresh_due {
            self.request_refresh(refresh);
        }
        if let Some(execution_id) = execution_started {
            self.clear_execution_notice_on_start(&active, execution_id);
        }
    }
}
