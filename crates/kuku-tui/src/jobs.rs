use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use kuku_client::local::BuildPolicy;
use kuku_client::local::RestartOutcome;
use kuku_client::{
    Acknowledgement, AgentConfig, Client, Command, Connection, ConnectionOptions, Error,
    HistoryDestinations, HistoryKind, HistoryPage, JobSnapshot, JobStatus, ModelCatalog,
    ModelSelection, NamedValue, NamedValueKind, NamedValueSource, Observation, OutputChunk,
    RewindGroupPage, Snapshot, Target,
};
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinSet};

use crate::menu::{Choice, ChoiceKind, Ticket};
use crate::model_default::{self, PreparedDefaultSave, ResolvedModel};
use crate::setup::{
    CredentialRepairApplied, CredentialRepairPlan, CredentialRepairRequest,
    PreparedCredentialRepair, SavedCredentialRepair,
};
use crate::text::{format_elapsed, single_line};
use crate::transcript::{HistoryRequest, REWIND_GROUP_PAGE, RewindGroupRequest};

mod connect;
mod history;
pub(crate) use history::LoadedHistory;
mod observe;
mod search;
use search::find;

const UPDATE_CAPACITY: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Selection {
    pub(crate) generation: u64,
    pub(crate) target: Target,
}

#[derive(Debug)]
pub(crate) enum Search {
    Skills(String),
    Paths(String),
    Sessions(String),
    Models {
        tier: Option<String>,
        route: Option<ResolvedModel>,
    },
    Values(Target),
    Jobs(Target),
    Server,
}

pub(crate) struct ValueSave {
    pub(crate) name: String,
    pub(crate) source: NamedValueSource,
    pub(crate) replace: bool,
    pub(crate) create: Option<AgentConfig>,
}

pub(crate) enum Job {
    OpenModelEditor(u64, crate::setup::SetupIntent),
    SaveModelEditor(u64, Box<crate::setup::ModelEditor>),
    SaveModelSetting(crate::setup::SavedModelSetting),
    SaveAndUseModel(Selection, Box<kuku_client::TierSummary>),
    PrepareCredentialRepair {
        ticket: u64,
        selection: Selection,
        request: CredentialRepairRequest,
    },
    SaveCredentialRepair {
        ticket: u64,
        selection: Selection,
        plan: std::sync::Arc<CredentialRepairPlan>,
        key: String,
    },
    RestartCredentialRepair {
        ticket: u64,
        selection: Selection,
        saved: SavedCredentialRepair,
    },
    Connect,
    WorkspacePermission {
        ticket: u64,
        workspace: PathBuf,
        mode: Option<kuku_client::PermissionMode>,
    },
    PendingApprovals(Selection),
    Observe(Selection),
    StopObserving,
    Refresh(Selection),
    RefreshProgress(Selection),
    History(Selection, HistoryRequest),
    RewindHistory(Selection, Ticket, u64),
    RewindSource(Selection, Ticket, u64),
    HistoryJump(Selection, Ticket, Option<u64>),
    HistoryJumpDraft(Selection, u64, u64),
    FileRewindPreview(Selection, u64, u64),
    RewindGroupHistory(Selection, RewindGroupRequest),
    Find(Ticket, Search),
    SendRouteLabels(u64, [String; 2]),
    AgentModels(u64),
    ThinkingDefaults(u64, AgentConfig, ResolvedModel),
    CustomSpeed(
        u64,
        Box<crate::app::speed::SpeedTarget>,
        crate::setup::service_tier::SpeedWork,
    ),
    ClosePicker,
    PrepareDefault {
        ticket: u64,
        selection: Selection,
        route: ResolvedModel,
    },
    SaveDefault {
        ticket: u64,
        selection: Selection,
        plan: PreparedDefaultSave,
    },
    Save(Command),
    SaveValue(Selection, Box<ValueSave>),
    CancelBackground(Selection, String),
    Send(Command),
    Check(Command),
    Discard(Command),
    Pending,
    Model(AgentConfig, u64),
    StartupSkills(u64),
    Output(Selection, u64, String, u64, usize),
    PrepareRestart(u64),
    Restart(Box<kuku_client::local::ServiceIdentity>),
    Web(Option<Target>),
    ReplySource(Selection, u64),
    Copy(String),
}

