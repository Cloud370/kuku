use std::collections::HashMap;

use kuku::adapter::{
    ModelTrial, Probe, ProbeBatch, ProbeProgress, ProbeResult, ProbeRow, ReasoningCheck,
    SetupTextCheck,
};
use kuku::config::ProviderConfig;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::text::shorten_middle;
use crate::theme::{ACCENT, MUTED, SUCCESS, WARNING};

use super::probes::{finished, seconds_until, timed_out};
use super::recovery::text_problem;
use super::ui::line;

const COMPLETE: usize = 1000;
const COLUMNS: [&str; 4] = ["Model", "Text", "Thinking", "Image"];

type Cell = (String, Color);

enum CheckActivity {
    Paused { seconds: u64, sent: bool },
    Running { seconds: u64 },
    Queued,
}

impl CheckActivity {
    fn cell(self) -> Cell {
        match self {
            Self::Paused { seconds, sent } => {
                let verb = if sent { "Retry" } else { "Start" };
                (format!("{verb} {seconds}s"), WARNING)
            }
            Self::Running { seconds } => (format!("Testing {seconds}s"), ACCENT),
            Self::Queued => ("Queued".into(), MUTED),
        }
    }
}

#[derive(Default)]
struct ModelChecks<'a> {
    text: CheckGroup<'a>,
    thinking: CheckGroup<'a>,
    image: CheckGroup<'a>,
    pricing: CheckGroup<'a>,
}

impl<'a> ModelChecks<'a> {
    fn add(&mut self, row: &'a ProbeRow) {
        let group = match &row.probe {
            Probe::ApiType { .. }
            | Probe::Trial {
                check: ModelTrial::Text { .. } | ModelTrial::Tools { .. },
                ..
            } => &mut self.text,
            Probe::Thinking { .. } => &mut self.thinking,
            Probe::Trial {
                check: ModelTrial::ImageInput { .. },
                ..
            } => &mut self.image,
            Probe::Quotes { .. } => &mut self.pricing,
            Probe::Catalog(_) => return,
        };
        group.0.push(row);
    }

    fn groups(&self) -> [&CheckGroup<'a>; 4] {
        [&self.text, &self.thinking, &self.image, &self.pricing]
    }
}

#[derive(Default)]
struct CheckGroup<'a>(Vec<&'a ProbeRow>);

