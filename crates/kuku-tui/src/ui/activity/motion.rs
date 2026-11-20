use ratatui::style::{Color, Style};
use ratatui::text::Line;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

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

const TYPING_STEP: usize = 3;
const FLOW_CELLS: usize = 40;
const MAX_BACKLOG_BYTES: usize = 256;

#[derive(Default)]
pub(super) struct ThinkingFlow {
    execution: Option<u64>,
    end: usize,
    last_tick: Option<u128>,
    batch_end: usize,
    visible: String,
}

impl ThinkingFlow {
    pub(super) fn sample(&mut self, source: Option<(u64, &str)>, tick: Option<u128>) -> &str {
        let Some((execution, source)) = source else {
            *self = Self::default();
            return "";
        };
        if self.execution != Some(execution)
            || self.end > source.len()
            || !source.is_char_boundary(self.end)
        {
            *self = Self {
                execution: Some(execution),
                ..Self::default()
            };
        }
        if tick == self.last_tick || self.end == source.len() {
            return &self.visible;
        }
        self.last_tick = tick;
        if self.end >= self.batch_end {
            let pending = &source[self.end..];
            if pending.len() > MAX_BACKLOG_BYTES {
                let skip = pending
                    .grapheme_indices(true)
                    .rev()
                    .nth(FLOW_CELLS - 1)
                    .map_or(0, |(at, _)| at);
                if skip > 0 && !self.visible.is_empty() {
                    self.visible.push_str(" … ");
                }
                self.end += skip;
            }
            self.batch_end = source.len();
        }
        let next = &source[self.end..];
        for grapheme in next.graphemes(true).take(TYPING_STEP) {
            self.end += grapheme.len();
            if grapheme.len() <= MAX_BACKLOG_BYTES {
                self.visible.push_str(&crate::text::single_line(grapheme));
            } else {
                self.visible.push('�');
            }
        }
        self.visible = flow_excerpt(&self.visible, FLOW_CELLS).to_owned();
        &self.visible
    }
}

pub(super) fn flow_excerpt(text: &str, width: usize) -> &str {
    let mut cells = 0;
    let start = text
        .grapheme_indices(true)
        .rev()
        .take(FLOW_CELLS)
        .take_while(|(_, g)| {
            cells += g.width();
            cells <= width.min(FLOW_CELLS)
        })
        .last()
        .map_or(text.len(), |(i, _)| i);
    &text[start..]
}

pub(super) fn thinking_label(label: &str, step: Option<u128>) -> Line<'static> {
    let index = step.map(|n| (n / 2 % (label.len() as u128 + 3)) as usize);
    let spans = label
        .char_indices()
        .map(|(i, c)| {
            let tone = if index == Some(i) {
                crate::theme::THINKING_GLINT
            } else {
                crate::theme::MUTED
            };
            ratatui::text::Span::styled(c.to_string(), Style::new().fg(tone))
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::{flow_excerpt, thinking_label, ThinkingFlow};
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn published_notes_are_paced_by_frames_even_during_bursts_or_pauses() {
        let mut flow = ThinkingFlow::default();
        assert_eq!("abc", flow.sample(Some((1, "abcdef")), Some(0)));
        assert_eq!("abc", flow.sample(Some((1, "abcdefghij")), Some(0)));
        assert_eq!("abcdef", flow.sample(Some((1, "abcdef")), Some(1)));
        assert_eq!("abcdef", flow.sample(Some((1, "abcdef")), Some(80)));
        let burst = format!(
            "abcdef{}{}",
            "obsolete ".repeat(400),
            "你e\u{301}好".repeat(20)
        );
        let joined = flow.sample(Some((1, &burst)), Some(81)).to_owned();
        assert!(joined.starts_with("abcdef … ") && !joined.contains("obsolete"));
        assert!(joined.width() <= 15 && flow_excerpt(&joined, 12).width() <= 12);
        let next_burst = format!("{burst}{}", "新的内容".repeat(100));
        assert_eq!(joined, flow.sample(Some((1, &next_burst)), Some(81)));
        assert!(flow
            .sample(Some((1, &next_burst)), Some(82))
            .starts_with(&joined));
        assert_eq!("", flow.sample(None, Some(83)));
        assert_eq!("新任务", flow.sample(Some((2, "新任务来了")), Some(84)));
        assert_ne!(
            thinking_label("Thinking", Some(0)),
            thinking_label("Thinking", Some(4))
        );
        assert_eq!("Thinking", thinking_label("Thinking", Some(5)).to_string());
    }
}
