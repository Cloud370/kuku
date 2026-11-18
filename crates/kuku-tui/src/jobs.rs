use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use kuku_client::local::{RestartOutcome, SESSION_PROTOCOL_VERSION};
use kuku_client::{
    Acknowledgement, AgentConfig, Client, Command, Connection, ConnectionOptions, Error,
    HistoryKind, HistoryPage, ModelCatalog, ModelSelection, Observation, OutputChunk,
    SessionStatus, Snapshot, SubscriptionUpdate, Target,
};
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinSet};

use crate::menu::{Choice, ChoiceKind, Ticket};
use crate::output::OUTPUT_PAGE_BYTES;
use crate::transcript::{HistoryRequest, HISTORY_PAGE};

const UPDATE_CAPACITY: usize = 64;
const SEARCH_DELAY: Duration = Duration::from_millis(70);
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

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
    Models,
    Server,
}

pub(crate) enum Job {
    Connect,
    Observe(Selection),
    StopObserving,
    Refresh(Selection),
    RefreshProgress(Selection),
    History(Selection, HistoryRequest),
    Find(Ticket, Search),
    ClosePicker,
    Save(Command),
    Send(Command),
    Check(Command),
    Discard(Command),
    Pending,
    Model(AgentConfig, u64),
    Output(Selection, u64, String, u64),
    Restart,
    Web(Option<Target>),
    ReplySource(Selection, u64),
    Copy(String),
}

pub(crate) enum Update {
    Connected {
        endpoint: String,
        workspace: PathBuf,
    },
    ConnectFailed(Error),
    Loaded(Selection, Box<Snapshot>, HistoryPage),
    WatchConnected(Selection),
    WatchReconnecting(Selection, String),
    WatchFailed(Selection, Error),
    Observation(Selection, Observation),
    Refreshed(Selection, Result<Box<Snapshot>, Error>),
    History(Selection, HistoryRequest, Result<HistoryPage, Error>),
    Found(Ticket, Result<(Vec<Choice>, String), Error>),
    Saved(Command, Result<(), Error>),
    Sent(String, Result<Acknowledgement, Error>),
    Checked(String, Result<Option<Acknowledgement>, Error>),
    Discarded(String, Result<(), Error>),
    Pending(Result<Vec<Command>, Error>),
    DefaultModel(u64, Result<ModelSelection, Error>),
    Output(Selection, u64, Result<OutputChunk, Error>),
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
    connection: Option<Connection>,
    tasks: JoinSet<()>,
    observer: Option<AbortHandle>,
    finder: Option<AbortHandle>,
    sender: mpsc::Sender<Message>,
    receiver: mpsc::Receiver<Message>,
}

