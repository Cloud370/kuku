use ratatui::style::{Color, Style};
use ratatui::text::Line;

use crate::app::Activity;
use crate::theme::{ACCENT, ACTIVITY_MINT, KUKU_MARK, MODEL, USAGE};

const ORBIT: [&str; 8] = ["⢌⡱", "⢆⡱", "⢎⡰", "⢎⡡", "⢎⡑", "⢎⠱", "⠎⡱", "⢊⡱"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Ring {
    glyph: &'static str,
    tone: Color,
}

impl Ring {
    pub(super) fn resting(tone: Color) -> Self {
        Self {
            glyph: KUKU_MARK,
            tone,
        }
    }

    pub(super) fn moving(activity: Activity, step: u128) -> Self {
        let tone = match activity {
            Activity::Thinking => MODEL,
            Activity::Streaming => ACTIVITY_MINT,
            Activity::ToolArguments | Activity::RunningTools => USAGE,
            _ => ACCENT,
        };
        Self {
            glyph: ORBIT[(step % ORBIT.len() as u128) as usize],
            tone,
        }
    }

    pub(super) fn line(self, width: usize) -> Line<'static> {
        let glyph = if width >= 2 { self.glyph } else { "·" };
        Line::styled(glyph, Style::new().fg(self.tone))
    }
}
