use std::io::{self, Stdout, Write};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine};
use crossterm::cursor::{Hide, Show};
#[cfg(not(windows))]
use crossterm::event::EventStream;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
#[cfg(not(windows))]
use futures_util::StreamExt;
use kuku_client::ConnectionOptions;
use ratatui::backend::CrosstermBackend;
use ratatui::{CompletedFrame, Terminal};

use crate::app::App;
use crate::jobs::{Job, Jobs};
use crate::ui;

#[cfg(windows)]
mod windows;

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
    #[cfg(not(windows))]
    let mut events = EventStream::new();
    #[cfg(windows)]
    let mut events = windows::Input::new()?;
    let mut clipboard = Clipboard::default();
    let mut frames = tokio::time::interval(FRAME_INTERVAL);
    frames.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut screen = terminal.draw(&mut app)?;
    dispatch(&mut app, &mut jobs, &mut clipboard);
    let mut dirty = false;
    while !app.quit {
        tokio::select! {
            event = events.next() => match event {
                Some(Ok(event)) => {
                    app.event(event, screen.buffer);
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
                    screen = terminal.draw(&mut app)?;
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
        let mut restore = Restore::new()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, Hide)?;
        restore.enable_input_modes(&mut stdout)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout)).map_err(io::Error::other)?;
        Ok(Self {
            terminal,
            _restore: restore,
        })
    }

    fn draw(&mut self, app: &mut App) -> io::Result<CompletedFrame<'_>> {
        self.terminal
            .draw(|frame| ui::render(frame, app))
            .map_err(io::Error::other)
    }
}

struct Restore {
    enhanced_keyboard: bool,
    #[cfg(windows)]
    _modes: windows::ConsoleModes,
}

impl Restore {
    fn new() -> io::Result<Self> {
        #[cfg(not(windows))]
        crossterm::terminal::enable_raw_mode()?;
        Ok(Self {
            enhanced_keyboard: false,
            #[cfg(windows)]
            _modes: windows::ConsoleModes::enter()?,
        })
    }

    #[cfg(not(windows))]
    fn enable_input_modes(&mut self, stdout: &mut Stdout) -> io::Result<()> {
        use crossterm::event::{
            EnableMouseCapture, KeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
        };

        execute!(stdout, EnableMouseCapture)?;
        self.enhanced_keyboard = execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok();
        Ok(())
    }

    #[cfg(windows)]
    fn enable_input_modes(&mut self, stdout: &mut Stdout) -> io::Result<()> {
        stdout.write_all(b"\x1b[?1002h\x1b[?1006h")?;
        self.enhanced_keyboard = true;
        stdout.write_all(b"\x1b[>1u")?;
        stdout.flush()
    }

    #[cfg(not(windows))]
    fn disable_input_modes(&self, stdout: &mut Stdout) {
        use crossterm::event::{DisableMouseCapture, PopKeyboardEnhancementFlags};

        if self.enhanced_keyboard {
            let _ = execute!(stdout, PopKeyboardEnhancementFlags);
        }
        let _ = execute!(stdout, DisableMouseCapture);
    }

    #[cfg(windows)]
    fn disable_input_modes(&self, stdout: &mut Stdout) {
        if self.enhanced_keyboard {
            let _ = stdout.write_all(b"\x1b[<1u");
        }
        let _ = stdout.write_all(b"\x1b[?1006l\x1b[?1002l");
        let _ = stdout.flush();
    }
}

impl Drop for Restore {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        self.disable_input_modes(&mut stdout);
        let _ = execute!(stdout, DisableBracketedPaste);
        let _ = execute!(stdout, LeaveAlternateScreen);
        let _ = execute!(stdout, Show);
        #[cfg(not(windows))]
        let _ = crossterm::terminal::disable_raw_mode();
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
                return Ok("Copied to clipboard");
            }
        }
        let mut terminal = io::stdout().lock();
        write!(terminal, "\x1b]52;c;{}\x07", STANDARD.encode(text))?;
        terminal.flush()?;
        Ok("Sent text to terminal clipboard")
    }
}