impl CheckGroup<'_> {
    fn progress(&self) -> usize {
        let mut discovery = None;
        let mut settled = 0;
        let mut requests = 0;
        for row in &self.0 {
            let done = finished(&row.progress);
            match row.probe {
                Probe::Thinking {
                    check: ReasoningCheck::Discover,
                    ..
                } => {
                    discovery = Some(done);
                }
                _ => {
                    settled += usize::from(done);
                    requests += 1;
                }
            }
        }
        let checks = (COMPLETE * settled)
            .checked_div(requests)
            .unwrap_or(COMPLETE);
        match discovery {
            Some(false) => 0,
            Some(true) => (COMPLETE + checks) / 2,
            None => checks,
        }
    }

    fn activity(&self) -> Option<CheckActivity> {
        if let Some((until, sent)) = self
            .0
            .iter()
            .filter_map(|row| match row.progress {
                ProbeProgress::RetryWaiting { until, sent } => Some((until, sent)),
                _ => None,
            })
            .min_by_key(|(until, sent)| (!sent, *until))
        {
            return Some(CheckActivity::Paused {
                seconds: seconds_until(until),
                sent,
            });
        }
        if let Some(started) = self
            .running()
            .filter_map(|row| match row.progress {
                ProbeProgress::Waiting(started) => Some(started),
                _ => None,
            })
            .min()
        {
            return Some(CheckActivity::Running {
                seconds: started.elapsed().as_secs(),
            });
        }
        self.0
            .iter()
            .any(|row| matches!(row.progress, ProbeProgress::Queued))
            .then_some(CheckActivity::Queued)
    }

    fn running(&self) -> impl Iterator<Item = &ProbeRow> {
        self.0
            .iter()
            .copied()
            .filter(|row| matches!(row.progress, ProbeProgress::Waiting(_)))
    }

    fn idle_cell(&self, unverified: Color) -> Cell {
        if self.0.is_empty() {
            ("Not tested".into(), MUTED)
        } else if self.0.iter().any(|row| timed_out(&row.progress)) {
            ("Timed out".into(), WARNING)
        } else if self
            .0
            .iter()
            .all(|row| matches!(row.progress, ProbeProgress::Canceled { .. }))
        {
            ("Stopped".into(), MUTED)
        } else {
            ("Unverified".into(), unverified)
        }
    }

    fn image_cell(&self) -> Cell {
        self.activity().map(CheckActivity::cell).unwrap_or_else(|| {
            let verified = self.0.iter().any(|row| matches!(
                &row.progress,
                ProbeProgress::Finished(_, Ok(ProbeResult::Trial(trial))) if trial.image_match == Some(true)
            ));
            if verified {
                ("Verified".into(), SUCCESS)
            } else {
                self.idle_cell(WARNING)
            }
        })
    }

    fn thinking_cell(&self, levels: &[&str], width: usize) -> Cell {
        let usable = levels.join(", ");
        if let Some(activity) = self.activity() {
            let (activity, color) = match activity {
                CheckActivity::Running { seconds } => {
                    (self.thinking_activity(width, seconds), ACCENT)
                }
                activity => activity.cell(),
            };
            if levels.is_empty() {
                return (activity, color);
            }
            let with_levels = format!("{usable} · {activity}");
            if with_levels.width() <= width {
                return (with_levels, color);
            }
            let with_count = format!("{activity} · {} ok", levels.len());
            if with_count.width() <= width {
                return (with_count, color);
            }
            return (activity, color);
        }
        if levels.is_empty() {
            return self.idle_cell(MUTED);
        }
        if usable.width() <= width {
            return (usable, SUCCESS);
        }
        for visible in (1..levels.len()).rev() {
            let compact = format!(
                "{} +{}",
                levels[..visible].join(", "),
                levels.len() - visible
            );
            if compact.width() <= width {
                return (compact, SUCCESS);
            }
        }
        (format!("{} usable", levels.len()), SUCCESS)
    }

    fn thinking_activity(&self, width: usize, seconds: u64) -> String {
        let time = format!(" {seconds}s");
        let width = width.saturating_sub(time.len());
        let mut running = self.running().filter_map(|row| match &row.probe {
            Probe::Thinking { check, .. } => Some(check),
            _ => None,
        });
        let Some(ReasoningCheck::Level(first)) = running.next() else {
            let label = if width >= "Find levels".len() {
                "Find levels"
            } else {
                "Finding"
            };
            return format!("{label}{time}");
        };
        let levels = std::iter::once(first.as_str())
            .chain(running.filter_map(|check| match check {
                ReasoningCheck::Level(level) => Some(level.as_str()),
                ReasoningCheck::Discover => None,
            }))
            .collect::<Vec<_>>();
        let names = levels.join(", ");
        let label = format!("Try {names}");
        if label.width() <= width {
            return format!("{label}{time}");
        }
        let active = format!("… {names}");
        if active.width() <= width {
            return format!("{active}{time}");
        }
        if levels.len() > 1 {
            let compact = format!("{first}+{}", levels.len() - 1);
            if compact.width() <= width {
                return format!("{compact}{time}");
            }
        }
        format!("{} tests{time}", levels.len())
    }

    fn text_cell(&self, status: SetupTextCheck) -> Cell {
        match status {
            SetupTextCheck::Ready => ("Ready".into(), SUCCESS),
            SetupTextCheck::Failed { .. } => (text_problem(status).into(), WARNING),
            _ => self
                .activity()
                .map(CheckActivity::cell)
                .unwrap_or_else(|| self.idle_cell(MUTED)),
        }
    }
}

pub(super) struct TestOverview<'a> {
    batch: &'a ProbeBatch,
    models: &'a [String],
    checks: HashMap<&'a str, ModelChecks<'a>>,
}

impl<'a> TestOverview<'a> {
    pub(super) fn new(batch: &'a ProbeBatch, models: &'a [String]) -> Self {
        let mut checks = HashMap::<&str, ModelChecks<'_>>::new();
        for row in batch.selected_rows() {
            if let Some(model) = row.probe.model() {
                checks.entry(model).or_default().add(row);
            }
        }
        Self {
            batch,
            models,
            checks,
        }
    }

    pub(super) fn render_progress(&self, frame: &mut Frame<'_>, area: Rect, row: &mut u16) {
        let mut done = 0;
        let mut total = 0;
        for checks in self.checks.values() {
            for group in checks.groups() {
                if !group.0.is_empty() {
                    done += group.progress();
                    total += COMPLETE;
                }
            }
        }
        let width = 24;
        let percent = (100 * done).checked_div(total).unwrap_or(0);
        let filled = (width * done).checked_div(total).unwrap_or(0);
        let color = if percent == 100 { SUCCESS } else { ACCENT };
        let bar = Line::from(vec![
            Span::styled("Overall   [", Style::default().fg(MUTED)),
            Span::styled("━".repeat(filled), Style::default().fg(color)),
            Span::styled("─".repeat(width - filled), Style::default().fg(MUTED)),
            Span::styled("]", Style::default().fg(MUTED)),
            Span::raw(format!(" {percent}%")),
        ]);
        frame.render_widget(
            Paragraph::new(bar),
            Rect::new(area.x, area.y + *row, area.width, 1),
        );
        *row += 1;
    }

