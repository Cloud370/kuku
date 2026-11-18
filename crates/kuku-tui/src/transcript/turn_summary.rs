use std::time::Duration;

use kuku_client::TurnSummary;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::text::{compact_token_count, format_elapsed};
use crate::theme::{emphasis, MUTED, TURN_CARD, USAGE};

use super::Transcript;
use crate::text::shorten;

impl Transcript {
    pub(crate) fn turn_dock(&self, width: usize) -> Option<Line<'static>> {
        let (window, record) = if let Some(reading) = &self.reading {
            (
                &reading.window,
                reading
                    .window
                    .records
                    .iter()
                    .find(|record| record.key == reading.anchor.key)?,
            )
        } else {
            (
                &self.recent,
                self.recent
                    .records
                    .iter()
                    .rev()
                    .find(|record| record.turn.is_some())?,
            )
        };
        let turn = record.turn?;
        let records = window
            .records
            .iter()
            .rev()
            .chain(self.recent.records.iter().rev())
            .filter(|record| record.turn == Some(turn));
        let number = record
            .turn_number
            .or_else(|| records.clone().find_map(|record| record.turn_number));
        let title = number.map_or_else(|| "Turn".into(), |n| format!("Turn {n}"));
        let mut line = Line::from(Span::styled(
            shorten(&format!("▌ {title}"), width),
            emphasis(USAGE),
        ));
        line.style = Style::new().bg(TURN_CARD);
        if let Some(summary) = records
            .filter_map(|record| record.turn_summary.as_ref())
            .next()
        {
            let metrics = render_metrics(summary, width.saturating_sub(line.width() + 3));
            if metrics.width() > 0 {
                line.push_span("   ");
                line.extend(metrics.spans);
            }
        }
        Some(line)
    }
}

fn render_metrics(summary: &TurnSummary, width: usize) -> Line<'static> {
    let tools = if summary.tool_calls == 1 {
        "tool"
    } else {
        "tools"
    };
    let labels = [
        summary
            .elapsed_ms
            .map(|millis| format_elapsed(Duration::from_millis(millis))),
        summary
            .input_tokens
            .map(|count| format!("{} in", compact_token_count(count))),
        summary
            .output_tokens
            .map(|count| format!("{} out", compact_token_count(count))),
        summary
            .cache_hit_percent
            .filter(|rate| (0.0..=100.0).contains(rate))
            .map(cache_hit),
        Some(format!("{} {tools}", summary.tool_calls)),
    ];
    let muted = Style::new().fg(MUTED);
    let mut line = Line::default();
    for label in labels.into_iter().flatten() {
        let separator = if line.spans.is_empty() { "" } else { " · " };
        if line.width() + separator.width() + label.width() <= width {
            line.push_span(Span::styled(separator, muted));
            line.push_span(Span::styled(
                label,
                Style::new().fg(crate::theme::SURFACE_TEXT),
            ));
        }
    }
    line
}

fn cache_hit(rate: f64) -> String {
    let rate = format!("{rate:.1}");
    format!("Cache {}%", rate.strip_suffix(".0").unwrap_or(&rate))
}

#[cfg(test)]
mod tests {
    use kuku_client::{HistoryEntry, HistoryPage};
    use serde_json::{json, Value};

    use super::super::{HistoryRequest, ReplySource, Transcript};

    fn entry(id: u64, turn: u64, mut value: Value) -> HistoryEntry {
        value["id"] = json!(id);
        value["turn"] = json!(turn);
        value["turn_number"] = json!(turn);
        value["agent"] = json!("main");
        serde_json::from_value(value).unwrap()
    }

    fn rows(transcript: &mut Transcript, width: usize) -> Vec<String> {
        transcript
            .project(width, 25)
            .lines
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn measured_turn_usage_is_docked_and_stays_out_of_copy() {
        let source = "## Final reply\n\nThe work is complete.";
        let mut transcript = Transcript::default();
        transcript.apply(entry(4, 1, json!({"type":"assistant","text":source})), None);
        assert_eq!("▌ Turn 1", transcript.turn_dock(65).unwrap().to_string());
        transcript.apply(entry(5, 1, json!({"type":"turn_summary","turn_summary":{
            "elapsed_ms":65400,"input_tokens":12500,"output_tokens":800,"cache_hit_percent":75.5,"tool_calls":2
        }})), None);
        assert!(!rows(&mut transcript, 90).join("\n").contains("2 tools"));
        assert_eq!(
            "▌ Turn 1   1m 05s · 12.5K in · 800 out · Cache 75.5% · 2 tools",
            transcript.turn_dock(90).unwrap().to_string()
        );
        assert_eq!(Some(ReplySource::Local(source)), transcript.last_reply());
        let narrow = transcript.turn_dock(18).unwrap();
        assert!(narrow.width() <= 18 && narrow.to_string().contains("1m 05s"));
    }

    #[test]
    fn interrupted_output_keeps_its_role_and_only_shows_known_counts() {
        let previous = "Previous settled reply";
        let mut transcript = Transcript::default();
        transcript.apply(
            entry(4, 1, json!({"type":"assistant","text":previous})),
            None,
        );
        transcript.apply(entry(8, 2, json!({"type":"status", "status":"interrupted", "execution_id":7,"text":"## Partial result", "turn_summary":{"tool_calls":1}})), Some(7));
        let rendered = rows(&mut transcript, 64);
        let header = rendered
            .iter()
            .position(|line| line == "Interrupted")
            .unwrap();
        let body = rendered
            .iter()
            .position(|line| line == "Partial result")
            .unwrap();
        assert!(header < body);
        assert_eq!(
            "▌ Turn 2   1 tool",
            transcript.turn_dock(64).unwrap().to_string()
        );
        assert_eq!(Some(ReplySource::Local(previous)), transcript.last_reply());
        assert!(transcript.toggle_source());
        let raw = rows(&mut transcript, 64);
        assert!(raw.iter().any(|line| line == "## Partial result"));
        assert_eq!(
            "▌ Turn 2   1 tool",
            transcript.turn_dock(64).unwrap().to_string()
        );
    }

    #[test]
    fn older_pages_add_turn_summaries_without_rebasing_the_reader() {
        let mut transcript = Transcript::default();
        transcript.replace_recent(HistoryPage {
            entries: vec![
                entry(8, 2, json!({"type":"assistant","text":"Second reply"})),
                entry(9, 2, json!({"type":"turn_summary","turn_summary":{"elapsed_ms":8000,"tool_calls":3}})),
            ],
            next_before: Some(8),
        });
        assert_eq!(
            Some(HistoryRequest::Older(8)),
            transcript.scroll(-30, 60, 8)
        );
        let before = rows(&mut transcript, 60);
        transcript.apply_page(
            HistoryRequest::Older(8),
            HistoryPage {
                entries: vec![
                    entry(2, 1, json!({"type":"assistant","text":"First reply"})),
                    entry(
                        3,
                        1,
                        json!({"type":"turn_summary","turn_summary":{"tool_calls":0}}),
                    ),
                ],
                next_before: None,
            },
        );
        assert_eq!(before, rows(&mut transcript, 60));
        transcript.scroll(-20, 60, 8);
        assert_eq!(
            "▌ Turn 1   0 tools",
            transcript.turn_dock(60).unwrap().to_string()
        );
        transcript.show_latest();
        assert_eq!(
            "▌ Turn 2   8s · 3 tools",
            transcript.turn_dock(60).unwrap().to_string()
        );
    }
}