impl Jobs {
    pub(crate) fn new(options: ConnectionOptions) -> Self {
        let (sender, receiver) = mpsc::channel(UPDATE_CAPACITY);
        Self {
            options,
            connection: None,
            tasks: JoinSet::new(),
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
            Job::Connect => {
                Self::abort(&mut self.observer);
                let options = self.options.clone();
                let sender = self.sender.clone();
                self.tasks.spawn(async move {
                    let result = options.connect().await;
                    let _ = sender.send(Message::Connection(Box::new(result))).await;
                });
            }
            Job::Observe(selection) => {
                Self::abort(&mut self.observer);
                if let Some(connection) = &self.connection {
                    let client = connection.client.clone();
                    let sender = self.sender.clone();
                    self.observer = Some(self.tasks.spawn(watch(client, selection, sender)));
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
            Job::ClosePicker => Self::abort(&mut self.finder),
            Job::Restart => {
                let options = self.options.clone();
                self.spawn(async move { Update::Restarted(restart(&options).await) });
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
            let result = client
                .history(&selection.target, Some(request.before()), HISTORY_PAGE)
                .await;
            Update::History(selection, request, result)
        }
        Job::Save(command) => {
            let save = command.clone();
            let result = tokio::task::spawn_blocking(move || connection.pending.save(&save)).await;
            Update::Saved(command, disk_result(result))
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
        Job::Output(selection, id, output_id, offset) => {
            let result = client
                .output(&selection.target, &output_id, offset, OUTPUT_PAGE_BYTES)
                .await;
            Update::Output(selection, id, result)
        }
        Job::Web(target) => Update::Web(web(&connection, target.as_ref()).await),
        Job::ReplySource(selection, id) => {
            let result = history_reply(client, &selection.target, id).await;
            Update::ReplySource(selection, id, result)
        }
        Job::Connect
        | Job::Observe(_)
        | Job::StopObserving
        | Job::Find(_, _)
        | Job::ClosePicker
        | Job::Restart
        | Job::Copy(_) => unreachable!("job requires service options or terminal"),
    }
}

async fn history_reply(client: &Client, target: &Target, id: u64) -> Result<String, Error> {
    client
        .history(target, Some(id.saturating_add(1)), 1)
        .await?
        .entries
        .into_iter()
        .find(|entry| {
            entry.id == id && entry.kind == HistoryKind::Assistant && entry.agent == target.agent
        })
        .and_then(|entry| entry.text)
        .ok_or_else(|| Error::Protocol("the assistant reply is missing from history".into()))
}

async fn watch(client: Client, selection: Selection, sender: mpsc::Sender<Message>) {
    loop {
        let error = observe(&client, &selection, &sender).await;
        if !matches!(
            error,
            Error::Transport(_)
                | Error::ObservationEnded
                | Error::HttpStatus(502..=504)
                | Error::Service {
                    status: 502..=504,
                    ..
                }
        ) {
            push(&sender, Update::WatchFailed(selection, error)).await;
            return;
        }
        if !push(
            &sender,
            Update::WatchReconnecting(selection.clone(), error.to_string()),
        )
        .await
        {
            return;
        }
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

async fn observe(client: &Client, selection: &Selection, sender: &mpsc::Sender<Message>) -> Error {
    let target = &selection.target;
    let snapshot = match client.observation_snapshot(target).await {
        Ok(snapshot) => snapshot,
        Err(error) => return error,
    };
    let page = match client
        .history(
            target,
            Some(snapshot.cursor.saturating_add(1)),
            HISTORY_PAGE,
        )
        .await
    {
        Ok(page) => page,
        Err(error) => return error,
    };
    let mut subscription = client.subscribe_with_agent_activity(target, snapshot.cursor);
    if !push(
        sender,
        Update::Loaded(selection.clone(), Box::new(snapshot), page),
    )
    .await
    {
        return Error::ObservationEnded;
    }
    loop {
        let update = match subscription.next_update().await {
            Ok(SubscriptionUpdate::Connected) => Update::WatchConnected(selection.clone()),
            Ok(SubscriptionUpdate::Reconnecting(error)) => {
                Update::WatchReconnecting(selection.clone(), error)
            }
            Ok(SubscriptionUpdate::Observation(event)) => {
                Update::Observation(selection.clone(), event)
            }
            Err(error) => return error,
        };
        if !push(sender, update).await {
            return Error::ObservationEnded;
        }
    }
}

async fn push(sender: &mpsc::Sender<Message>, update: Update) -> bool {
    sender.send(Message::View(Box::new(update))).await.is_ok()
}

async fn find(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
    query: Search,
) -> Result<(Vec<Choice>, String), Error> {
    if matches!(query, Search::Server) {
        return Ok((Vec::new(), service_status(options, connection).await?));
    }
    let connection = connection.ok_or_else(|| Error::Options("Connect with /reconnect".into()))?;
    let client = &connection.client;
    let workspace = &connection.workspace;
    let choices = match query {
        Search::Skills(query) => {
            tokio::time::sleep(SEARCH_DELAY).await;
            let query = query.trim_start_matches('/').trim_start_matches("skill:");
            client
                .skills(workspace, Some(query))
                .await?
                .into_iter()
                .map(|skill| {
                    Choice::new(
                        format!("/skill:{}", skill.name),
                        format!("Skill · {}", skill.description),
                        ChoiceKind::Skill(skill.name),
                    )
                })
                .collect()
        }
        Search::Paths(query) => {
            tokio::time::sleep(SEARCH_DELAY).await;
            client
                .workspace_files(workspace, &query)
                .await?
                .into_iter()
                .map(|path| Choice::new(&path, "Workspace path", ChoiceKind::Path(path.clone())))
                .collect()
        }
        Search::Sessions(query) => {
            tokio::time::sleep(SEARCH_DELAY).await;
            client
                .sessions(Some(workspace), Some(&query))
                .await?
                .into_iter()
                .map(|session| {
                    let state = if !session.waiting_agents.is_empty() {
                        "Waiting"
                    } else {
                        match session.status {
                            SessionStatus::Active => "Active",
                            SessionStatus::Done => "Done",
                            SessionStatus::Interrupted => "Interrupted",
                        }
                    };
                    let title = if session.title.trim().is_empty() {
                        session.session_id.clone()
                    } else {
                        session.title
                    };
                    Choice::new(
                        &title,
                        format!(
                            "{} · {state} · {} · {} Turns",
                            session.session_id.chars().take(8).collect::<String>(),
                            session.mtime,
                            session.turn_count
                        ),
                        ChoiceKind::Session {
                            id: session.session_id,
                            title: title.clone(),
                            exists: true,
                        },
                    )
                })
                .collect()
        }
        Search::Models => return Ok(model_choices(client.models().await?)),
        Search::Server => unreachable!("handled above"),
    };
    Ok((choices, String::new()))
}

fn model_choices(catalog: ModelCatalog) -> (Vec<Choice>, String) {
    let mut choices = Vec::new();
    for tier in catalog.tiers {
        let detail = tier.error.map_or_else(
            || {
                format!(
                    "{} · {}{}",
                    tier.connection,
                    tier.model,
                    tier.purpose
                        .map_or_else(String::new, |purpose| format!(" · {purpose}"))
                )
            },
            |error| format!("{} · {} · {}", tier.connection, tier.model, error.message),
        );
        choices.push(Choice::new(
            format!("Tier · {}", tier.name),
            detail,
            ChoiceKind::Tier(tier.name),
        ));
    }
    for connection in catalog.connections {
        choices.push(Choice::new(
            format!("Connection · {}", connection.name),
            format!("{:?} · enter an explicit model ID", connection.protocol),
            ChoiceKind::Connection(connection.name),
        ));
    }
    (
        choices,
        format!("Server default Tier: {}", catalog.default_tier),
    )
}

async fn service_status(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
) -> Result<String, Error> {
    let workspace =
        connection.map_or_else(|| options.workspace(), |item| Ok(item.workspace.clone()))?;
    if options.remote.is_some() {
        let client = options
            .remote_connection()?
            .expect("explicit remote client");
        let health = client.health().await?;
        return Ok(format!("Remote {}\nWorkspace {}\nBuild {} · service protocol {} · client protocol {SESSION_PROTOCOL_VERSION}",
            client.endpoint(), workspace.display(), health.version, health.protocol_version));
    }
    let service = options.local_service()?;
    if let Some(record) = service.status().await? {
        let status = record.status();
        return Ok(format!("Local {}\nWorkspace {}\nConfig {}\nBuild {} · service protocol {} · client protocol {SESSION_PROTOCOL_VERSION}\n{}",
            record.endpoint(), workspace.display(), status.config_path.as_deref().unwrap_or("Unavailable"), status.version,
            status.protocol_version, busy(&status.busy, status.draining)));
    }
    Ok(format!(
        "Local service stopped\nWorkspace {}\nData {}",
        workspace.display(),
        service.data_dir().display()
    ))
}

async fn restart(options: &ConnectionOptions) -> Result<Restarted, Error> {
    match options.local_service()?.restart().await? {
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
