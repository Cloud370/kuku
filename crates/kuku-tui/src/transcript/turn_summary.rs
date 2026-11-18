use std::time::Duration;

use kuku_client::TurnSummary;
use ratatui::style::Style;
use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;

use crate::text::{compact_token_count, format_elapsed};
use crate::theme::MUTED;

use super::{body_inset, render_rows, Row};

pub(super) fn append_turn_summary(
    rows: &mut Vec<Row>,
    summary: Option<&TurnSummary>,
    width: usize,
) {
    let Some(row) = summary.and_then(|summary| render_turn_summary(summary, width)) else {
        return;
    };
    if rows.last().is_some_and(|row| row.line.spans.is_empty()) {
        rows.pop();
    }
    rows.push(row);
}

fn render_turn_summary(summary: &TurnSummary, width: usize) -> Option<Row> {
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
    let inset = body_inset(width);
    let columns = width.saturating_sub(usize::from(inset));
    let mut text = String::new();
    let separator = " · ";
    let mut used = 0;
    for label in labels.into_iter().flatten() {
        let separator_size = if text.is_empty() {
            0
        } else {
            separator.width()
        };
        let needed = separator_size + label.width();
        if used + needed > columns {
            continue;
        }
        if separator_size > 0 {
            text.push_str(separator);
        }
        text.push_str(&label);
        used += needed;
    }
    if text.is_empty() {
        return None;
    }
    render_rows(
        vec![Line::styled(text, Style::new().fg(MUTED))],
        width,
        1,
        inset,
    )
    .next()
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
    fn measured_turn_usage_is_one_row_after_the_reply_and_stays_out_of_copy() {
        let source = "## Final reply\n\nThe work is complete.";
        let mut transcript = Transcript::default();
        transcript.apply(entry(4, 1, json!({"type":"assistant","text":source})), None);
        assert!(!rows(&mut transcript, 65).join("\n").contains("tools"));
        transcript.apply(entry(5, 1, json!({"type":"turn_summary","turn_summary":{
            "elapsed_ms":65400,"input_tokens":12500,"output_tokens":800,"cache_hit_percent":75.5,"tool_calls":2
        }})), None);
        let wide = rows(&mut transcript, 70);
        let text = "1m 05s · 12.5K in · 800 out · Cache 75.5% · 2 tools";
        let row = wide.iter().position(|row| row.contains(text)).unwrap();
        assert_eq!("The work is complete.", wide[row - 1]);
        assert_eq!(1, wide.iter().filter(|row| row.contains("Turn 1")).count());
        assert_eq!(None, transcript.detail_at(row));
        assert_eq!(Some(ReplySource::Local(source)), transcript.last_reply());
        let narrow = rows(&mut transcript, 18);
        assert!(narrow.last().unwrap().contains("1m 05s"));
        assert!(narrow
            .iter()
            .all(|row| unicode_width::UnicodeWidthStr::width(row.as_str()) <= 18));
        assert!(!narrow.join("\n").contains("Cache"));
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
        assert_eq!(Some("  1 tool"), rendered.get(body + 1).map(String::as_str));
        assert_eq!(body + 2, rendered.len());
        assert_eq!(Some(ReplySource::Local(previous)), transcript.last_reply());
        assert!(transcript.toggle_source());
        let raw = rows(&mut transcript, 64);
        assert!(raw.iter().any(|line| line == "## Partial result"));
        assert_eq!("1 tool", raw.last().unwrap().trim());
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
        let joined = rows(&mut transcript, 60).join("\n");
        assert!(
            joined.contains("First reply\n  0 tools\nTurn 2"),
            "{joined}"
        );
        assert!(joined.contains("Second reply\n  8s · 3 tools"), "{joined}");
    }
}
