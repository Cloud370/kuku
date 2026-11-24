use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::FutureExt;
use kuku::adapter::{
    SubscriptionLogin, SubscriptionLoginProgress, SubscriptionLoginResult, SubscriptionStatus,
};
use kuku::config::{ProviderProxy, SubscriptionChannel};
use ratatui::{
    layout::{Position, Rect},
    Frame,
};
use tokio::task::JoinHandle;

use crate::editor::Editor;
use crate::terminal::{send_to_terminal_clipboard, AuthorizationLink};
use crate::theme::{ACCENT, MUTED, WARNING};

use super::super::config::Settings;
use super::super::state::{edit_field, SetupState, SetupStep};
use super::super::ui::{field, line};
use super::super::{ConnectionChoice, Finish};

const MAX_CALLBACK_BYTES: usize = 64 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(35);
const RAW_URL_ROW: u16 = 3;

pub(in crate::setup) struct Enrollment {
    channel: SubscriptionChannel,
    name: Editor,
    callback: Editor,
    url_first_row: usize,
    url_copy_sent: bool,
    url_retry: bool,
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
        JoinHandle<kuku::Result<SubscriptionLoginProgress>>,
    ),
    Registered(ProviderProxy, SubscriptionLoginResult),
}

impl Drop for Enrollment {
    fn drop(&mut self) {
        self.abort_pending();
    }
}

impl Enrollment {
    fn new(channel: SubscriptionChannel) -> Self {
        Self {
            channel,
            name: Editor::default(),
            callback: Editor::default(),
            url_first_row: 0,
            url_copy_sent: false,
            url_retry: false,
            stage: Stage::Settings,
            next_poll: None,
        }
    }

    fn abort_pending(&self) {
        match &self.stage {
            Stage::Preparing(_, task) => task.abort(),
            Stage::Verifying(_, task) => task.abort(),
            _ => {}
        }
    }

    fn begin(&mut self, settings: &Settings, selected_proxy: &ProviderProxy) -> Result<(), String> {
        let data_dir = settings
            .private_data_dir()
            .map_err(|err| err.to_string())?
            .to_owned();
        let channel = self.channel;
        let name = self.name.text().trim();
        let mut proxy = selected_proxy.clone();
        let saved_proxy = settings
            .subscription_connection(channel, name)
            .map(|(_, _, connection)| &connection.options.proxy);
        if let (
            ProviderProxy::Explicit { url, auth },
            Some(ProviderProxy::Explicit {
                url: saved_url,
                auth: saved_auth,
            }),
        ) = (&mut proxy, saved_proxy)
        {
            if url == saved_url && auth.is_none() {
                *auth = saved_auth.clone();
            }
        }
        let name = (!name.is_empty()).then(|| name.to_owned());
        let login_proxy = proxy.clone();
        let task = tokio::spawn(async move {
            SubscriptionLogin::start(&data_dir, channel, name.as_deref(), login_proxy).await
        });
        self.stage = Stage::Preparing(proxy, task);
        self.url_first_row = 0;
        self.url_copy_sent = false;
        self.url_retry = false;
        self.next_poll = Some(Instant::now());
        Ok(())
    }

