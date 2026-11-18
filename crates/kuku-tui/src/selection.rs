use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::{Buffer, CellWidth};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};

#[derive(Default)]
pub(crate) struct TextSelection {
    areas: Vec<Rect>,
    drag: Option<Drag>,
}

impl TextSelection {
    pub(crate) fn render(&mut self, buffer: &mut Buffer, areas: &[Rect]) {
        self.areas.clear();
        self.areas.extend_from_slice(areas);
        let Some(drag) = &self.drag else { return };
        if !areas.contains(&drag.area) {
            self.drag = None;
            return;
        }
        let (text, highlights) = drag.project(buffer);
        if text != drag.text {
            self.drag = None;
        } else if drag.anchor != drag.end {
            for row in highlights {
                buffer.set_style(row, Style::new().add_modifier(Modifier::REVERSED));
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
                    .map(|area| Drag::new(*area, point, buffer));
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(drag) = &mut self.drag {
                    drag.move_to(point, buffer);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let mut drag = self.drag.take()?;
                drag.move_to(point, buffer);
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
        self.drag = None;
    }
}

struct Drag {
    area: Rect,
    anchor: Position,
    end: Position,
    text: String,
}

impl Drag {
    fn new(area: Rect, anchor: Position, buffer: &Buffer) -> Self {
        let mut drag = Self {
            area,
            anchor,
            end: anchor,
            text: String::new(),
        };
        drag.text = drag.project(buffer).0;
        drag
    }

    fn move_to(&mut self, point: Position, buffer: &Buffer) {
        let end = Position::new(
            point.x.clamp(self.area.left(), self.area.right() - 1),
            point.y.clamp(self.area.top(), self.area.bottom() - 1),
        );
        if self.end != end {
            self.end = end;
            self.text = self.project(buffer).0;
        }
    }

    fn project(&self, buffer: &Buffer) -> (String, Vec<Rect>) {
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
            let from = if row == start.y {
                start.x
            } else {
                self.area.left()
            };
            let to = if row == end.y {
                end.x + 1
            } else {
                self.area.right()
            };
            let line_end = (self.area.left()..self.area.right())
                .rev()
                .find(|&x| buffer[(x, row)].symbol() != " ")
                .map_or(self.area.left(), |x| x + buffer[(x, row)].cell_width());
            let mut left = from;
            let mut width = 0;
            let mut x = self.area.left();
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
