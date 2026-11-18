use std::collections::{HashSet, VecDeque};
use std::ops::Range;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::text::shorten;
use crate::theme::{emphasis, ACCENT, MUTED, SUCCESS};

use super::agent::AgentCard;
use super::tool::{tool_title, Tool};
use super::{body_inset, Key, Record, Row, ToolDetails, Transcript, MAX_RECORDS};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetailKey {
    kind: DetailKind,
    record: Key,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailKind {
    Agent,
    Group,
    Tool,
}

#[derive(Debug, Default)]
pub(super) struct Details {
    pub(super) visible: Vec<VisibleDetail>,
    groups: VecDeque<Key>,
    tools: VecDeque<Key>,
}

impl Details {
    pub(super) fn agent_keys(&self) -> impl Iterator<Item = &Key> {
        self.visible
            .iter()
            .filter(|item| item.key.kind == DetailKind::Agent)
            .map(|item| &item.key.record)
    }

    pub(super) fn group_open(&self, records: &[Record]) -> bool {
        records
            .iter()
            .any(|record| self.groups.contains(&record.key))
    }

    pub(super) fn tool_mode(&self, key: &Key, fallback: ToolDetails) -> ToolDetails {
        if self.tools.contains(key) {
            ToolDetails::Inspection
        } else {
            fallback
        }
    }

    pub(super) fn has_closed_group(&self) -> bool {
        self.visible
            .iter()
            .any(|item| item.key.kind == DetailKind::Group && !item.open)
    }

    pub(super) fn clear_expansions(&mut self) {
        self.groups.clear();
        self.tools.clear();
    }
}

#[derive(Debug)]
pub(super) struct VisibleDetail {
    rows: Range<usize>,
    key: DetailKey,
    label: String,
    summary: String,
    open: bool,
}

impl VisibleDetail {
    pub(super) fn group(row: usize, key: &Key, group: GroupHeading) -> Self {
        let GroupHeading { title, open } = group;
        Self::new(
            row..row + 1,
            key,
            DetailKind::Group,
            title,
            String::new(),
            open,
        )
    }

    pub(super) fn record(rows: Range<usize>, key: &Key, item: DetailRecord<'_>) -> Self {
        match item {
            DetailRecord::Agent(agent) => Self::new(
                rows,
                key,
                DetailKind::Agent,
                format!("Agent {}", agent.address),
                agent.choice_summary(),
                false,
            ),
            DetailRecord::Tool(tool) => {
                let (label, summary) = tool.detail_choice();
                Self::new(rows, key, DetailKind::Tool, label, summary, false)
            }
        }
    }

    fn new(
        rows: Range<usize>,
        key: &Key,
        kind: DetailKind,
        label: String,
        summary: String,
        open: bool,
    ) -> Self {
        Self {
            rows,
            key: DetailKey {
                kind,
                record: key.clone(),
            },
            label,
            summary,
            open,
        }
    }
}

pub(super) enum DetailRecord<'a> {
    Agent(AgentCard<'a>),
    Tool(&'a Tool),
}

impl Transcript {
    pub(crate) fn detail_at(&self, row: usize) -> Option<&DetailKey> {
        self.detail_items
            .visible
            .iter()
            .find(|item| item.rows.contains(&row))
            .map(|item| &item.key)
    }

    pub(crate) fn visible_detail_choices(&self) -> Vec<(DetailKey, String, String)> {
        self.detail_items
            .visible
            .iter()
            .rev()
            .map(|item| (item.key.clone(), item.label.clone(), item.summary.clone()))
            .collect()
    }

    pub(crate) fn toggle_detail(&mut self, key: &DetailKey) -> bool {
        let window = self
            .reading
            .as_ref()
            .map_or(&self.recent, |reading| &reading.window);
        match key.kind {
            DetailKind::Agent => self.toggle_agent_record(&key.record),
            DetailKind::Tool if window.contains(&key.record) => {
                if let Some(index) = self
                    .detail_items
                    .tools
                    .iter()
                    .position(|opened| opened == &key.record)
                {
                    self.detail_items.tools.remove(index);
                } else {
                    remember(&mut self.detail_items.tools, &key.record);
                }
                true
            }
            DetailKind::Tool => false,
            DetailKind::Group => {
                let Some(span) = groups(&window.records).into_iter().find(|span| {
                    window.records[span.range.clone()]
                        .iter()
                        .any(|record| record.key == key.record)
                }) else {
                    return false;
                };
                let records = &window.records[span.range];
                let open = !self.detail_items.group_open(records);
                if open {
                    remember(&mut self.detail_items.groups, &key.record);
                } else {
                    self.detail_items
                        .groups
                        .retain(|opened| !records.iter().any(|record| &record.key == opened));
                }
                if let Some(item) = self.detail_items.visible.iter_mut().find(|item| {
                    item.key.kind == DetailKind::Group
                        && records.iter().any(|record| record.key == item.key.record)
                }) {
                    item.open = open;
                }
                true
            }
        }
    }

    pub(super) fn toggle_group_details(&mut self, open: bool) {
        if !open {
            self.detail_items.clear_expansions();
        }
        for item in &mut self.detail_items.visible {
            if item.key.kind != DetailKind::Group {
                continue;
            }
            if open && !item.open {
                remember(&mut self.detail_items.groups, &item.key.record);
            }
            item.open = open;
        }
    }
}

fn remember(expanded: &mut VecDeque<Key>, key: &Key) {
    if expanded.len() == MAX_RECORDS {
        expanded.pop_front();
    }
    expanded.push_back(key.clone());
}

pub(super) struct GroupSpan {
    pub(super) range: Range<usize>,
    pub(super) title: String,
}

pub(super) struct GroupHeading {
    pub(super) title: String,
    pub(super) open: bool,
}

impl GroupHeading {
    pub(super) fn render(&self, width: usize) -> Row {
        let inset = body_inset(width);
        let columns = width.saturating_sub(usize::from(inset));
        let mut spans = Vec::new();
        if inset > 0 {
            spans.push(Span::styled(
                if self.open { "▾ " } else { "▸ " },
                Style::new().fg(MUTED),
            ));
        }
        let status = " · ok";
        let title = if columns > status.width() {
            shorten(&self.title, columns - status.width())
        } else {
            shorten(&self.title, columns)
        };
        spans.push(Span::styled(title, emphasis(ACCENT)));
        if columns > status.width() {
            spans.push(Span::styled(status, Style::new().fg(SUCCESS)));
        }
        Row {
            line: Line::from(spans),
            inset,
        }
    }
}

pub(super) fn groups(records: &[Record]) -> Vec<GroupSpan> {
    let mut groups = Vec::new();
    let mut index = 0;
    while let Some(first) = records.get(index) {
        let Some(name) = group_name(first) else {
            index += 1;
            continue;
        };
        let start = index;
        index += 1;
        while records.get(index).is_some_and(|record| {
            record.turn == first.turn
                && record.owner == first.owner
                && group_name(record) == Some(name)
        }) {
            index += 1;
        }
        if index > start + 1 {
            let title = group_title(name, &records[start..index]);
            groups.push(GroupSpan {
                range: start..index,
                title,
            });
        }
    }
    groups
}

fn group_name(record: &Record) -> Option<&str> {
    record.turn.filter(|&turn| turn > 0)?;
    record.owner.as_ref()?;
    record.tool()?.group_name()
}

fn group_title(name: &str, records: &[Record]) -> String {
    let count = records.len();
    if name == "read_file" {
        let unique: HashSet<_> = records
            .iter()
            .filter_map(|record| record.tool()?.read_path())
            .collect();
        if unique.len() == count {
            format!("Read {count} files")
        } else {
            format!("Read file ×{count}")
        }
    } else {
        format!("{} ×{count}", tool_title(name))
    }
}

#[cfg(test)]
mod tests {
    use kuku_client::{HistoryEntry, HistoryPage};
    use serde_json::{json, Value};

    use super::super::{AnchorLine, HistoryRequest, Transcript, TranscriptView};

    #[derive(Default)]
    struct Journal(Vec<HistoryEntry>);

    impl Journal {
        fn push(&mut self, turn: u64, agent: &str, mut event: Value) {
            event["id"] = json!(self.0.len() as u64 + 1);
            event["turn"] = json!(turn);
            event["agent"] = json!(agent);
            self.0.push(serde_json::from_value(event).unwrap());
        }

        fn tool(&mut self, name: &str, args: Value, mut result: Value) {
            let id = format!("call-{}", self.0.len());
            self.push(
                7,
                "main",
                json!({"type":"tool_call", "tool":name, "tool_call_id":id, "args":args}),
            );
            result["type"] = json!("tool_result");
            result["tool_call_id"] = json!(id);
            self.push(7, "main", result);
        }

        fn read(&mut self, turn: u64, agent: &str, path: &str) {
            let id = format!("read-{}", self.0.len());
            self.push(turn, agent, json!({"type":"tool_call","tool":"read_file","tool_call_id":id,"args":{"path":path,"offset":2}}));
            self.push(turn, agent, json!({"type":"tool_result","tool_call_id":id,"status":"ok","summary":format!("read {path}"),"model_content":format!("2\tcontents for {path}"),"structured":{"kind":"file_content","path":path,"start_line":2,"line_count":1,"total_lines":3}}));
        }

        fn load(self) -> Transcript {
            let mut transcript = Transcript::default();
            transcript.replace_recent(HistoryPage {
                entries: self.0,
                next_before: None,
            });
            transcript
        }
    }

    fn text(view: &TranscriptView) -> String {
        view.lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn line(view: &TranscriptView, part: &str) -> usize {
        view.lines
            .iter()
            .position(|line| line.to_string().contains(part))
            .unwrap_or_else(|| panic!("missing {part:?}\n{}", text(view)))
    }

    #[test]
    fn mixed_twenty_four_tools_compact_by_operation_and_remain_individually_inspectable() {
        let mut journal = Journal::default();
        for index in 1..=12 {
            journal.read(7, "main", &format!("samples/read-{index:02}.txt"));
        }
        for pattern in ["*.rs", "*.md", "*.toml"] {
            journal.tool(
                "find_files",
                json!({"pattern":pattern}),
                json!({"status":"ok","summary":"found files"}),
            );
        }
        for command in ["check-one", "check-two", "check-three"] {
            journal.tool("run_command", json!({"command":command}), json!({"status":"ok","summary":"exited","structured":{"exit_code":0,"command":command}}));
        }
        journal.tool("run_command", json!({"command":"check-failed"}), json!({"status":"ok","summary":"check failed","model_content":"actual diagnostic","structured":{"exit_code":23}}));
        journal.tool("run_command", json!({"command":"check-warning"}), json!({"status":"ok","summary":"captured stderr","model_content":"actual warning","structured":{"exit_code":0,"stderr_lines":1}}));
        journal.tool("read_file", json!({"path":"missing.txt"}), json!({"status":"error","summary":"File is missing","model_content":"No such file: missing.txt"}));
        journal.tool(
            "ask_user",
            json!({"question":"Continue?"}),
            json!({"status":"ok","summary":"session operation committed"}),
        );
        journal.push(
            7,
            "main",
            json!({"type":"permission","text":"Confirm the next operation"}),
        );
        journal.push(7, "main", json!({"type":"tool_call","tool":"run_command","tool_call_id":"active","args":{"command":"checking now"}}));
        journal.push(7, "main", json!({"type":"tool_call","tool":"read_file","tool_call_id":"requested","args":{"path":"queued.txt"}}));
        let mut transcript = journal.load();
        transcript.live(
            4,
            Some(7),
            None,
            &json!({"type":"tool_start","tool":"run_command","id":"active"}),
        );
        assert_eq!(24, transcript.tools().len());

        let compact = transcript.project(90, 64);
        let rendered = text(&compact);
        assert!(rendered.contains("Read 12 files · ok") && rendered.contains("Find files ×3 · ok"));
        assert!(rendered.contains("Run command ×3 · ok") && rendered.contains("failed · exit 23"));
        for term in [
            "actual diagnostic",
            "ok with warnings",
            "actual warning",
            "No such file",
            "Permission",
            "Confirm the next operation",
            "running",
            "requested",
        ] {
            assert!(rendered.contains(term), "{term} missing\n{rendered}");
        }
        assert!(!rendered.contains("samples/read-12.txt"));
        assert!(transcript.recent.records[..12]
            .iter()
            .all(|record| record.rendered.is_none()));
        assert_eq!(Some("Ctrl+O show · /details"), compact.tool_hint.as_deref());
        let group = transcript
            .detail_at(line(&compact, "Read 12 files"))
            .unwrap()
            .clone();
        assert!(transcript.toggle_detail(&group));
        let expanded = transcript.project(90, 64);
        assert!(text(&expanded).contains("samples/read-12.txt"));
        assert!(!text(&expanded).contains("contents for samples/read-12.txt"));
        let (record, _, _) = transcript
            .visible_detail_choices()
            .into_iter()
            .find(|(_, _, summary)| summary.contains("samples/read-12.txt"))
            .unwrap();
        transcript.project(36, 2);
        assert!(transcript.toggle_detail(&record));
        let detail = text(&transcript.project(90, 64));
        assert!(detail.contains("Requested arguments") && detail.contains("\"offset\":2"));
        assert!(detail.contains("contents for samples/read-12.txt"));
        assert!(transcript.toggle_detail(&group));
        let compact_again = text(&transcript.project(90, 64));
        assert_eq!(rendered, compact_again);
    }

    #[test]
    fn summaries_do_not_merge_across_prose_turns_addresses_or_missing_metadata() {
        let mut journal = Journal::default();
        journal.read(7, "main", "same.txt");
        journal.read(7, "main", "same.txt");
        journal.push(
            7,
            "main",
            json!({"type":"assistant","text":"I will check a different file."}),
        );
        journal.read(7, "main", "a.txt");
        journal.read(7, "reviewer", "b.txt");
        journal.read(9, "reviewer", "c.txt");
        journal.read(0, "main", "d.txt");
        journal.read(0, "main", "e.txt");
        let mut transcript = journal.load();
        let view = text(&transcript.project(90, 50));
        assert_eq!(1, view.matches("Read file ×2 · ok").count());
        assert!(!view.contains("Read 2 files"));
        for path in ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"] {
            assert!(view.contains(path), "{path} missing\n{view}");
        }
    }

    #[test]
    fn built_in_actions_use_human_labels_and_extensions_keep_their_names() {
        let mut journal = Journal::default();
        for name in [
            "search_text",
            "search_text",
            "write_file",
            "edit_file",
            "vendor_lookup",
        ] {
            journal.tool(name, json!({"path":"source.txt"}), json!({"status":"ok"}));
        }
        let view = text(&journal.load().project(72, 32));
        for label in [
            "Search text ×2 · ok",
            "Write file · ok",
            "Edit file · ok",
            "vendor_lookup · ok",
        ] {
            assert!(view.contains(label), "{label} missing\n{view}");
        }
    }

    #[test]
    fn missing_read_has_one_default_diagnostic_and_keeps_originals_in_its_details() {
        let mut journal = Journal::default();
        let path = "samples/missing.txt";
        let summary = format!("failed: path not found: {path}");
        let diagnostic = format!("path does not exist: {path}");
        journal.tool(
            "read_file",
            json!({"path":path}),
            json!({"status":"error","summary":summary,"model_content":diagnostic}),
        );
        let mut transcript = journal.load();
        let view = transcript.project(80, 20);
        let compact = text(&view);
        assert!(compact.contains("Read file · error") && compact.contains(&diagnostic));
        assert_eq!(1, compact.matches(path).count());
        assert!(!compact.contains(&summary));

        let key = transcript
            .detail_at(line(&view, "Read file · error"))
            .unwrap()
            .clone();
        assert!(transcript.toggle_detail(&key));
        let inspected = text(&transcript.project(80, 20));
        assert!(inspected.contains("Requested arguments · read_file"));
        assert!(inspected.contains(&summary) && inspected.contains(&diagnostic));
        let raw = &transcript.tools()[0];
        assert_eq!(diagnostic, raw.details);
        assert_eq!(summary, raw.summary);
    }

    #[test]
    fn older_page_can_extend_an_open_group_without_moving_the_reader_or_invalidating_its_key() {
        let mut journal = Journal::default();
        for index in 1..=8 {
            journal.read(7, "main", &format!("p{index}.txt"));
        }
        let latest = journal.0.split_off(6);
        let mut transcript = Transcript::default();
        transcript.replace_recent(HistoryPage {
            entries: latest,
            next_before: Some(7),
        });
        assert_eq!(
            Some(HistoryRequest::Older(7)),
            transcript.scroll(-20, 72, 3)
        );
        let partial = transcript.project(72, 3);
        let group = transcript
            .detail_at(line(&partial, "Read 5 files"))
            .unwrap()
            .clone();
        assert!(transcript.toggle_detail(&group));
        transcript.scroll(2, 72, 3);
        let before = text(&transcript.project(72, 3));
        assert!(before.starts_with("  Read file · p5.txt"), "{before}");
        let anchor = transcript.reading.as_ref().unwrap().anchor.clone();
        assert_eq!(AnchorLine::Body(0), anchor.line);

        transcript.apply_page(
            HistoryRequest::Older(7),
            HistoryPage {
                entries: journal.0,
                next_before: None,
            },
        );
        assert_eq!(before, text(&transcript.project(72, 3)));
        assert!(transcript.toggle_detail(&group));
        assert!(text(&transcript.project(72, 3)).starts_with("▸ Read 8 files · ok"));
        assert_eq!(&anchor, &transcript.reading.as_ref().unwrap().anchor);
        assert!(transcript.toggle_detail(&group));
        assert_eq!(before, text(&transcript.project(72, 3)));
    }

    #[test]
    fn live_result_joins_the_group_only_on_commit_and_reconnect_keeps_its_expansion() {
        let mut journal = Journal::default();
        journal.read(7, "main", "first.txt");
        journal.read(7, "main", "second.txt");
        let complete = journal.0.clone();
        let result = journal.0.pop().unwrap();
        let id = result.details["tool_call_id"].as_str().unwrap();
        let mut transcript = journal.load();
        transcript.live(4, Some(7), None, &json!({"type":"tool_end","id":id,"status":"ok","summary":"read second.txt","result":result.details["structured"]}));
        let provisional = text(&transcript.project(80, 20));
        assert!(provisional.contains("second.txt") && !provisional.contains("Read 2 files"));

        transcript.apply(result, Some(4));
        let compact = transcript.project(80, 20);
        let key = transcript
            .detail_at(line(&compact, "Read 2 files · ok"))
            .unwrap()
            .clone();
        assert!(transcript.toggle_detail(&key));
        assert!(text(&transcript.project(80, 20)).contains("Read file · second.txt"));

        transcript.replace_recent(HistoryPage {
            entries: complete,
            next_before: None,
        });
        let restored = text(&transcript.project(80, 20));
        assert!(
            restored.contains("Read file · first.txt")
                && restored.contains("Read file · second.txt")
        );
        assert_eq!(1, restored.matches("Read 2 files · ok").count());
    }

    #[test]
    fn repeated_global_toggles_follow_the_group_state_before_the_next_frame() {
        let mut journal = Journal::default();
        journal.read(7, "main", "first.txt");
        journal.read(7, "main", "second.txt");
        let mut transcript = journal.load();
        transcript.project(80, 20);
        assert!(transcript.toggle_details());
        assert!(!transcript.toggle_details());
        assert!(!text(&transcript.project(80, 20)).contains("second.txt"));
        assert!(transcript.toggle_details());
        assert!(text(&transcript.project(80, 20)).contains("Read file · second.txt"));
    }
}