pub(crate) enum Update {
    SetupRequired,
    ModelEditorOpened(u64, Result<Box<crate::setup::ModelEditor>, String>),
    ModelEditorSaved(u64, Box<crate::setup::ModelEditor>, Result<bool, String>),
    ModelSettingSaved(Result<bool, String>),
    ModelDefaultAndSwitchSaved(
        Selection,
        Box<kuku_client::TierSummary>,
        Result<bool, String>,
    ),
    PreparedCredentialRepair(u64, Selection, Result<PreparedCredentialRepair, String>),
    SavedCredentialRepair(u64, Selection, Result<CredentialRepairApplied, String>),
    Connected {
        endpoint: String,
        workspace: PathBuf,
    },
    ConnectFailed(Error),
    WorkspacePermission {
        ticket: u64,
        saving: bool,
        result: Result<Option<kuku_client::PermissionMode>, Error>,
    },
    PendingApprovals(Selection, Result<Vec<kuku_client::PendingApproval>, Error>),
    Loaded(Selection, Box<Snapshot>, HistoryPage),
    WatchConnected(Selection),
    WatchReconnecting(Selection, String),
    WatchFailed(Selection, Error),
    Observation(Selection, Observation),
    Refreshed(Selection, Result<Box<Snapshot>, Error>),
    History(Selection, HistoryRequest, Result<LoadedHistory, Error>),
    RewindHistory(Selection, Ticket, u64, Result<HistoryPage, Error>),
    RewindSource(Selection, Ticket, u64, Result<String, Error>),
    HistoryJump(
        Selection,
        Ticket,
        Option<u64>,
        Result<HistoryDestinations, Error>,
    ),
    HistoryJumpDraft(Selection, u64, Result<String, Error>),
    FileRewindPreview(
        Selection,
        u64,
        Result<kuku_client::FileRewindPreview, Error>,
    ),
    RewindGroupHistory(
        Selection,
        RewindGroupRequest,
        Result<RewindGroupPage, Error>,
    ),
    Found(Ticket, Result<(Vec<Choice>, String), Error>),
    SendRouteLabels(u64, [Option<String>; 2]),
    AgentModels(u64, Result<ModelCatalog, Error>),
    ThinkingDefaults(u64, crate::setup::ThinkingDefaults),
    SpeedFinished(
        u64,
        Box<crate::app::speed::SpeedTarget>,
        Result<crate::setup::service_tier::SpeedResult, String>,
    ),
    PreparedDefault(u64, Selection, Result<PreparedDefaultSave, String>),
    SavedDefault(u64, Selection, Result<(), String>),
    Saved(Command, Result<(), Error>),
    ValueSaved(Selection, Option<String>, Result<NamedValue, Error>),
    BackgroundJobCancelled(Selection, Result<JobSnapshot, Error>),
    Sent(String, Result<Acknowledgement, Error>),
    Checked(String, Result<Option<Acknowledgement>, Error>),
    Discarded(String, Result<(), Error>),
    Pending(Result<Vec<Command>, Error>),
    DefaultModel(u64, Result<ModelSelection, Error>),
    StartupSkills(u64, Result<Vec<String>, Error>),
    Output(Selection, u64, Result<OutputChunk, Error>),
    RestartPrepared(u64, Result<kuku_client::local::ServiceIdentity, Error>),
    Restarted(Result<Restarted, Error>),
    Web(Result<(), Error>),
    ReplySource(Selection, u64, Result<String, Error>),
}

pub(crate) enum Restarted {
    Ready,
    Busy(String),
}

enum Message {
    Connection(Box<Result<Connection, Error>>),
    View(Box<Update>),
}

pub(crate) struct Jobs {
    options: ConnectionOptions,
    build: BuildPolicy<'static>,
    connection: Option<Connection>,
    tasks: JoinSet<()>,
    connector: Option<AbortHandle>,
    observer: Option<AbortHandle>,
    finder: Option<AbortHandle>,
    sender: mpsc::Sender<Message>,
    receiver: mpsc::Receiver<Message>,
}

