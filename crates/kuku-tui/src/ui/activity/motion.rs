use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::app::Activity;
use crate::theme::{ACCENT, ACTIVITY_BRIGHT, ACTIVITY_DIM, KUKU_MARK};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Sprite {
    glyphs: [&'static str; 3],
    tones: [Color; 3],
}

impl Sprite {
    pub(super) fn resting(tone: Color) -> Self {
        Self {
            glyphs: KUKU_MARK,
            tones: [tone; 3],
        }
    }

    pub(super) fn moving(activity: Activity, step: u128) -> Self {
        let mut mark = Self::resting(ACCENT);
        match activity {
            Activity::WaitingForModel | Activity::RetryingModel | Activity::Running => {
                let frame = (step / 2 % 6) as usize;
                let (wing, tone) = [
                    ("▖", ACTIVITY_DIM),
                    ("▚", ACCENT),
                    ("▙", ACTIVITY_BRIGHT),
                    ("▙", ACTIVITY_BRIGHT),
                    ("▚", ACCENT),
                    ("▖", ACTIVITY_DIM),
                ][frame];
                mark.glyphs[0] = wing;
                mark.tones = [tone; 3];
            }
            Activity::Thinking => {
                mark.glyphs[0] = ["▘", "▝", "▗", "▖"][(step % 4) as usize];
                mark.tones[0] = ACTIVITY_BRIGHT;
            }
            Activity::Streaming => {
                let cell = (step % 4) as usize;
                if cell < mark.glyphs.len() {
                    mark.glyphs[cell] = "▸";
                    mark.tones[cell] = ACTIVITY_BRIGHT;
                }
            }
            Activity::ToolArguments => {
                let cell = (step % 2) as usize;
                mark.glyphs[0] = ["▞", "▚"][cell];
                mark.tones[cell] = ACTIVITY_BRIGHT;
            }
            Activity::RunningTools => {
                if step % 5 == 0 {
                    mark.glyphs[0] = "▛";
                    mark.tones = [ACTIVITY_BRIGHT; 3];
                }
            }
            Activity::Ready
            | Activity::Opening
            | Activity::Configuration
            | Activity::Waiting
            | Activity::Stopping
            | Activity::Stopped
            | Activity::Interrupted
            | Activity::Finished
            | Activity::Failed => {}
        }
        mark
    }

    pub(super) fn line(self, compact: bool) -> Line<'static> {
        let cells = if compact { 1 } else { self.glyphs.len() };
        Line::from(
            self.glyphs
                .into_iter()
                .zip(self.tones)
                .take(cells)
                .map(|(glyph, color)| Span::styled(glyph, Style::default().fg(color)))
                .collect::<Vec<_>>(),
        )
    }
}
