use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::FutureExt;
use kuku::adapter::{SubscriptionLogin, SubscriptionLoginResult, SubscriptionStatus};
use kuku::config::{ProviderProxy, SubscriptionChannel};
use ratatui::{layout::Rect, Frame};
use tokio::task::JoinHandle;

use crate::editor::Editor;
use crate::terminal::AuthorizationLink;
use crate::theme::{ACCENT, MUTED};

use super::super::config::Settings;
use super::super::state::{edit_field, SetupState, SetupStep};
use super::super::ui::{field, line};
use super::super::{ConnectionChoice, Finish};

const MAX_CALLBACK_BYTES: usize = 64 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(35);
const LINK_ROW: u16 = 2;

pub(in crate::setup) struct Enrollment {
    channel: SubscriptionChannel,
    proxy: Editor,
    name: Editor,
    editing_name: bool,
    callback: Editor,
    stage: Stage,
    next_poll: Option<Instant>,
}

#[derive(Default)]
enum Stage {
    #[default]
    Settings,
    Preparing(ProviderProxy, JoinHandle<kuku::Result<SubscriptionLogin>>),
    Authorizing(ProviderProxy, Box<SubscriptionLogin>),
    Verifying(
        ProviderProxy,
        JoinHandle<kuku::Result<SubscriptionLoginResult>>,
    ),
    Registered(ProviderProxy, SubscriptionLoginResult),
}

impl Drop for Enrollment {
    fn drop(&mut self) {
        match &self.stage {
            Stage::Preparing(_, task) => task.abort(),
            Stage::Verifying(_, task) => task.abort(),
            _ => {}
        }
    }
}

impl Enrollment {
    fn new(channel: SubscriptionChannel) -> Self {
        Self {
            channel,
            proxy: Editor::default(),
            name: Editor::default(),
            editing_name: false,
            callback: Editor::default(),
            stage: Stage::Settings,
            next_poll: None,
        }
    }

    fn begin(&mut self, settings: &Settings) -> Result<(), String> {
        let data_dir = settings
            .private_data_dir()
            .map_err(|err| err.to_string())?
            .to_owned();
        let channel = self.channel;
        let name = self.name.text().trim();
        let url = self.proxy.text().trim();
        let saved_proxy = settings
            .subscription_connection(channel, name)
            .map(|(_, _, connection)| &connection.options.proxy);
        let proxy = match saved_proxy {
            Some(saved) if url.is_empty() => saved.clone(),
            Some(saved @ ProviderProxy::Explicit { url: saved_url, .. }) if url == saved_url => {
                saved.clone()
            }
            _ => ProviderProxy::Explicit {
                url: url.to_owned(),
                auth: None,
            },
        };
        let name = (!name.is_empty()).then(|| name.to_owned());
        let login_proxy = proxy.clone();
        let task = tokio::spawn(async move {
            SubscriptionLogin::start(&data_dir, channel, name.as_deref(), login_proxy).await
        });
        self.stage = Stage::Preparing(proxy, task);
        self.next_poll = Some(Instant::now());
        Ok(())
    }

    fn submit(&mut self) {
        let Stage::Authorizing(proxy, attempt) = std::mem::take(&mut self.stage) else {
            unreachable!("submitting an authorization callback");
        };
        let callback = self.callback.take_text();
        let task = tokio::spawn(async move { attempt.complete(&callback).await });
        self.stage = Stage::Verifying(proxy, task);
        self.next_poll = Some(Instant::now());
    }

    fn poll(&mut self) -> Option<Result<(), String>> {
        let now = Instant::now();
        if self.next_poll.is_none_or(|at| at > now) {
            return None;
        }
        let stage = match &mut self.stage {
            Stage::Preparing(proxy, task) => {
                ready(task)?.map(|attempt| Stage::Authorizing(proxy.clone(), Box::new(attempt)))
            }
            Stage::Verifying(proxy, task) => {
                ready(task)?.map(|registration| Stage::Registered(proxy.clone(), registration))
            }
            Stage::Registered(..) => return Some(Ok(())),
            _ => return None,
        };
        self.next_poll = None;
        match stage {
            Ok(stage) => {
                self.stage = stage;
                Some(Ok(()))
            }
            Err(error) => {
                self.stage = Stage::Settings;
                Some(Err(error))
            }
        }
    }