impl Jobs {
    pub(crate) fn new(options: ConnectionOptions, build: &'static str) -> Self {
        let (sender, receiver) = mpsc::channel(UPDATE_CAPACITY);
        Self {
            options,
            build: BuildPolicy::Start(build),
            connection: None,
            tasks: JoinSet::new(),
            connector: None,
            observer: None,
            finder: None,
            sender,
            receiver,
        }
    }

    pub(crate) async fn next(&mut self) -> Option<Update> {
        let message = self.receiver.recv().await?;
        Some(self.apply(message))
    }

    pub(crate) fn try_next(&mut self) -> Option<Update> {
        let message = self.receiver.try_recv().ok()?;
        Some(self.apply(message))
    }

    pub(crate) fn start(&mut self, job: Job) {
        while self.tasks.try_join_next().is_some() {}
        match job {
            Job::OpenModelEditor(ticket, intent) => {
                let options = self.options.clone();
                let connection = self.connection.clone();
                self.spawn(async move {
                    Update::ModelEditorOpened(
                        ticket,
                        crate::setup::ModelEditor::open(&options, connection.as_ref(), intent)
                            .await
                            .map(Box::new),
                    )
                });
            }
            Job::SaveModelEditor(ticket, editor) => {
                let options = self.options.clone();
                let connection = self.connection.clone();
                self.spawn(async move {
                    let result = editor.save(&options, connection.as_ref()).await;
                    Update::ModelEditorSaved(ticket, editor, result)
                });
            }
            Job::SaveModelSetting(change) => {
                let options = self.options.clone();
                let connection = self.connection.clone();
                self.spawn(async move {
                    Update::ModelSettingSaved(
                        crate::setup::save_setting(&options, connection.as_ref(), change).await,
                    )
                });
            }
            Job::SaveAndUseModel(selection, tier) => {
                let options = self.options.clone();
                let connection = self.connection.clone();
                self.spawn(async move {
                    let change = crate::setup::SavedModelSetting::Default {
                        name: tier.name.clone(),
                        route: ResolvedModel {
                            connection: tier.connection.clone(),
                            model: tier.model.clone(),
                        },
                    };
                    let result =
                        crate::setup::save_setting(&options, connection.as_ref(), change).await;
                    Update::ModelDefaultAndSwitchSaved(selection, tier, result)
                });
            }
            Job::Connect => self.connect(),
            Job::Observe(selection) => {
                Self::abort(&mut self.observer);
                if let Some(connection) = &self.connection {
                    self.observer = Some(self.tasks.spawn(observe::watch(
                        connection.clone(),
                        self.build,
                        selection,
                        self.sender.clone(),
                    )));
                }
            }
            Job::StopObserving => Self::abort(&mut self.observer),
            Job::Find(ticket, query) => {
                Self::abort(&mut self.finder);
                let options = self.options.clone();
                let connection = self.connection.clone();
                self.finder = Some(self.spawn(async move {
                    Update::Found(ticket, find(&options, connection.as_ref(), query).await)
                }));
            }
            Job::SendRouteLabels(ticket, names) => {
                Self::abort(&mut self.finder);
                let connection = self.connection.clone();
                self.finder = Some(self.spawn(async move {
                    let labels = search::send_route_labels(connection.as_ref(), &names).await;
                    Update::SendRouteLabels(ticket, labels)
                }));
            }
            Job::ThinkingDefaults(ticket, config, route) => {
                Self::abort(&mut self.finder);
                let options = self.options.clone();
                let connection = self.connection.clone();
                self.finder = Some(self.spawn(async move {
                    Update::ThinkingDefaults(
                        ticket,
                        crate::setup::thinking_defaults(
                            &options,
                            connection.as_ref(),
                            config,
                            route,
                        )
                        .await,
                    )
                }));
            }
            Job::CustomSpeed(ticket, target, work) => {
                let options = self.options.clone();
                let connection = self.connection.clone();
                let probing = matches!(work, crate::setup::service_tier::SpeedWork::Probe(_));
                if probing {
                    Self::abort(&mut self.finder);
                }
                let task = self.spawn(async move {
                    let result = crate::setup::service_tier::custom_service_tier(
                        &options,
                        connection.as_ref(),
                        &target,
                        work,
                    )
                    .await;
                    Update::SpeedFinished(ticket, target, result)
                });
                if probing {
                    self.finder = Some(task);
                }
            }
            Job::ClosePicker => Self::abort(&mut self.finder),
            history @ (Job::RewindHistory(..) | Job::RewindSource(..) | Job::HistoryJump(..)) => {
                Self::abort(&mut self.finder);
                if let Some(connection) = self.connection.clone() {
                    self.finder = Some(self.spawn(run_connected(history, connection)));
                }
            }
            Job::PrepareDefault {
                ticket,
                selection,
                route,
            } => {
                let options = self.options.clone();
                let connection = self.connection.clone();
                self.spawn(async move {
                    Update::PreparedDefault(
                        ticket,
                        selection,
                        model_default::prepare(&options, connection.as_ref(), route).await,
                    )
                });
            }
            Job::SaveDefault {
                ticket,
                selection,
                plan,
            } => {
                let options = self.options.clone();
                let connection = self.connection.clone();
                self.spawn(async move {
                    Update::SavedDefault(
                        ticket,
                        selection,
                        model_default::save(&options, connection.as_ref(), plan).await,
                    )
                });
            }
            Job::PrepareCredentialRepair {
                ticket,
                selection,
                request,
            } => {
                let options = self.options.clone();
                let connection = self.connection.clone();
                self.spawn(async move {
                    Update::PreparedCredentialRepair(
                        ticket,
                        selection,
                        crate::setup::prepare_credential_repair(
                            &options,
                            connection.as_ref(),
                            request,
                        )
                        .await,
                    )
                });
            }
            Job::SaveCredentialRepair {
                ticket,
                selection,
                plan,
                key,
            } => {
                let options = self.options.clone();
                let connection = self.connection.clone();
                self.spawn(async move {
                    Update::SavedCredentialRepair(
                        ticket,
                        selection,
                        crate::setup::save_credential_repair(
                            &options,
                            connection.as_ref(),
                            plan,
                            key,
                        )
                        .await,
                    )
                });
            }
            Job::RestartCredentialRepair {
                ticket,
                selection,
                saved,
            } => {
                let options = self.options.clone();
                self.spawn(async move {
                    Update::SavedCredentialRepair(
                        ticket,
                        selection,
                        Ok(crate::setup::apply_credential_repair(&options, saved).await),
                    )
                });
            }
            Job::PrepareRestart(ticket) => {
                let options = self.options.clone();
                let connection = self.connection.clone();
                self.spawn(async move {
                    Update::RestartPrepared(
                        ticket,
                        prepare_restart(&options, connection.as_ref()).await,
                    )
                });
            }
            Job::Restart(identity) => {
                let options = self.options.clone();
                self.spawn(async move { Update::Restarted(restart(&options, &identity).await) });
            }
            Job::Copy(_) => unreachable!("clipboard belongs to the terminal"),
            connected => {
                if let Some(connection) = self.connection.clone() {
                    self.spawn(run_connected(connected, connection));
                }
            }
        }
    }