    fn submit(&mut self) {
        let Stage::Authorizing(proxy, attempt) = std::mem::take(&mut self.stage) else {
            unreachable!("submitting an authorization callback");
        };
        let callback = self.callback.take_text();
        let task = tokio::spawn(async move { attempt.complete_or_retry(&callback).await });
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
            Stage::Verifying(proxy, task) => ready(task)?.map(|progress| match progress {
                SubscriptionLoginProgress::Completed(login) => {
                    Stage::Registered(proxy.clone(), login)
                }
                SubscriptionLoginProgress::Retry(attempt) => {
                    self.url_first_row = 0;
                    self.url_copy_sent = false;
                    self.url_retry = true;
                    Stage::Authorizing(proxy.clone(), attempt)
                }
            }),
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
                if key.code == KeyCode::Char('y')
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                if let Stage::Authorizing(_, attempt) = &self.stage {
                    send_to_terminal_clipboard(attempt.authorization_url())
                        .map_err(|error| error.to_string())?;
                    self.url_copy_sent = true;
                }
            }
            Event::Key(key) if key.code == KeyCode::PageUp => {
                self.url_first_row = self.url_first_row.saturating_sub(8);
            }
            Event::Key(key) if key.code == KeyCode::PageDown => {
                self.url_first_row = self.url_first_row.saturating_add(8);
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
            Position::new(area.x, area.y),
            Rect::new(
                area.x,
                area.y + RAW_URL_ROW,
                area.width,
                area.height.saturating_sub(RAW_URL_ROW + 3),
            ),
            self.url_first_row,
        ))
    }

    pub(in crate::setup) fn verifying(&self) -> bool {
        matches!(self.stage, Stage::Verifying(..))
    }

    pub(in crate::setup) fn render_authorization(
        &mut self,
        frame: &mut Frame<'_>,
        problem: Option<&str>,
    ) -> bool {
        let area = frame.area();
        let Some(link) = self.link(area) else {
            return false;
        };
        self.url_first_row = link.first_row();
        let mut row = 0;
        let clipboard = if self.url_copy_sent { " · Sent" } else { "" };
        let actions = format!("{}  [Ctrl+Y Copy URL]{clipboard}", AuthorizationLink::LABEL);
        line(frame, area, &mut row, &actions, ACCENT);
        let heading = if self.url_retry {
            "──── New authorization URL ────"
        } else {
            "──── Authorization URL ────"
        };
        line(frame, area, &mut row, heading, MUTED);
        row += 1;
        for (_, text) in link.rows() {
            line(frame, area, &mut row, text, ACCENT);
        }
        row += 1;
        let end = if link.last_page_start() == 0 {
            "──── End authorization URL ────"
        } else {
            "──── Page Up/Down to view the URL ────"
        };
        line(frame, area, &mut row, end, MUTED);
        if self.callback.text().is_empty() {
            let prompt = "Paste callback URL > ";
            let input_row = row;
            line(frame, area, &mut row, prompt, ACCENT);
            if input_row < area.height {
                frame.set_cursor_position((area.x + prompt.len() as u16, area.y + input_row));
            }
            if problem.is_none() {
                let help = if self.url_retry {
                    "OpenAI rejected the code. Open the new link above."
                } else {
                    "Copy the browser address even if the page fails to load."
                };
                line(frame, area, &mut row, help, MUTED);
            }
        } else {
            line(
                frame,
                area,
                &mut row,
                "Callback received · Enter to verify",
                MUTED,
            );
        }
        if let Some(problem) = problem {
            line(frame, area, &mut row, problem, WARNING);
        }
        line(
            frame,
            area,
            &mut row,
            "Esc previous step · Ctrl+C exit",
            MUTED,
        );
        true
    }

    pub(in crate::setup) fn render(&mut self, frame: &mut Frame<'_>, area: Rect, row: &mut u16) {
        line(frame, area, row, "Sign in with ChatGPT", ACCENT);
        match self.stage {
            Stage::Settings => {
                line(frame, area, row, "Account name · optional", MUTED);
                field(frame, area, row, &mut self.name);
                line(frame, area, row, "Enter to continue", MUTED);
            }
            Stage::Preparing(..) => line(frame, area, row, "Preparing authorization…", MUTED),
            Stage::Authorizing(..) => unreachable!("authorization uses the full terminal width"),
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
        self.step = SetupStep::Network;
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
                if matches!(flow.stage, Stage::Settings) {
                    self.previous();
                } else {
                    flow.abort_pending();
                    flow.stage = Stage::Settings;
                    flow.next_poll = None;
                    flow.callback.set_text(String::new());
                    self.problem = None;
                }
                return;
            }
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                self.finish = Some(Finish::Cancel);
                return;
            }
        }
        self.problem = match (&flow.stage, event) {
            (Stage::Settings, Event::Key(key)) if key.code == KeyCode::Enter => {
                flow.begin(settings, &self.proxy).err()
            }
            (Stage::Settings, Event::Key(key)) => {
                edit_field(&mut flow.name, key);
                None
            }
            (Stage::Settings, Event::Paste(text)) => {
                flow.name.insert(&text);
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
