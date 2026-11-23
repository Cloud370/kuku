use std::io::{self, Stdout, Write};
use std::time::{Duration, Instant, SystemTime};

use base64::{engine::general_purpose::STANDARD, Engine};
use crossterm::cursor::{Hide, MoveTo, Show};
#[cfg(not(windows))]
use crossterm::event::EventStream;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::terminal::{
    BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{execute, queue};
#[cfg(not(windows))]
use futures_util::StreamExt;
use kuku_client::{ConnectionOptions, Observation};
use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::{Buffer, Cell, CellDiffOption, CellWidth};
use ratatui::layout::{Position, Rect, Size};
use ratatui::{CompletedFrame, Terminal};

use crate::app::{Address, App};
use crate::config::load_statusline;
use crate::jobs::{Job, Jobs, Update};
use crate::ui::{self, ActivityClock, ActivityFrame};

#[cfg(windows)]
mod windows;

const FRAME_INTERVAL: Duration = Duration::from_millis(25);
const ACTIVITY_SAMPLE_INTERVAL: Duration = Duration::from_millis(33);
const MAX_RENDER_COOLDOWN: Duration = Duration::from_millis(60);
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

    fn deadline(&self, now: Instant) -> Option<Instant> {
        self.dirty
            .then_some(if self.urgent { now } else { self.next_stream })
    }

    fn ready(&self, now: Instant) -> bool {
        self.deadline(now).is_some_and(|deadline| deadline <= now)
    }

    fn painted(&mut self, start: Instant, end: Instant) {
        let render_cost = end.duration_since(start);
        self.next_stream = (start + FRAME_INTERVAL).max(end + render_cost.min(MAX_RENDER_COOLDOWN));
        self.dirty = false;
        self.urgent = false;
    }
}

fn apply_update(
    app: &mut App,
    update: Update,
    activity: &mut ActivityClock,
    paint: &mut PaintSchedule,
) {
    if matches!(update, Update::Observation(_, Observation::Heartbeat)) {
        return;
    }
    let delta = matches!(&update, Update::Observation(_, Observation::Live { event, .. })
        if matches!(event["type"].as_str(), Some("text" | "thinking" | "reasoning_preview" | "tool_arguments" | "tool_output" | "model_output_usage")));
    app.update(update);
    activity.observe(app, Instant::now());
    paint.request(!delta);
}