    fn abort(handle: &mut Option<AbortHandle>) {
        if let Some(handle) = handle.take() {
            handle.abort();
        }
    }

    fn apply(&mut self, message: Message) -> Update {
        match message {
            Message::Connection(result) => match *result {
                Ok(connection) => {
                    self.build = self.build.for_reconnect();
                    let endpoint = connection.client.endpoint().to_owned();
                    let workspace = connection.workspace.clone();
                    self.connection = Some(connection);
                    Update::Connected {
                        endpoint,
                        workspace,
                    }
                }
                Err(error) => {
                    self.connection = None;
                    Update::ConnectFailed(error)
                }
            },
            Message::View(update) => *update,
        }
    }

    fn spawn(&mut self, execute: impl Future<Output = Update> + Send + 'static) -> AbortHandle {
        let sender = self.sender.clone();
        self.tasks.spawn(async move {
            push(&sender, execute.await).await;
        })
    }
}

async fn run_connected(job: Job, connection: Connection) -> Update {
    let client = &connection.client;
    match job {
        Job::WorkspacePermission {
            ticket,
            workspace,
            mode,
        } => {
            let result = match mode {
                Some(mode) => client
                    .set_workspace_permission(&workspace, mode)
                    .await
                    .map(Some),
                None => client.workspace_permission(&workspace).await,
            };
            Update::WorkspacePermission {
                ticket,
                saving: mode.is_some(),
                result,
            }
        }
        Job::PendingApprovals(selection) => {
            let result = client.pending_approvals(&selection.target).await;
            Update::PendingApprovals(selection, result)
        }
        Job::Refresh(selection) => {
            let result = client.snapshot(&selection.target).await.map(Box::new);
            Update::Refreshed(selection, result)
        }
        Job::RefreshProgress(selection) => {
            let result = client
                .observation_snapshot(&selection.target)
                .await
                .map(Box::new);
            Update::Refreshed(selection, result)
        }
        Job::History(selection, request) => {
            let result = history::load(client, &selection.target, request).await;
            Update::History(selection, request, result)
        }
        Job::RewindHistory(selection, ticket, before) => {
            let result = client
                .user_history(&selection.target, Some(before), crate::app::REWIND_PAGE)
                .await;
            Update::RewindHistory(selection, ticket, before, result)
        }
        Job::RewindSource(selection, ticket, event_id) => {
            let result = history_text(client, &selection.target, event_id, HistoryKind::User).await;
            Update::RewindSource(selection, ticket, event_id, result)
        }
        Job::HistoryJump(selection, ticket, before) => {
            let result = client
                .history_destinations(&selection.target, before, 24)
                .await;
            Update::HistoryJump(selection, ticket, before, result)
        }
        Job::HistoryJumpDraft(selection, ticket, event_id) => {
            let result = original_user_message(client, &selection.target, event_id).await;
            Update::HistoryJumpDraft(selection, ticket, result)
        }
        Job::FileRewindPreview(selection, ticket, event_id) => {
            let result = client
                .file_rewind_preview(&selection.target, event_id)
                .await;
            Update::FileRewindPreview(selection, ticket, result)
        }
        Job::RewindGroupHistory(selection, request) => {
            let result = client
                .rewind_group_history(
                    &selection.target,
                    request.rewind_event_id,
                    request.before,
                    REWIND_GROUP_PAGE,
                )
                .await;
            Update::RewindGroupHistory(selection, request, result)
        }
        Job::Save(command) => {
            let save = command.clone();
            let result = tokio::task::spawn_blocking(move || connection.pending.save(&save)).await;
            Update::Saved(command, disk_result(result))
        }
        Job::SaveValue(selection, save) => {
            let result = client
                .save_named_value(
                    &selection.target,
                    &save.name,
                    &save.source,
                    save.replace,
                    save.create.as_ref(),
                )
                .await;
            let text = if let NamedValueSource::Text(text) = save.source {
                Some(text)
            } else {
                None
            };
            Update::ValueSaved(selection, text, result)
        }
        Job::CancelBackground(selection, id) => {
            let result = client.cancel_job(&selection.target, &id).await;
            Update::BackgroundJobCancelled(selection, result)
        }
        Job::Send(command) => {
            let result = connection.pending.submit_saved(client, &command).await;
            Update::Sent(command.command_id, result)
        }
        Job::Check(command) => {
            let result = connection.pending.reconcile(client, &command).await;
            Update::Checked(command.command_id, result)
        }
        Job::Discard(command) => {
            let id = command.command_id.clone();
            let result =
                tokio::task::spawn_blocking(move || connection.pending.remove(&command)).await;
            Update::Discarded(id, disk_result(result))
        }
        Job::Pending => {
            let workspace = connection.workspace;
            let result = tokio::task::spawn_blocking(move || connection.pending.list()).await;
            Update::Pending(disk_result(result).map(|commands| {
                commands
                    .into_iter()
                    .filter(|command| command.target.workspace == workspace)
                    .collect()
            }))
        }
        Job::Model(config, version) => {
            Update::DefaultModel(version, client.resolve_model(&config).await)
        }
        Job::AgentModels(ticket) => Update::AgentModels(ticket, client.models().await),
        Job::StartupSkills(generation) => {
            let result = client
                .skills(&connection.workspace, None)
                .await
                .map(|skills| skills.into_iter().map(|skill| skill.name).collect());
            Update::StartupSkills(generation, result)
        }
        Job::PrepareDefault { .. } | Job::SaveDefault { .. } => {
            unreachable!("default-save jobs are handled before connected jobs")
        }
        Job::Output(selection, id, output_id, offset, limit) => {
            let result = client
                .output(&selection.target, &output_id, offset, limit)
                .await;
            Update::Output(selection, id, result)
        }
        Job::Web(target) => Update::Web(web(&connection, target.as_ref()).await),
        Job::ReplySource(selection, id) => {
            let result = history_text(client, &selection.target, id, HistoryKind::Assistant).await;
            Update::ReplySource(selection, id, result)
        }
        Job::OpenModelEditor(..)
        | Job::SaveModelEditor(..)
        | Job::SaveModelSetting(..)
        | Job::SaveAndUseModel(..)
        | Job::PrepareCredentialRepair { .. }
        | Job::SaveCredentialRepair { .. }
        | Job::RestartCredentialRepair { .. }
        | Job::Connect
        | Job::Observe(_)
        | Job::StopObserving
        | Job::Find(_, _)
        | Job::SendRouteLabels(..)
        | Job::ThinkingDefaults(..)
        | Job::CustomSpeed(..)
        | Job::ClosePicker
        | Job::PrepareRestart(_)
        | Job::Restart(_)
        | Job::Copy(_) => unreachable!("job requires service options or terminal"),
    }
}

