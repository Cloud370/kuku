use std::io::{self, Stdout, Write};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    EventStream, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures_util::StreamExt;
use kuku_client::ConnectionOptions;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::app::App;
use crate::jobs::{Job, Jobs};
use crate::ui;

const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const UPDATE_BATCH: usize = 32;

pub(crate) async fn run(options: ConnectionOptions) -> io::Result<()> {
    let workspace = options
        .workspace
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    let mut app = App::new(workspace, options.remote.clone());
    let mut jobs = Jobs::new(options);
    let mut terminal = Screen::new()?;
    let mut events = EventStream::new();
    let mut clipboard = Clipboard::default();
    let mut frames = tokio::time::interval(FRAME_INTERVAL);
    frames.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    terminal.draw(&mut app)?;
    dispatch(&mut app, &mut jobs, &mut clipboard);
    let mut dirty = false;
    while !app.quit {
        tokio::select! {
            event = events.next() => match event {
                Some(Ok(event)) => {
                    app.event(event);
                    dirty = true;
                }
                Some(Err(error)) => return Err(error),
                None => break,
            },
            event = jobs.next() => {
                if let Some(event) = event {
                    app.update(event);
                    for _ in 1..UPDATE_BATCH {
                        let Some(event) = jobs.try_next() else { break };
                        app.update(event);
                    }
                    dirty = true;
                }
            },
            _ = frames.tick() => {
                app.tick();
                if dirty {
                    terminal.draw(&mut app)?;
                    dirty = false;
                }
            },
        }
        dispatch(&mut app, &mut jobs, &mut clipboard);
    }
    Ok(())
}

fn dispatch(app: &mut App, jobs: &mut Jobs, clipboard: &mut Clipboard) {
    for effect in std::mem::take(&mut app.effects) {
        if let Job::Copy(text) = effect {
            match clipboard.copy(text) {
                Ok(method) => app.message(method),
                Err(error) => app.problem(format!("Clipboard · {error}")),
            }
        } else {
            jobs.start(effect);
        }
    }
}

struct Screen {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    _restore: Restore,
}

impl Screen {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut restore = Restore {
            enhanced_keyboard: false,
        };
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableMouseCapture,
            Hide
        )?;
        restore.enhanced_keyboard = execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok();
        let terminal = Terminal::new(CrosstermBackend::new(stdout)).map_err(io::Error::other)?;
        Ok(Self {
            terminal,
            _restore: restore,
        })
    }

    fn draw(&mut self, app: &mut App) -> io::Result<()> {
        self.terminal
            .draw(|frame| ui::render(frame, app))
            .map(|_| ())
            .map_err(io::Error::other)
    }
}

struct Restore {
    enhanced_keyboard: bool,
}

impl Drop for Restore {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        if self.enhanced_keyboard {
            let _ = execute!(stdout, PopKeyboardEnhancementFlags);
        }
        let _ = execute!(stdout, DisableMouseCapture);
        let _ = execute!(stdout, DisableBracketedPaste);
        let _ = execute!(stdout, LeaveAlternateScreen);
        let _ = execute!(stdout, Show);
        let _ = disable_raw_mode();
    }
}

#[derive(Default)]
struct Clipboard {
    native: Option<arboard::Clipboard>,
}

impl Clipboard {
    fn copy(&mut self, text: String) -> io::Result<&'static str> {
        if self.native.is_none() {
            self.native = arboard::Clipboard::new().ok();
        }
        if let Some(clipboard) = &mut self.native {
            if clipboard.set_text(text.as_str()).is_ok() {
                return Ok("Copied assistant reply");
            }
        }
        let mut terminal = io::stdout().lock();
        write!(terminal, "\x1b]52;c;{}\x07", STANDARD.encode(text))?;
        terminal.flush()?;
        Ok("Sent assistant reply to terminal clipboard")
    }
}
