use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::markup::{render_literal, wrap_bounded};
use crate::text::{replace_tabs, shorten, terminal_text};
use crate::theme::{emphasis, ERROR, MUTED, SUCCESS};

const COMPACT_DIFF_ROWS: usize = 4;

#[derive(Clone, Copy)]
pub(super) enum Presentation {
    Compact,
    Expanded(usize),
}

pub(super) fn render(
    text: &str,
    numbers: &[u64],
    width: usize,
    presentation: Presentation,
    truncated: bool,
) -> Vec<Line<'static>> {
    if width == 0 || text.is_empty() {
        return Vec::new();
    }
    let edits = text
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let sign = line.as_bytes().first().copied()?;
            matches!(sign, b'+' | b'-').then(|| Edit {
                sign: char::from(sign),
                number: numbers.get(index).copied().filter(|number| *number > 0),
                text: &line[1..],
            })
        })
        .collect::<Vec<_>>();
    let columns = Columns::new(width, numbers);
    let mut rows = Vec::new();
    match presentation {
        Presentation::Compact => {
            let mut visible = edits.iter().filter(|edit| !edit.text.trim().is_empty());
            for edit in visible.by_ref().take(COMPACT_DIFF_ROWS) {
                let body = replace_tabs(&terminal_text(edit.text));
                let body = Line::from(shorten(&body, columns.body));
                rows.push(columns.paint(edit, body, true));
            }
            if truncated || visible.next().is_some() {
                rows.push(Columns::more());
            }
        }
        Presentation::Expanded(limit) => {
            for edit in &edits {
                let body = if columns.body == 0 {
                    vec![Line::default()]
                } else {
                    wrap_bounded(
                        render_literal(edit.text, Style::default()),
                        columns.body,
                        limit - rows.len() + 1,
                    )
                };
                for (index, line) in body.into_iter().enumerate() {
                    if rows.len() == limit {
                        rows.push(Columns::more());
                        return rows;
                    }
                    rows.push(columns.paint(edit, line, index == 0));
                }
            }
            if truncated {
                rows.push(Columns::more());
            }
        }
    }
    rows
}

struct Edit<'a> {
    sign: char,
    number: Option<u64>,
    text: &'a str,
}

struct Columns {
    digits: usize,
    body: usize,
    after_marker: &'static str,
}

impl Columns {
    fn new(width: usize, numbers: &[u64]) -> Self {
        let digits = numbers
            .iter()
            .copied()
            .filter(|number| *number > 0)
            .max()
            .map_or(0, |number| number.to_string().len());
        let after_marker = if width > 1 { " " } else { "" };
        let marker = 1 + after_marker.len();
        let digits = if width > digits + 1 + marker {
            digits
        } else {
            0
        };
        let number = if digits > 0 { digits + 1 } else { 0 };
        Self {
            digits,
            body: width.saturating_sub(number + marker),
            after_marker,
        }
    }

    fn paint(&self, edit: &Edit<'_>, body: Line<'static>, first: bool) -> Line<'static> {
        let color = if edit.sign == '+' { SUCCESS } else { ERROR };
        let number_style = Style::new().fg(MUTED);
        let text_style = Style::new().fg(color);
        let mut row = Line::default();
        if self.digits != 0 {
            let number = if !first {
                String::new()
            } else if let Some(number) = edit.number {
                number.to_string()
            } else {
                "—".into()
            };
            row.push_span(Span::styled(
                format!("{number:>width$} ", width = self.digits),
                number_style,
            ));
        }
        let marker = if first { edit.sign } else { ' ' };
        row.push_span(Span::styled(
            format!("{marker}{}", self.after_marker),
            emphasis(color),
        ));
        for span in body.spans {
            row.push_span(Span::styled(span.content, text_style));
        }
        row
    }

    fn more() -> Line<'static> {
        Line::styled("…", Style::new().fg(MUTED))
    }
}

#[cfg(test)]
mod tests {
    use super::{render, Presentation, ERROR, MUTED, SUCCESS};

    #[test]
    fn unified_diff_keeps_numbers_dim_and_colors_plain_additions_and_deletions() {
        let rows = render(
            "-old interface name\n+new",
            &[98, 105],
            14,
            Presentation::Expanded(8),
            false,
        );
        let text: Vec<_> = rows.iter().map(ToString::to_string).collect();
        assert!(text[0].starts_with(" 98 - old"), "{text:?}");
        assert!(text[1].starts_with("      "), "{text:?}");
        assert_eq!(text.last().unwrap(), "105 + new");
        assert_eq!(rows[0].spans[0].style.fg, Some(MUTED));
        assert!(rows[0].spans[1..]
            .iter()
            .all(|span| span.style.fg == Some(ERROR)));
        assert!(rows.last().unwrap().spans[1..]
            .iter()
            .all(|span| span.style.fg == Some(SUCCESS)));
        assert!(rows
            .iter()
            .flat_map(|row| &row.spans)
            .all(|span| span.style.bg.is_none()));
        assert!(rows.iter().all(|row| row.width() <= 14));
        let narrow = render("+# heading", &[1], 1, Presentation::Compact, false);
        assert_eq!(narrow[0].to_string(), "+");
    }

    #[test]
    fn compact_preview_skips_whitespace_only_rows_before_counting_its_limit() {
        let diff = "-first\n-  \n+\t\n-second\n+replacement\n+more\n+omitted";
        let numbers = [1, 2, 3, 4, 5, 6, 7];
        let compact: Vec<_> = render(diff, &numbers, 24, Presentation::Compact, false)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            compact,
            [
                "1 - first",
                "4 - second",
                "5 + replacement",
                "6 + more",
                "…"
            ]
        );

        let expanded: Vec<_> = render(diff, &numbers, 24, Presentation::Expanded(8), false)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(expanded.len(), 7);
        assert_eq!(expanded[1], "2 -   ");
        assert_eq!(expanded[2], "3 +     ");

        let blanks = render("- \n+\t", &[2, 3], 24, Presentation::Compact, false);
        assert!(blanks.is_empty());
    }
}
