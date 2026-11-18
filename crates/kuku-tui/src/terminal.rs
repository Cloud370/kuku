use std::io::{self, Stdout, Write};
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD, Engine};
use crossterm::cursor::{Hide, Show};
#[cfg(not(windows))]
use crossterm::event::EventStream;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
#[cfg(not(windows))]
use futures_util::StreamExt;
use kuku_client::{ConnectionOptions, Observation};
use ratatui::backend::CrosstermBackend;
use ratatui::{CompletedFrame, Terminal};

use crate::app::App;
use crate::config::load_statusline;
use crate::jobs::{Job, Jobs, Update};
use crate::statusline::StatusLine;
use crate::ui;

#[cfg(windows)]
mod windows;

const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const UPDATE_BATCH: usize = 32;

struct PaintSchedule {
    dirty: bool,
    urgent: bool,
    next_stream: Instant,
}

impl PaintSchedule {
    fn request(&mut self, urgent: bool) {
        self.dirty = true;
        self.urgent |= urgent;
    }

    fn ready(&self, now: Instant) -> bool {
        self.dirty && (self.urgent || now >= self.next_stream)
    }

    fn painted(&mut self, start: Instant, end: Instant) {
        let render_cost = end.duration_since(start);
        self.next_stream = end + (render_cost * 2).min(Duration::from_millis(120));
        self.dirty = false;
        self.urgent = false;
    }
}

fn apply_update(app: &mut App, update: Update, paint: &mut PaintSchedule) {
    let delta = matches!(&update, Update::Observation(_, Observation::Live { event, .. })
        if matches!(event["type"].as_str(), Some("text" | "thinking" | "tool_output")));
    app.update(update);
    paint.request(!delta);
}

pub(crate) async fn run(options: ConnectionOptions) -> io::Result<()> {
    let workspace = options
        .workspace
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    let mut app = App::new(workspace, options.remote.clone());
    let statusline = load_statusline(&options).unwrap_or_else(|problem| {
        app.problem(format!("Using the default status line · {problem}"));
        StatusLine::default()
    });
    let mut jobs = Jobs::new(options);
    let mut terminal = Screen::new(statusline)?;
    #[cfg(not(windows))]
    let mut events = EventStream::new();
    #[cfg(windows)]
    let mut events = windows::Input::new()?;
    let mut clipboard = Clipboard::default();
    let mut frames = tokio::time::interval(FRAME_INTERVAL);
    frames.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut screen = terminal.draw(&mut app)?;
    dispatch(&mut app, &mut jobs, &mut clipboard);
    let mut paint = PaintSchedule {
        dirty: false,
        urgent: false,
        next_stream: Instant::now(),
    };
    while !app.quit {
        tokio::select! {
            event = events.next() => match event {
                Some(Ok(event)) => {
                    app.event(event, screen.buffer);
                    paint.request(true);
                }
                Some(Err(error)) => return Err(error),
                None => break,
            },
            event = jobs.next() => {
                if let Some(event) = event {
                    apply_update(&mut app, event, &mut paint);
                    for _ in 1..UPDATE_BATCH {
                        let Some(event) = jobs.try_next() else { break };
                        apply_update(&mut app, event, &mut paint);
                    }
                }
            },
            _ = frames.tick() => {
                if app.tick() {
                    paint.request(true);
                }
                let start = Instant::now();
                if paint.ready(start) {
                    screen = terminal.draw(&mut app)?;
                    paint.painted(start, Instant::now());
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
                Ok(method) => app.confirmation(method),
                Err(error) => app.problem(format!("Clipboard · {error}")),
            }
        } else {
            jobs.start(effect);
        }
    }
}

struct Screen {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    statusline: StatusLine,
    _restore: Restore,
}

impl Screen {
    fn new(statusline: StatusLine) -> io::Result<Self> {
        let mut restore = Restore::new()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, Hide)?;
        restore.enable_input_modes(&mut stdout)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout)).map_err(io::Error::other)?;
        Ok(Self {
            terminal,
            statusline,
            _restore: restore,
        })
    }

    fn draw(&mut self, app: &mut App) -> io::Result<CompletedFrame<'_>> {
        self.terminal
            .draw(|frame| ui::render(frame, app, &self.statusline))
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

#[cfg(test)]
mod tests {
    use super::{Duration, Instant, PaintSchedule};

    #[test]
    fn streaming_bursts_share_frames_and_input_bypasses_the_render_budget() {
        let start = Instant::now();
        let mut paint = PaintSchedule {
            dirty: false,
            urgent: false,
            next_stream: start,
        };
        for _ in 0..10_000 {
            paint.request(false);
        }
        assert!(paint.ready(start));
        let end = start + Duration::from_millis(40);
        paint.painted(start, end);
        assert!(!paint.ready(end));
        paint.request(false);
        assert!(!paint.ready(end + Duration::from_millis(79)));
        assert!(paint.ready(end + Duration::from_millis(80)));
        paint.request(true);
        assert!(paint.ready(end));
    }
}