pub(crate) async fn run(options: ConnectionOptions, build: &'static str) -> io::Result<()> {
    let workspace = options
        .workspace
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    let mut app = App::new(workspace, options.remote.clone());
    let (settings, problem) = load_statusline(&options);
    app.statusline_settings = settings;
    if let Some(problem) = problem {
        app.problem(format!("Using the default status line · {problem}"));
    }
    let mut jobs = Jobs::new(options, build);
    let mut terminal = Screen::new(app.active.clone())?;
    #[cfg(not(windows))]
    let mut events = EventStream::new();
    #[cfg(windows)]
    let mut events = windows::Input::new()?;
    let mut clipboard = Clipboard::default();
    let mut next_animation = Instant::now() + ACTIVITY_SAMPLE_INTERVAL;
    let mut activity = ActivityClock::default();
    let mut painted_activity = activity.sample(&app, Instant::now(), SystemTime::now());
    let (mut screen, mut activity_visible) = terminal.draw(&mut app, painted_activity.clone())?;
    dispatch(&mut app, &mut jobs, &mut clipboard);
    let mut paint = PaintSchedule {
        dirty: false,
        urgent: false,
        next_stream: Instant::now(),
    };
    while !app.quit {
        let animate = activity.confirming_phase()
            || (activity_visible && painted_activity.changes_with_time());
        let wake = [
            paint.deadline(Instant::now()),
            animate.then_some(next_animation),
            app.next_deadline(),
        ]
        .into_iter()
        .flatten()
        .min();
        tokio::select! {
            event = events.next() => match event {
                Some(Ok(event)) => {
                    app.event(event, screen.buffer);
                    activity.observe(&app, Instant::now());
                    paint.request(true);
                }
                Some(Err(error)) => return Err(error),
                None => break,
            },
            event = jobs.next() => {
                if let Some(event) = event {
                    apply_update(&mut app, event, &mut activity, &mut paint);
                    for _ in 1..UPDATE_BATCH {
                        let Some(event) = jobs.try_next() else { break };
                        apply_update(&mut app, event, &mut activity, &mut paint);
                    }
                }
            },
            _ = tokio::time::sleep_until(wake.unwrap_or(next_animation).into()), if wake.is_some() => {
                if app.tick() {
                    paint.request(true);
                }
                let start = Instant::now();
                if start >= next_animation {
                    next_animation = start + ACTIVITY_SAMPLE_INTERVAL;
                }
                let frame = activity.sample(&app, start, SystemTime::now());
                if frame.phase_changed(&painted_activity) || (activity_visible && frame != painted_activity) {
                    paint.request(false);
                }
                if paint.ready(start) {
                    (screen, activity_visible) = terminal.draw(&mut app, frame.clone())?;
                    painted_activity = frame;
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
    terminal: Terminal<FrameBackend<Stdout>>,
    painted_target: Address,
    wide_cells: WideCells,
    _restore: Restore,
}

impl Screen {
    fn new(painted_target: Address) -> io::Result<Self> {
        let mut restore = Restore::new()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, Hide)?;
        restore.enable_input_modes(&mut stdout)?;
        let terminal = Terminal::new(FrameBackend::new(stdout))?;
        Ok(Self {
            terminal,
            painted_target,
            wide_cells: WideCells::default(),
            _restore: restore,
        })
    }

    fn draw(
        &mut self,
        app: &mut App,
        activity: ActivityFrame,
    ) -> io::Result<(CompletedFrame<'_>, bool)> {
        let target_changed = self.painted_target != app.active;
        if target_changed {
            self.painted_target.clone_from(&app.active);
        }
        if target_changed {
            let area = self.terminal.current_buffer_mut().area;
            self.terminal.resize(area)?;
            self.wide_cells.spans.clear();
        }
        let mut activity_visible = false;
        let wide_cells = &mut self.wide_cells;
        let frame = self.terminal.draw(|frame| {
            activity_visible = ui::render_at(frame, app, activity);
            wide_cells.repair(frame.buffer_mut());
        })?;
        Ok((frame, activity_visible))
    }
}

#[derive(Default)]
struct WideCells {
    area: Rect,
    spans: Vec<std::ops::Range<usize>>,
}

impl WideCells {
    fn repair(&mut self, buffer: &mut Buffer) {
        if self.area == buffer.area {
            for span in self.spans.drain(..) {
                let current_width = buffer.content[span.start].cell_width() as usize;
                for cell in
                    &mut buffer.content[(span.start + current_width.max(1)).min(span.end)..span.end]
                {
                    if cell.diff_option == CellDiffOption::None {
                        cell.diff_option = CellDiffOption::AlwaysUpdate;
                    }
                }
            }
        } else {
            self.spans.clear();
        }
        self.area = buffer.area;
        let row_width = usize::from(self.area.width);
        if row_width == 0 {
            return;
        }
        for (index, cell) in buffer.content.iter().enumerate() {
            let width = cell.cell_width() as usize;
            if width > 1 {
                let end = index + width.min(row_width - index % row_width);
                self.spans.push(index..end);
            }
        }
    }
}

struct FrameBackend<W> {
    output: W,
    commands: Vec<u8>,
    position: Position,
    visible: bool,
    painted_cursor: Option<Position>,
}

impl<W: Write> FrameBackend<W> {
    fn new(output: W) -> Self {
        Self {
            output,
            commands: Vec::new(),
            position: Position::ORIGIN,
            visible: false,
            painted_cursor: None,
        }
    }

    fn begin(&mut self) -> io::Result<()> {
        if self.commands.is_empty() {
            queue!(self.commands, BeginSynchronizedUpdate)?;
            if self.painted_cursor.is_some() {
                queue!(self.commands, Hide)?;
            }
        }
        Ok(())
    }
}

impl<W: Write> Backend for FrameBackend<W> {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut content = content.peekable();
        if content.peek().is_none() {
            return Ok(());
        }
        self.begin()?;
        CrosstermBackend::new(&mut self.commands).draw(content)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.visible = false;
        Ok(())
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.visible = true;
        Ok(())
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.position = position.into();
        Ok(())
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        CrosstermBackend::new(io::sink()).get_cursor_position()
    }

    fn size(&self) -> io::Result<Size> {
        CrosstermBackend::new(io::sink()).size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        CrosstermBackend::new(io::sink()).window_size()
    }

    fn clear(&mut self) -> io::Result<()> {
        self.clear_region(ClearType::All)
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.begin()?;
        CrosstermBackend::new(&mut self.commands).clear_region(clear_type)
    }

    fn flush(&mut self) -> io::Result<()> {
        let cursor = self.visible.then_some(self.position);
        if self.commands.is_empty() && cursor == self.painted_cursor {
            return Ok(());
        }
        self.begin()?;
        let bytes = &mut self.commands;
        if let Some(Position { x, y }) = cursor {
            queue!(bytes, MoveTo(x, y), Show)?;
        }
        queue!(bytes, EndSynchronizedUpdate)?;
        self.output.write_all(bytes)?;
        self.output.flush()?;
        bytes.clear();
        self.painted_cursor = cursor;
        Ok(())
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
    use ratatui::backend::{Backend, ClearType};
    use ratatui::buffer::Cell;

    use super::{Duration, FrameBackend, Instant, PaintSchedule, FRAME_INTERVAL};

    #[test]
    fn a_wide_glyph_moving_left_repaints_its_old_background() {
        use std::num::NonZeroU16;

        use ratatui::backend::{Backend, CrosstermBackend};
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::style::{Modifier, Style};

        let area = Rect::new(0, 0, 5, 1);
        let blank = Buffer::empty(area);
        let mut old = blank.clone();
        let inverted = Style::new().add_modifier(Modifier::REVERSED);
        old.set_string(1, 0, "雪", inverted);
        let mut next = blank.clone();
        next.set_string(0, 0, "雪", inverted);
        let mut wide = super::WideCells::default();
        wide.repair(&mut old);
        let mut bytes = Vec::new();
        let mut backend = CrosstermBackend::new(&mut bytes);
        backend.draw(blank.diff_iter(&old)).unwrap();
        wide.repair(&mut next);
        backend.draw(old.diff_iter(&next)).unwrap();

        let mut terminal = vt100::Parser::new(NonZeroU16::MIN, NonZeroU16::new(5).unwrap(), 0);
        terminal.process(&bytes);
        let screen = terminal.screen();
        assert!(screen.cell(0, 0).unwrap().inverse());
        assert!(
            !screen.cell(0, 2).unwrap().inverse(),
            "{:?}",
            String::from_utf8_lossy(&bytes)
        );
    }

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
        assert!(!paint.ready(end + Duration::from_millis(39)));
        assert_eq!(paint.deadline(end), Some(end + Duration::from_millis(40)));
        assert!(paint.ready(end + Duration::from_millis(40)));
        paint.request(true);
        assert!(paint.ready(end));
    }
    #[test]
    fn short_frames_keep_a_fixed_interval_between_their_starts() {
        let start = Instant::now();
        let mut paint = PaintSchedule {
            dirty: true,
            urgent: false,
            next_stream: start,
        };
        let end = start + Duration::from_millis(2);
        paint.painted(start, end);
        paint.request(false);
        assert_eq!(paint.deadline(end), Some(start + FRAME_INTERVAL));
    }

    #[test]
    fn terminal_frames_keep_cursor_hidden_during_cell_updates_and_move_before_showing() {
        let mut backend = FrameBackend::new(Vec::new());
        backend.show_cursor().unwrap();
        backend.set_cursor_position((5, 4)).unwrap();
        backend.flush().unwrap();
        assert_eq!(backend.output, b"\x1b[?2026h\x1b[5;6H\x1b[?25h\x1b[?2026l");
        backend.output.clear();

        backend.clear_region(ClearType::All).unwrap();
        let mut cell = Cell::EMPTY;
        cell.set_symbol("X");
        backend.draw([(1, 1, &cell)].into_iter()).unwrap();
        backend.show_cursor().unwrap();
        backend.set_cursor_position((8, 6)).unwrap();
        assert!(backend.output.is_empty());
        backend.flush().unwrap();
        let output = String::from_utf8_lossy(&backend.output);
        assert!(output.starts_with("\x1b[?2026h\x1b[?25l\x1b[2J"));
        assert!(output.contains("\x1b[2;2HX"));
        assert!(output.ends_with("\x1b[7;9H\x1b[?25h\x1b[?2026l"));

        backend.output.clear();
        backend.draw(std::iter::empty()).unwrap();
        backend.flush().unwrap();
        assert!(backend.output.is_empty());
    }
}
