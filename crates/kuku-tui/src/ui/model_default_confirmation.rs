use ratatui::style::Style;
use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;

use crate::model_default::SAVE_SCOPE;
use crate::theme::MUTED;

pub(super) fn wrap_scope(width: usize) -> Option<Vec<Line<'static>>> {
    let style = Style::default().fg(MUTED);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in SAVE_SCOPE.split_whitespace() {
        let word_width = UnicodeWidthStr::width(word);
        if word_width > width {
            return None;
        }
        if !line.is_empty() && UnicodeWidthStr::width(line.as_str()) + 1 + word_width > width {
            lines.push(Line::styled(std::mem::take(&mut line), style));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(Line::styled(line, style));
    }
    Some(lines)
}
