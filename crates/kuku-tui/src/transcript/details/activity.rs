use std::collections::HashMap;

use ratatui::style::Style;
use ratatui::text::Line;

use crate::markup::render_literal;
use crate::text::shorten;
use crate::theme::{emphasis, CardFrame, TranscriptTone, ACCENT, MUTED};

use super::{
    inspection_composition, render_rows, GroupHeading, GroupKind, GroupSummary, Record, Row,
    GROUP_DETAIL_ROWS,
};

const ATTENTION_PREVIEW_LIMIT: usize = 3;
const OTHER_PREVIEW_LIMIT: usize = 2;

pub(super) fn summary(records: &[Record]) -> GroupSummary {
    let tools = || records.iter().filter_map(Record::tool);
    let mut preview = vec![0; records.len()];
    let mut repeated = HashMap::new();
    let mut changes = 0;
    let mut commands = 0;
    let mut attention = 0;
    let mut pending = 0;
    let mut other = 0;
    let mut shown_attention = 0;
    let mut shown_other = 0;
    let mut remaining = 0;
    for (index, tool) in tools().enumerate() {
        changes += tool.activity_changes();
        if tool.group_name().is_some() {
            continue;
        }
        let show_changes = tool.activity_has_changes();
        if tool.compact_file_change() {
            preview[index] = 1;
            continue;
        }
        let needs_attention = tool.activity_needs_attention();
        if needs_attention {
            attention += 1;
        } else if tool.activity_result_missing() {
            pending += 1;
        } else if tool.name() == "run_command" {
            commands += 1;
        } else {
            other += 1;
        }
        if show_changes {
            preview[index] = 1;
            continue;
        }
        let first = tool
            .activity_repeat_key()
            .map_or(index, |key| *repeated.entry(key).or_insert(index));
        if first != index {
            if preview[first] > 0 {
                preview[first] += 1;
            } else {
                remaining += 1;
            }
            continue;
        }
        let show = if needs_attention {
            shown_attention += 1;
            shown_attention <= ATTENTION_PREVIEW_LIMIT
        } else if tool.activity_running() {
            true
        } else {
            shown_other += 1;
            shown_other <= OTHER_PREVIEW_LIMIT
        };
        preview[index] = usize::from(show);
        remaining += usize::from(!show);
    }
    let inspection = inspection_composition(tools(), false);
    let mut inspections = inspection.split(" · ").filter(|part| !part.is_empty());
    let categories = [
        inspections.next().unwrap_or_default().to_owned(),
        count_label(changes, "file change", "file changes"),
        count_label(attention, "needs attention", "need attention"),
        count_label(commands, "command", "commands"),
        count_label(pending, "result not loaded", "results not loaded"),
        count_label(other, "other call", "other calls"),
    ];
    let mut labels = categories
        .iter()
        .map(String::as_str)
        .filter(|part| !part.is_empty())
        .chain(inspections);
    let title = labels.by_ref().take(3).collect::<Vec<_>>().join(" · ");
    let composition = labels.collect::<Vec<_>>().join(" · ");
    GroupSummary {
        title,
        composition,
        listing: String::new(),
        preview: None,
        kind: GroupKind::WorkspaceActivity { preview, remaining },
    }
}

fn count_label(count: usize, singular: &str, plural: &str) -> String {
    match count {
        0 => String::new(),
        1 => format!("1 {singular}"),
        _ => format!("{count} {plural}"),
    }
}

pub(in crate::transcript) fn footer(frame: &CardFrame, remaining: usize) -> Vec<Row> {
    if remaining == 0 {
        return Row::frame_end(frame);
    }
    let label = Row::plain(Row::fit(
        Row::metric(
            "",
            remaining.to_string(),
            if remaining == 1 {
                " more call"
            } else {
                " more calls"
            },
            TranscriptTone::Value,
        ),
        frame.content_width(),
    ));
    vec![label.framed_footer(frame), Row::plain(Line::default())]
}

pub(super) fn heading(
    summary: GroupSummary,
    open: bool,
    records: &[Record],
    frame: &CardFrame,
) -> GroupHeading {
    let workspace = matches!(summary.kind, GroupKind::WorkspaceActivity { .. });
    let label = if workspace || summary.composition.is_empty() || !summary.listing.is_empty() {
        &summary.title
    } else {
        &summary.composition
    };
    let mut rows = vec![Row::plain(Line::styled(
        shorten(label, frame.content_width()),
        emphasis(ACCENT),
    ))
    .framed_title(frame)];
    if workspace && open && !summary.composition.is_empty() {
        let counts = Row::caption(&summary.composition, "", MUTED, frame.content_width());
        rows.push(counts.framed(frame));
    }
    if !open {
        for detail in [summary.preview.as_deref().unwrap_or(""), &summary.listing] {
            if !detail.is_empty() {
                rows.extend(
                    render_rows(
                        render_literal(detail, Style::default()),
                        frame.content_width(),
                        GROUP_DETAIL_ROWS,
                        0,
                    )
                    .map(|row| row.framed(frame)),
                );
            }
        }
        if !workspace && rows.len() == 1 {
            rows.extend(super::inspection::preview(records, frame));
        }
    }
    GroupHeading {
        agent_link: None,
        summary,
        open,
        rows,
    }
}
