use crate::theme::CardFrame;

use super::{Record, Row, Tool};

pub(super) fn preview(records: &[Record], frame: &CardFrame) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for row in records
        .iter()
        .rev()
        .filter_map(Record::tool)
        .filter_map(|tool| tool.inspection_preview(frame.content_width()))
    {
        if rows.iter().all(|previous| previous.line != row.line) {
            rows.push(row);
            if rows.len() == 2 {
                break;
            }
        }
    }
    rows.into_iter()
        .rev()
        .map(|row| row.framed(frame))
        .collect()
}

pub(super) const INSPECTIONS: [(&str, &str, &str); 9] = [
    ("read_file", "file read", "file reads"),
    ("find_files", "file search", "file searches"),
    ("search_text", "text search", "text searches"),
    ("list_agents", "Agent listing", "Agent listings"),
    ("query_session", "Session query", "Session queries"),
    ("stat_file", "file stat", "file stats"),
    ("read_output", "output read", "output reads"),
    ("search_output", "output search", "output searches"),
    ("list_values", "value listing", "value listings"),
];

pub(in crate::transcript) fn inspection_composition<'a>(
    tools: impl IntoIterator<Item = &'a Tool>,
    hide_agent_listing: bool,
) -> String {
    let mut totals = [0u64; INSPECTIONS.len()];
    for tool in tools {
        for (total, (name, _, _)) in totals.iter_mut().zip(INSPECTIONS) {
            *total = total.saturating_add(tool.inspection_count(name));
        }
    }
    totals
        .into_iter()
        .zip(INSPECTIONS)
        .filter(|(count, (name, _, _))| {
            *count != 0 && !(hide_agent_listing && *name == "list_agents")
        })
        .map(|(count, (_, one, many))| format!("{count} {}", if count == 1 { one } else { many }))
        .collect::<Vec<_>>()
        .join(" · ")
}