async fn original_user_message(client: &Client, target: &Target, id: u64) -> Result<String, Error> {
    use kuku::event::EventPayload;

    let page = client
        .events_before(
            &target.workspace,
            &target.session_id,
            Some(id.saturating_add(1)),
            1,
        )
        .await?;
    page.events
        .into_iter()
        .find_map(|event| match event.payload {
            EventPayload::MessageUser {
                conversation,
                text,
                job_delivery: false,
                ..
            } if event.id == id && conversation == target.agent => Some(text),
            _ => None,
        })
        .ok_or_else(|| Error::Protocol("the selected original message is missing".into()))
}

async fn history_text(
    client: &Client,
    target: &Target,
    id: u64,
    kind: HistoryKind,
) -> Result<String, Error> {
    let page = client
        .history(target, Some(id.saturating_add(1)), 1)
        .await?;
    history_entry_text(page, target, id, kind)
}

fn history_entry_text(
    page: HistoryPage,
    target: &Target,
    id: u64,
    kind: HistoryKind,
) -> Result<String, Error> {
    page.entries
        .into_iter()
        .find(|entry| entry.id == id && entry.kind == kind && entry.agent == target.agent)
        .and_then(|entry| entry.text)
        .ok_or_else(|| Error::Protocol("the selected message is missing from history".into()))
}

