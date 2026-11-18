use pulldown_cmark::Alignment;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::MUTED;

use super::{append, wrap_bounded};

const MIN_CONTENT_WIDTH: usize = 6;
const COLUMN_PADDING_AND_EDGE: usize = 3;

type Cell = Vec<Line<'static>>;

pub(super) struct Table {
    alignments: Vec<Alignment>,
    rows: Vec<Vec<Cell>>,
    cell: Cell,
}

impl Table {
    pub(super) fn new(alignments: Vec<Alignment>) -> Self {
        Self {
            alignments,
            rows: Vec::new(),
            cell: Vec::new(),
        }
    }

    pub(super) fn start_row(&mut self) {
        self.rows.push(Vec::new());
    }

    pub(super) fn push_cell_line(&mut self, line: Line<'static>) {
        self.cell.push(line);
    }

    pub(super) fn finish_cell(&mut self) {
        self.rows
            .last_mut()
            .expect("table cell belongs to a row")
            .push(std::mem::take(&mut self.cell));
    }

    pub(super) fn render(self, width: usize, limit: usize) -> Vec<Line<'static>> {
        let max_lines = limit.saturating_add(1);
        match self.column_widths(width) {
            Some(columns) => self.render_grid(&columns, max_lines),
            None => self.render_stacked(width, max_lines),
        }
    }

    fn column_widths(&self, width: usize) -> Option<Vec<usize>> {
        let budget = width.checked_sub(self.alignments.len() * COLUMN_PADDING_AND_EDGE + 1)?;
        let mut natural = vec![1; self.alignments.len()];
        for row in &self.rows {
            for (cell, column) in row.iter().zip(&mut natural) {
                for line in cell {
                    *column = (*column).max(line.width());
                }
            }
        }
        let total: usize = natural.iter().sum();
        if total <= budget {
            return Some(natural);
        }
        let mut widths: Vec<_> = natural
            .iter()
            .map(|&width| width.min(MIN_CONTENT_WIDTH))
            .collect();
        let minimum: usize = widths.iter().sum();
        let mut spare = budget.checked_sub(minimum)?;
        let mut wanted = total - minimum;
        for (width, natural) in widths.iter_mut().zip(natural) {
            if spare == 0 {
                break;
            }
            let growth = natural - *width;
            let share = ((spare as u128 * growth as u128) / wanted as u128) as usize;
            *width += share;
            spare -= share;
            wanted -= growth;
        }
        Some(widths)
    }

    fn render_grid(self, columns: &[usize], limit: usize) -> Vec<Line<'static>> {
        let mut lines = vec![render_border(columns, ["┌", "┬", "┐"])];
        let separator = render_border(columns, ["├", "┼", "┤"]);
        for (row_index, row) in self.rows.into_iter().enumerate() {
            if lines.len() == limit {
                return lines;
            }
            if row_index > 0 {
                lines.push(separator.clone());
            }
            let remaining = limit - lines.len();
            lines.extend(render_grid_row(
                row,
                columns,
                &self.alignments,
                row_index == 0,
                remaining,
            ));
        }
        if lines.len() < limit {
            lines.push(render_border(columns, ["└", "┴", "┘"]));
        }
        lines
    }

    fn render_stacked(self, width: usize, limit: usize) -> Vec<Line<'static>> {
        let mut rows = self.rows.into_iter();
        let header = rows.next().unwrap_or_default();
        if rows.as_slice().is_empty() {
            return wrap_bounded(header.into_iter().flatten().collect(), width, limit - 1);
        }
        let mut lines = Vec::new();
        for (row_index, row) in rows.enumerate() {
            if lines.len() == limit {
                return lines;
            }
            if row_index > 0 {
                lines.push(Line::default());
            }
            for (index, cell) in row.into_iter().enumerate() {
                if lines.len() == limit {
                    return lines;
                }
                let field = labelled_cell(header.get(index), index, cell);
                lines.extend(wrap_bounded(field, width, limit - lines.len() - 1));
            }
        }
        lines
    }
}

fn render_border(widths: &[usize], edges: [&str; 3]) -> Line<'static> {
    let [left, joint, right] = edges;
    let mut line = left.to_owned();
    for (index, width) in widths.iter().enumerate() {
        if index > 0 {
            line.push_str(joint);
        }
        line.push_str(&"─".repeat(width + 2));
    }
    line.push_str(right);
    Line::styled(line, Style::default().fg(MUTED))
}

fn render_grid_row(
    row: Vec<Cell>,
    columns: &[usize],
    alignments: &[Alignment],
    header: bool,
    limit: usize,
) -> Vec<Line<'static>> {
    let mut row = row.into_iter();
    let wrapped: Vec<_> = columns
        .iter()
        .map(|&width| wrap_bounded(row.next().unwrap_or_default(), width, limit))
        .collect();
    let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
    (0..height.min(limit))
        .map(|line| {
            let border_style = Style::default().fg(MUTED);
            let mut spans = vec![Span::styled("│", border_style)];
            for (index, (&width, &alignment)) in columns.iter().zip(alignments).enumerate() {
                let content = wrapped[index].get(line);
                let padding = width - content.map_or(0, Line::width);
                let left = match alignment {
                    Alignment::None | Alignment::Left => 0,
                    Alignment::Center => padding / 2,
                    Alignment::Right => padding,
                };
                append(&mut spans, &" ".repeat(left + 1), Style::default());
                if let Some(content) = content {
                    for span in &content.spans {
                        let style = if header {
                            span.style.add_modifier(Modifier::BOLD)
                        } else {
                            span.style
                        };
                        append(&mut spans, &span.content, style);
                    }
                }
                append(
                    &mut spans,
                    &" ".repeat(padding - left + 1),
                    Style::default(),
                );
                append(&mut spans, "│", border_style);
            }
            Line::from(spans)
        })
        .collect()
}

fn labelled_cell(header: Option<&Cell>, index: usize, cell: Cell) -> Vec<Line<'static>> {
    let mut label = Vec::new();
    for line in header.into_iter().flatten() {
        if !label.is_empty() {
            append(&mut label, " ", Style::default());
        }
        for span in &line.spans {
            append(
                &mut label,
                &span.content,
                span.style.add_modifier(Modifier::BOLD),
            );
        }
    }
    if label.is_empty() {
        label.push(Span::styled(
            format!("Column {}", index + 1),
            Style::default().add_modifier(Modifier::BOLD),
        ));
    }
    append(&mut label, ": ", Style::default());
    let mut value = cell.into_iter();
    if let Some(first) = value.next() {
        label.extend(first.spans);
    }
    let mut lines = vec![Line::from(label)];
    lines.extend(value);
    lines
}
