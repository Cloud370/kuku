use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Modal};
use crate::keys::Shortcut;
use crate::markup::{render_literal, wrap};
use crate::text::{shorten, single_line};
use crate::theme::{focused_row, ERROR, MUTED};

use super::{Field, StatusLine};

const HELP: &str = "Enter Save · Esc Cancel · ↑↓ Select · Space Toggle · ←→ Reorder · R Defaults";

#[derive(Debug)]
pub(crate) struct StatusLineEditor {
    layout: StatusLine,
    selected: usize,
    overridden: bool,
    problem: Option<String>,
}

impl StatusLineEditor {
    pub(crate) fn move_selection(&mut self, amount: isize) {
        self.selected = self
            .selected
            .saturating_add_signed(amount)
            .min(Field::all().count().saturating_sub(1));
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        if area.is_empty() {
            return;
        }
        let width = usize::from(area.width);
        let height = usize::from(area.height);
        let help = wrap(vec![Line::styled(HELP, Style::default().fg(MUTED))], width);
        let help_rows = help.len().min(height.saturating_sub(1)).min(3);
        let description = self.description(width);
        let notes = description.len().min(height.saturating_sub(help_rows + 1));
        let [header, list, footer] = Layout::vertical([
            Constraint::Length(notes as u16),
            Constraint::Min(0),
            Constraint::Length(help_rows as u16),
        ])
        .areas(area);
        frame.render_widget(Paragraph::new(description), header);
        frame.render_widget(Paragraph::new(help), footer);
        let visible = usize::from(list.height);
        let first = self.selected.saturating_sub(visible.saturating_sub(1));
        let fields = self
            .ordered_fields()
            .enumerate()
            .skip(first)
            .take(visible)
            .map(|(index, field)| {
                let selected = index == self.selected;
                let checked = self.layout.fields().contains(&field);
                let pointer = if selected { '›' } else { ' ' };
                let check = if checked { 'x' } else { ' ' };
                let mut text = shorten(&format!("{pointer} [{check}] {}", field.label()), width);
                let style = if selected {
                    text.push_str(&" ".repeat(width - text.width()));
                    focused_row()
                } else if checked {
                    Style::default()
                } else {
                    Style::default().fg(MUTED)
                };
                Line::styled(text, style)
            })
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(fields), list);
    }

    fn description(&self, width: usize) -> Vec<Line<'static>> {
        if let Some(error) = &self.problem {
            return wrap(
                render_literal(&single_line(error), Style::default().fg(ERROR)),
                width,
            );
        }
        let message = if self.overridden {
            "Editing tui.toml. KUKU_TUI_STATUSLINE stays active after saving."
        } else if self.layout.fields().is_empty() {
            "Preview hidden. Select fields to show the status line."
        } else {
            "Changes preview in the status line."
        };
        wrap(
            vec![Line::styled(message, Style::default().fg(MUTED))],
            width,
        )
    }

    fn key(&mut self, key: KeyEvent) {
        if !key.modifiers.is_empty() && key.modifiers != KeyModifiers::SHIFT {
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::Down => {
                let count = Field::all().count() as isize;
                let step = if key.code == KeyCode::Up { -1 } else { 1 };
                self.selected = (self.selected as isize + step).rem_euclid(count) as usize;
            }
            KeyCode::Left => self.reorder(-1),
            KeyCode::Right => self.reorder(1),
            KeyCode::Char(' ') => self.toggle(),
            KeyCode::Char('r' | 'R') => {
                self.layout = StatusLine::default();
                self.selected = 0;
            }
            _ => {}
        }
    }

    fn ordered_fields(&self) -> impl Iterator<Item = Field> + '_ {
        let enabled = self.layout.fields();
        enabled
            .iter()
            .copied()
            .chain(Field::all().filter(|field| !enabled.contains(field)))
    }

    fn toggle(&mut self) {
        let Some(field) = self.ordered_fields().nth(self.selected) else {
            return;
        };
        let mut fields = self.layout.fields().to_vec();
        if let Some(index) = fields.iter().position(|current| *current == field) {
            fields.remove(index);
        } else {
            fields.push(field);
        }
        self.layout.fields = Some(fields);
        let new_index = self.ordered_fields().position(|current| current == field);
        self.selected = new_index.expect("selected status line field");
    }

    fn reorder(&mut self, amount: isize) {
        let mut fields = self.layout.fields().to_vec();
        let Some(next) = self
            .selected
            .checked_add_signed(amount)
            .filter(|next| *next < fields.len())
        else {
            return;
        };
        if self.selected < fields.len() {
            fields.swap(self.selected, next);
            self.layout.fields = Some(fields);
            self.selected = next;
        }
    }
}

impl App {
    pub(crate) fn open_statusline(&mut self) {
        let (layout, problem) = match self.statusline_settings.read() {
            Ok(layout) => (layout, None),
            Err(problem) => (StatusLine::default(), Some(problem)),
        };
        self.modal = Some(Modal::StatusLineEditor(StatusLineEditor {
            layout,
            selected: 0,
            overridden: self.statusline_settings.overridden(),
            problem,
        }));
    }

    pub(crate) fn displayed_statusline(&self) -> &StatusLine {
        match &self.modal {
            Some(Modal::StatusLineEditor(editor)) => &editor.layout,
            _ => self.statusline_settings.active(),
        }
    }

    pub(crate) fn statusline_key(&mut self, key: KeyEvent) {
        let Some(Modal::StatusLineEditor(editor)) = &mut self.modal else {
            return;
        };
        if !Shortcut::ENTER.matches(key) {
            editor.key(key);
            return;
        }
        if let Err(problem) = self.statusline_settings.save(editor.layout.clone()) {
            editor.problem = Some(problem);
            return;
        }
        self.modal = None;
        let message = if self.statusline_settings.overridden() {
            "Status line saved · environment override active"
        } else {
            "Status line saved"
        };
        self.confirmation(message);
    }
}

impl Field {
    fn label(self) -> &'static str {
        match self {
            Self::Model => "Model",
            Self::Connection => "Connection",
            Self::Execution => "Execution",
            Self::Agent => "Agent",
            Self::Workspace => "Workspace",
            Self::Context => "Context usage",
            Self::Throughput => "Tokens per second",
            Self::InputTokens => "Input tokens",
            Self::OutputTokens => "Output tokens",
            Self::Cache => "Cache hit rate",
            Self::Cost => "Request cost",
            Self::Input => "Input feedback",
        }
    }
}
