use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::{Buffer, CellWidth};
use ratatui::layout::{Position, Rect};
use crate::theme;

#[derive(Default)]
pub(crate) struct TextSelection {
    areas: Vec<Rect>,
    transcript_insets: Option<(Rect, Vec<u16>)>,
    drag: Option<Drag>,
}

impl TextSelection {
    pub(crate) fn render(
        &mut self,
        buffer: &mut Buffer,
        areas: &[Rect],
        transcript_insets: Option<(Rect, Vec<u16>)>,
    ) {
        self.areas.clear();
        self.areas.extend_from_slice(areas);
        self.transcript_insets = transcript_insets;
        let Some(drag) = &self.drag else { return };
        if !areas.contains(&drag.area) {
            self.drag = None;
            return;
        }
        let (text, highlights) =
            drag.project(buffer, insets_for(&self.transcript_insets, drag.area));
        if text != drag.text {
            self.drag = None;
        } else if drag.anchor != drag.end {
            for row in highlights {
                buffer.set_style(row, theme::selection());
            }
        }
    }

    pub(crate) fn mouse(&mut self, mouse: MouseEvent, buffer: &Buffer) -> Option<String> {
        if mouse.modifiers.contains(KeyModifiers::SHIFT) {
            self.drag = None;
            return None;
        }
        let point = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.drag = self
                    .areas
                    .iter()
                    .find(|area| area.contains(point))
                    .map(|area| {
                        Drag::new(
                            *area,
                            point,
                            buffer,
                            insets_for(&self.transcript_insets, *area),
                        )
                    });
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(drag) = &mut self.drag {
                    drag.move_to(
                        point,
                        buffer,
                        insets_for(&self.transcript_insets, drag.area),
                    );
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let mut drag = self.drag.take()?;
                drag.move_to(
                    point,
                    buffer,
                    insets_for(&self.transcript_insets, drag.area),
                );
                return (drag.anchor != drag.end && !drag.text.trim().is_empty())
                    .then_some(drag.text);
            }
            MouseEventKind::Moved => self.drag = None,
            MouseEventKind::Down(MouseButton::Middle | MouseButton::Right)
            | MouseEventKind::Drag(MouseButton::Middle | MouseButton::Right)
            | MouseEventKind::Up(MouseButton::Middle | MouseButton::Right)
            | MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => {}
        }
        None
    }

    pub(crate) fn clear(&mut self) {
        self.areas.clear();
        self.transcript_insets = None;
        self.drag = None;
    }
}

fn insets_for(transcript: &Option<(Rect, Vec<u16>)>, area: Rect) -> &[u16] {
    transcript
        .as_ref()
        .filter(|(rect, _)| *rect == area)
        .map_or(&[], |(_, insets)| insets)
}

struct Drag {
    area: Rect,
    anchor: Position,
    end: Position,
    text: String,
}

impl Drag {
    fn new(area: Rect, anchor: Position, buffer: &Buffer, insets: &[u16]) -> Self {
        let mut drag = Self {
            area,
            anchor,
            end: anchor,
            text: String::new(),
        };
        drag.text = drag.project(buffer, insets).0;
        drag
    }

    fn move_to(&mut self, point: Position, buffer: &Buffer, insets: &[u16]) {
        let end = Position::new(
            point.x.clamp(self.area.left(), self.area.right() - 1),
            point.y.clamp(self.area.top(), self.area.bottom() - 1),
        );
        if self.end != end {
            self.end = end;
            self.text = self.project(buffer, insets).0;
        }
    }

    fn project(&self, buffer: &Buffer, insets: &[u16]) -> (String, Vec<Rect>) {
        let (start, end) = if (self.anchor.y, self.anchor.x) <= (self.end.y, self.end.x) {
            (self.anchor, self.end)
        } else {
            (self.end, self.anchor)
        };
        let mut text = String::new();
        let mut highlights = Vec::new();
        for row in start.y..=end.y {
            if row != start.y {
                text.push('\n');
            }
            let inset = insets
                .get(usize::from(row - self.area.top()))
                .copied()
                .unwrap_or(0);
            let line_start = self.area.left() + inset;
            let from = if row == start.y {
                start.x.max(line_start)
            } else {
                line_start
            };
            let to = if row == end.y {
                end.x + 1
            } else {
                self.area.right()
            };
            let line_end = (line_start..self.area.right())
                .rev()
                .find(|&x| buffer[(x, row)].symbol() != " ")
                .map_or(line_start, |x| x + buffer[(x, row)].cell_width());
            let mut left = from;
            let mut width = 0;
            let mut x = line_start;
            while x < to.min(line_end) {
                let cell = &buffer[(x, row)];
                let next = x + cell.cell_width().max(1);
                if next > from {
                    text.push_str(cell.symbol());
                    left = left.min(x);
                    width += next - x;
                }
                x = next;
            }
            highlights.push(Rect::new(left, row, width, 1));
        }
        (text, highlights)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use kuku_client::{HistoryEntry, HistoryKind};
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Modifier;
    use ratatui::Terminal;

    use crate::app::App;
    use crate::jobs::Job;
    use crate::statusline::StatusLine;
    use crate::ui::render;

    #[test]
    fn mouse_drag_skips_message_insets_and_preserves_literal_indentation() {
        let mut app = App::new(PathBuf::from("/workspace"), None);
        app.effects.clear();
        let messages = [
            (HistoryKind::User, "before 中\n  de\u{301}tail\n👩‍💻! after"),
            (HistoryKind::Assistant, "✓"),
        ];
        for (index, (kind, text)) in messages.into_iter().enumerate() {
            app.pane_mut().transcript.apply(
                HistoryEntry {
                    id: index as u64 + 1,
                    agent: "main".into(),
                    turn: Some(1),
                    kind,
                    text: Some(text.into()),
                    details: Default::default(),
                },
                None,
            );
        }
        app.pane_mut().draft.insert("Draft stays here");
        let statusline = StatusLine::default();
        let mut terminal = Terminal::new(TestBackend::new(40, 24)).unwrap();
        let buffer = terminal
            .draw(|frame| render(frame, &mut app, &statusline))
            .unwrap()
            .buffer;
        let (x, y) = cell(buffer, "中");
        let end = cell(buffer, "✓");
        let emoji = cell(buffer, "👩‍💻");
        let left = MouseButton::Left;
        app.event(mouse(MouseEventKind::Down(left), (x + 1, y)), buffer);
        app.event(mouse(MouseEventKind::Drag(left), end), buffer);
        let buffer = terminal
            .draw(|frame| render(frame, &mut app, &statusline))
            .unwrap()
            .buffer;
        assert!(buffer[emoji].modifier.contains(Modifier::REVERSED));
        assert!(!buffer[(emoji.0 - 1, emoji.1)]
            .modifier
            .contains(Modifier::REVERSED));
        app.event(mouse(MouseEventKind::Up(left), end), buffer);
        assert!(
            matches!(app.effects.as_slice(), [Job::Copy(text)] if text == "中\n  de\u{301}tail\n👩‍💻! after\n\nAssistant\n✓")
        );
        assert_eq!("Draft stays here", app.pane().draft.text());
    }

    fn cell(buffer: &Buffer, symbol: &str) -> (u16, u16) {
        let index = buffer
            .content
            .iter()
            .position(|cell| cell.symbol() == symbol)
            .unwrap_or_else(|| panic!("missing screen symbol {symbol:?}"));
        buffer.pos_of(index)
    }

    fn mouse(kind: MouseEventKind, (column, row): (u16, u16)) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }
}