async fn push(sender: &mpsc::Sender<Message>, update: Update) -> bool {
    sender.send(Message::View(Box::new(update))).await.is_ok()
}

fn value_choice(value: NamedValue) -> Choice {
    let kind = match value.kind {
        NamedValueKind::Text => "Text",
        NamedValueKind::Json => "JSON",
        NamedValueKind::Bytes => "Bytes",
        NamedValueKind::Output => "Saved output",
    };
    Choice::new(
        value.name.clone(),
        format!("{kind} · {} bytes", value.reference.bytes),
        ChoiceKind::Value(value),
    )
}

pub(crate) fn job_status_label(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Running => "Running",
        JobStatus::Completed => "Completed",
        JobStatus::Failed => "Failed",
        JobStatus::Cancelled => "Cancelled",
        JobStatus::Interrupted => "Interrupted",
    }
}

pub(crate) fn job_detail(job: &JobSnapshot) -> String {
    let title = if job.title.trim().is_empty() {
        &job.tool
    } else {
        &job.title
    };
    let mut detail = format!("{} · {}", job_status_label(job.status), single_line(title));
    if job.status == JobStatus::Running || job.tool == "run_js" {
        if let Some(elapsed_ms) = job.elapsed_ms {
            detail.push_str(" · ");
            detail.push_str(&format_elapsed(Duration::from_millis(elapsed_ms)));
        }
    }
    if job.status != JobStatus::Running && !job.summary.is_empty() && job.summary != *title {
        detail.push_str(" · ");
        detail.push_str(&single_line(&job.summary));
    }
    detail
}