    pub(super) fn render_table(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        row: &mut u16,
        connection: Option<&ProviderConfig>,
        room: u16,
    ) {
        if self.models.is_empty() || room == 0 {
            return;
        }
        if connection.is_some_and(|connection| connection.options.subscription.is_some())
            && self.batch.selected_rows().next().is_none()
        {
            let room = usize::from(room);
            let remainder_row = usize::from(self.models.len() > room && room > 1);
            let visible = self.models.len().min(room - remainder_row);
            for model in self.models.iter().take(visible) {
                let name = shorten_middle(model, usize::from(area.width));
                line(frame, area, row, &name, ACCENT);
            }
            if visible < self.models.len() && visible < room {
                line(
                    frame,
                    area,
                    row,
                    &format!("+{} more models", self.models.len() - visible),
                    MUTED,
                );
            }
            return;
        }
        if room < 3 {
            line(
                frame,
                area,
                row,
                &format!("{} models", self.models.len()),
                MUTED,
            );
            return;
        }

        let columns = columns(area, self.models);
        table_row(frame, columns, row, COLUMNS.map(|label| (label, MUTED)));
        let spacing = usize::from(columns[1].x - columns[0].right());
        let divider = columns
            .iter()
            .zip([spacing - 2, spacing - 1, spacing - 1, 1])
            .map(|(column, padding)| "─".repeat(usize::from(column.width) + padding))
            .collect::<Vec<_>>()
            .join("┼");
        line(frame, area, row, &divider, MUTED);

        let room = usize::from(room - 2);
        let visible = if self.models.len() > room && room > 1 {
            room - 1
        } else {
            self.models.len().min(room)
        };
        let empty = ModelChecks::default();
        for model in self.models.iter().take(visible) {
            let checks = self.checks.get(model.as_str()).unwrap_or(&empty);
            let text = checks.text.text_cell(self.batch.text_check(model));
            let test =
                connection.and_then(|connection| self.batch.thinking_test(connection, model));
            let levels = test
                .as_ref()
                .map(|test| {
                    test.accepted
                        .iter()
                        .filter(|level| !test.rejected.contains(level))
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let thinking = checks
                .thinking
                .thinking_cell(&levels, usize::from(columns[2].width));
            let image = checks.image.image_cell();
            let name = shorten_middle(model, usize::from(columns[0].width));
            table_row(
                frame,
                columns,
                row,
                [
                    (&name, ACCENT),
                    (&text.0, text.1),
                    (&thinking.0, thinking.1),
                    (&image.0, image.1),
                ],
            );
        }
        if visible < self.models.len() && visible < room {
            line(
                frame,
                area,
                row,
                &format!("+{} more models", self.models.len() - visible),
                MUTED,
            );
        }
    }
}

fn columns(area: Rect, models: &[String]) -> [Rect; 4] {
    let text = 16;
    let image = 11;
    let spacing = if area.width >= 66 { 3 } else { 2 };
    let padding = 3 * spacing;
    let name_limit = area
        .width
        .saturating_sub(text + image + padding + 12)
        .clamp(10, 30);
    let name = models.iter().map(|model| model.width()).max().unwrap_or(0);
    let name = name.clamp(10, usize::from(name_limit)) as u16;
    let thinking = area.width.saturating_sub(name + text + image + padding);
    let mut x = area.x;
    [name, text, thinking, image].map(|width| {
        let column = Rect::new(x, area.y, width, area.height);
        x += width + spacing;
        column
    })
}

fn table_row(frame: &mut Frame<'_>, columns: [Rect; 4], row: &mut u16, cells: [(&str, Color); 4]) {
    for (index, (area, (text, color))) in columns.into_iter().zip(cells).enumerate() {
        let mut cell_row = *row;
        line(frame, area, &mut cell_row, text, color);
        if index > 0 {
            let mut cell_row = *row;
            line(
                frame,
                Rect::new(area.x - 2, area.y, 1, area.height),
                &mut cell_row,
                "│",
                MUTED,
            );
        }
    }
    *row += 1;
}