    fn edit_callback(&mut self, event: Event) -> Result<(), String> {
        match event {
            Event::Paste(text) if text.len() > MAX_CALLBACK_BYTES => {
                return Err("Callback URL is too long.".into());
            }
            Event::Paste(text) => self.callback.set_text(text),
            Event::Key(key) if key.code == KeyCode::Enter => {
                if self.callback.text().trim().is_empty() {
                    return Err("Paste the full callback URL first.".into());
                }
                self.submit();
            }
            Event::Key(key)
                if key.code == KeyCode::Char('a')
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.callback.set_text(String::new());
            }
            Event::Key(key) if self.callback.text().len() < MAX_CALLBACK_BYTES - 4 => {
                edit_field(&mut self.callback, key);
            }
            _ => {}
        }
        Ok(())
    }

    pub(in crate::setup) fn link(&self, area: Rect) -> Option<AuthorizationLink> {
        let Stage::Authorizing(_, attempt) = &self.stage else {
            return None;
        };
        Some(AuthorizationLink::new(
            attempt.authorization_url(),
            Rect::new(
                area.x,
                area.y + LINK_ROW,
                area.width,
                area.height.saturating_sub(LINK_ROW + 3),
            ),
        ))
    }

    pub(in crate::setup) fn verifying(&self) -> bool {
        matches!(self.stage, Stage::Verifying(..))
    }

    pub(in crate::setup) fn render(&mut self, frame: &mut Frame<'_>, area: Rect, row: &mut u16) {
        line(frame, area, row, "Sign in with ChatGPT", ACCENT);
        match self.stage {
            Stage::Settings => {
                line(frame, area, row, "Proxy · HTTP or SOCKS5H", MUTED);
                if self.editing_name {
                    line(frame, area, row, &format!("> {}", self.proxy.text()), MUTED);
                } else {
                    field(frame, area, row, &mut self.proxy);
                }
                line(frame, area, row, "Account name · optional", MUTED);
                if self.editing_name {
                    field(frame, area, row, &mut self.name);
                } else {
                    line(frame, area, row, &format!("> {}", self.name.text()), MUTED);
                }
                line(
                    frame,
                    area,
                    row,
                    "Tab switches field · Enter continues",
                    MUTED,
                );
            }
            Stage::Preparing(..) => line(frame, area, row, "Preparing authorization…", MUTED),
            Stage::Authorizing(..) => {
                line(
                    frame,
                    area,
                    row,
                    "Open the full authorization link in your browser.",
                    MUTED,
                );
                if let Some(link) = self.link(area) {
                    for (_, text) in link.rows() {
                        line(frame, area, row, text, ACCENT);
                    }
                }
                if self.callback.text().is_empty() {
                    line(
                        frame,
                        area,
                        row,
                        "Paste the URL from your browser's address bar.",
                        MUTED,
                    );
                    line(
                        frame,
                        area,
                        row,
                        "If the page fails to load, still copy its address.",
                        MUTED,
                    );
                } else {
                    line(
                        frame,
                        area,
                        row,
                        "Callback received · Enter to verify",
                        MUTED,
                    );
                }
            }
            Stage::Verifying(..) => line(frame, area, row, "Verifying authorization…", MUTED),
            Stage::Registered(..) => line(
                frame,
                area,
                row,
                "Authorized · Enter to save the connection",
                MUTED,
            ),
        }
    }
}

fn ready<T>(task: &mut JoinHandle<kuku::Result<T>>) -> Option<Result<T, String>> {
    task.now_or_never().map(|result| match result {
        Ok(result) => result.map_err(|err| err.to_string()),
        Err(_) => Err("Authorization did not complete. Please try again.".into()),
    })
}

impl SetupState {
    pub(in crate::setup) fn open_subscription(&mut self, channel: SubscriptionChannel) {
        self.enrollment = Some(Enrollment::new(channel));
        self.step = SetupStep::SubscriptionSignIn;
    }

    pub(in crate::setup) fn subscription_input(&mut self, event: Event, settings: &Settings) {
        let Some(flow) = &mut self.enrollment else {
            return;
        };
        if let Event::Key(key) = &event {
            if key.kind == KeyEventKind::Release || flow.verifying() {
                return;
            }
            if key.code == KeyCode::Esc {
                self.enrollment = None;
                self.previous();
                return;
            }
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                self.finish = Some(Finish::Cancel);
                return;
            }
        }
        self.problem = match (&flow.stage, event) {
            (Stage::Settings, Event::Key(key)) if key.code == KeyCode::Enter => {
                flow.begin(settings).err()
            }
            (Stage::Settings, Event::Key(key))
                if matches!(
                    key.code,
                    KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down
                ) =>
            {
                flow.editing_name = !flow.editing_name;
                None
            }
            (Stage::Settings, event) => {
                let editor = if flow.editing_name {
                    &mut flow.name
                } else {
                    &mut flow.proxy
                };
                match event {
                    Event::Key(key) => edit_field(editor, key),
                    Event::Paste(text) => editor.insert(&text),
                    _ => {}
                }
                None
            }
            (Stage::Authorizing(..), event) => flow.edit_callback(event).err(),
            (Stage::Registered(..), Event::Key(key)) if key.code == KeyCode::Enter => {
                flow.next_poll = Some(Instant::now());
                None
            }
            _ => self.problem.take(),
        };
    }

    pub(in crate::setup) fn subscription_deadline(&self) -> Option<Instant> {
        self.enrollment.as_ref()?.next_poll
    }

    pub(in crate::setup) fn poll_subscription(&mut self, settings: &mut Settings) -> bool {
        let Some(flow) = &mut self.enrollment else {
            return false;
        };
        if flow.next_poll.is_none_or(|at| at > Instant::now()) {
            return false;
        }
        match flow.poll() {
            None => {
                flow.next_poll = Some(Instant::now() + POLL_INTERVAL);
                return false;
            }
            Some(Err(error)) => {
                self.problem = Some(error);
                return true;
            }
            Some(Ok(())) => {}
        }
        let Stage::Registered(proxy, registration) = &flow.stage else {
            return true;
        };
        self.subscription_saved = true;
        flow.next_poll = None;
        let published = settings
            .reload()
            .map_err(kuku::Error::from)
            .and_then(|latest| {
                let name = latest.save_subscription_connection(&registration.account, proxy)?;
                Ok((name, latest.reload()?))
            });
        let (name, updated) = match published {
            Ok(saved) => saved,
            Err(error) => {
                self.problem = Some(error.to_string());
                return true;
            }
        };
        let status = registration.info.status;
        self.enrollment = None;
        *settings = updated;
        self.choices = settings.connection_choices();
        let choice = ConnectionChoice::Existing(name);
        self.selected = self
            .choices
            .iter()
            .position(|item| *item == choice)
            .expect("saved account is selectable");
        self.load_selection(settings);
        if status == SubscriptionStatus::SignedIn {
            self.start_discovery(settings);
        } else {
            self.step = SetupStep::Connection;
            self.problem =
                Some("ChatGPT sign-in succeeded. Authorize plan usage to find models.".into());
        }
        true
    }
}