async fn prepare_restart(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
) -> Result<kuku_client::local::ServiceIdentity, Error> {
    let service = options.local_service()?;
    let current = service.status().await?.ok_or_else(|| {
        Error::Options("No managed local kuku service is running. Use Ctrl+R to connect.".into())
    })?;
    if connection.is_some_and(|connection| {
        !crate::local_settings::same_service_endpoint(
            connection.client.endpoint(),
            current.endpoint(),
        )
    }) {
        return Err(Error::Options(
            "The kuku service changed. Use Ctrl+R to reconnect.".into(),
        ));
    }
    Ok(current.identity())
}

async fn restart(
    options: &ConnectionOptions,
    identity: &kuku_client::local::ServiceIdentity,
) -> Result<Restarted, Error> {
    match options
        .local_service()?
        .restart_if_current(identity)
        .await?
    {
        RestartOutcome::Restarted(_) => Ok(Restarted::Ready),
        RestartOutcome::Deferred(status) => {
            Ok(Restarted::Busy(busy(&status.busy, status.draining)))
        }
    }
}

fn busy(items: &[String], draining: bool) -> String {
    if !items.is_empty() {
        format!("Busy · {}", items.join(", "))
    } else if draining {
        "Draining work".into()
    } else {
        "Ready".into()
    }
}

async fn web(connection: &Connection, target: Option<&Target>) -> Result<(), Error> {
    let session = target.map(|target| target.session_id.as_str());
    let agent = target.map(|target| target.agent.as_str());
    let workspace = &connection.workspace;
    let url = if connection.local.is_some() {
        connection
            .client
            .browser_url(workspace, session, agent)
            .await?
    } else {
        connection.client.web_url(workspace, session, agent)?
    };
    crate::open_browser(url.as_str()).map_err(Error::from)
}

fn disk_result<T>(result: Result<Result<T, Error>, tokio::task::JoinError>) -> Result<T, Error> {
    result.map_err(|error| std::io::Error::other(format!("client storage task: {error}")))?
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use kuku_client::{HistoryEntry, HistoryKind, HistoryPage, Target};

    use super::history_entry_text;

    #[test]
    fn message_source_requires_the_exact_event_agent_and_role_and_keeps_the_full_text() {
        let target = Target {
            workspace: PathBuf::from("workspace"),
            session_id: "session".into(),
            agent: "main".into(),
        };
        let text = "x".repeat(crate::transcript::MAX_BODY_BYTES + 17);
        let entry = HistoryEntry {
            id: 17,
            agent: "main".into(),
            turn: Some(1),
            kind: HistoryKind::User,
            text: Some(text.clone()),
            turn_summary: None,
            details: Default::default(),
        };
        let page = |entry| HistoryPage {
            entries: vec![entry],
            rewind_groups: Vec::new(),
            next_before: None,
        };
        assert_eq!(
            text,
            history_entry_text(page(entry.clone()), &target, 17, HistoryKind::User).unwrap()
        );
        for invalid in [
            HistoryEntry {
                id: 16,
                ..entry.clone()
            },
            HistoryEntry {
                agent: "reviewer".into(),
                ..entry.clone()
            },
            HistoryEntry {
                kind: HistoryKind::Assistant,
                ..entry.clone()
            },
            HistoryEntry {
                text: None,
                ..entry.clone()
            },
        ] {
            assert!(history_entry_text(page(invalid), &target, 17, HistoryKind::User).is_err());
        }
        let assistant = HistoryEntry {
            kind: HistoryKind::Assistant,
            ..entry
        };
        assert_eq!(
            text,
            history_entry_text(page(assistant), &target, 17, HistoryKind::Assistant).unwrap()
        );
    }
}
