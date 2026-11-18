use std::collections::{HashMap, HashSet, VecDeque};

use kuku_client::Receipt;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::app::Activity;
use crate::markup::{render_literal, render_markdown};
use crate::text::{cap, shorten, single_line};
use crate::theme::{
    emphasis, CardFrame, ACCENT, ACTIVITY_EXCERPT_CELLS, ERROR, MODEL, MUTED, SUCCESS, WARNING,
};

use super::{
    body_inset, capped, render_rows, Body, Key, Row, ToolDetails, Transcript, MAX_RECORDS,
};

const MAX_ACTIVITY_BYTES: usize = 4 * 1024;
const MAX_SECTION_ROWS: usize = 4;
const MAX_CHOICE_CELLS: usize = 120;

impl Transcript {
    pub(crate) fn sync_agent_activity<'a>(
        &mut self,
        revision: u64,
        agents: impl IntoIterator<Item = (&'a str, Activity, Option<&'a str>)>,
    ) {
        self.agent_cards.sync(revision, agents);
    }

    pub(super) fn toggle_agent_record(&mut self, key: &Key) -> bool {
        if !self.recent.contains(key)
            && !self
                .reading
                .as_ref()
                .is_some_and(|reading| reading.window.contains(key))
        {
            return false;
        }
        self.agent_cards.toggle(key);
        true
    }

    pub(super) fn link_recent_agent_cards(&mut self) {
        self.agent_cards.latest.clear();
        if self.agent_cards.activities.is_empty() {
            return;
        }
        let mut newest = HashMap::new();
        for record in &self.recent.records {
            let Body::Tool(tool) = &record.body else {
                continue;
            };
            let (Some(position), Some(card)) = (record.position, tool.agent_card()) else {
                continue;
            };
            if let Some(receipt) = card.receipt_cursor {
                let latest = newest.entry(card.address).or_insert((receipt, position));
                if receipt > latest.0 {
                    *latest = (receipt, position);
                }
            }
        }
        self.agent_cards
            .latest
            .extend(newest.values().map(|&(_, position)| position));
    }
}

#[derive(Debug, Default)]
pub(super) struct AgentCards {
    expanded: VecDeque<Key>,
    latest: HashSet<u64>,
    activities: HashMap<String, AgentFacts>,
    source_revision: Option<u64>,
    paint_revision: u64,
}

impl AgentCards {
    pub(super) fn is_open(&self, key: &Key) -> bool {
        self.expanded.contains(key)
    }

    pub(super) fn set_details<'a>(&mut self, open: bool, keys: impl Iterator<Item = &'a Key>) {
        if !open {
            self.expanded.clear();
            return;
        }
        for key in keys {
            if !self.is_open(key) {
                remember_expanded(&mut self.expanded, key);
            }
        }
    }

    pub(super) fn display(
        &self,
        key: &Key,
        position: Option<u64>,
        address: &str,
    ) -> (ToolDetails, Option<&AgentFacts>) {
        let details = if self.expanded.contains(key) {
            ToolDetails::Expanded
        } else {
            ToolDetails::Collapsed
        };
        let facts = position
            .filter(|position| self.latest.contains(position))
            .and_then(|_| self.activities.get(address));
        (details, facts)
    }

    fn sync<'a>(
        &mut self,
        revision: u64,
        agents: impl IntoIterator<Item = (&'a str, Activity, Option<&'a str>)>,
    ) {
        if self.source_revision == Some(revision) {
            return;
        }
        self.source_revision = Some(revision);
        for (address, status, text) in agents {
            let preview = cap(text.unwrap_or_default(), MAX_ACTIVITY_BYTES);
            if let Some(previous) = self.activities.get_mut(address) {
                previous.source_revision = revision;
                if previous.status == status && previous.preview == preview {
                    continue;
                }
                self.paint_revision += 1;
                previous.status = status;
                previous.preview.clear();
                previous.preview.push_str(preview);
                previous.paint_revision = self.paint_revision;
            } else {
                self.paint_revision += 1;
                self.activities.insert(
                    address.to_owned(),
                    AgentFacts {
                        status,
                        preview: preview.to_owned(),
                        source_revision: revision,
                        paint_revision: self.paint_revision,
                    },
                );
            }
        }
        self.activities
            .retain(|_, agent| agent.source_revision == revision);
    }

    fn toggle(&mut self, key: &Key) {
        if let Some(index) = self.expanded.iter().position(|item| item == key) {
            self.expanded.remove(index);
        } else {
            remember_expanded(&mut self.expanded, key);
        }
    }
}

fn remember_expanded(expanded: &mut VecDeque<Key>, key: &Key) {
    if expanded.len() == MAX_RECORDS {
        expanded.pop_front();
    }
    expanded.push_back(key.clone());
}

#[derive(Debug)]
pub(super) struct AgentFacts {
    status: Activity,
    preview: String,
    source_revision: u64,
    pub(super) paint_revision: u64,
}

#[derive(Debug, Clone)]
pub(super) struct AgentRequest {
    kind: AgentAction,
    target: String,
    task: String,
}

impl AgentRequest {
    pub(super) fn from_call(tool: &str, args: &Value) -> Option<Self> {
        let (kind, target, task) = match tool {
            "create_agent" => (AgentAction::Create, "address", "instructions"),
            "send_message" => (AgentAction::Send, "to", "message"),
            _ => return None,
        };
        let target = args[target].as_str()?.trim();
        if target.is_empty() {
            return None;
        }
        Some(Self {
            kind,
            target: capped(Some(target), MAX_ACTIVITY_BYTES),
            task: capped(args[task].as_str(), MAX_ACTIVITY_BYTES),
        })
    }

    pub(super) fn card<'a>(
        &'a self,
        receipt: Option<&'a Receipt>,
        tool_status: &'a str,
        error: &'a str,
    ) -> AgentCard<'a> {
        let (address, action) = match receipt {
            Some(receipt) => (receipt.agent.as_str(), self.kind.confirmed()),
            None => (self.target.as_str(), tool_status),
        };
        AgentCard {
            address,
            kind: self.kind,
            action,
            task: &self.task,
            task_label: self.kind.task_label(),
            error,
            receipt_cursor: receipt.map(|receipt| receipt.event_id),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum AgentAction {
    Create,
    Send,
}

impl AgentAction {
    fn confirmed(self) -> &'static str {
        match self {
            Self::Create => "Created",
            Self::Send => "Input delivered",
        }
    }

    fn task_label(self) -> &'static str {
        match self {
            Self::Create => "Requested instructions",
            Self::Send => "Requested task",
        }
    }
}

pub(super) struct AgentCard<'a> {
    pub(super) address: &'a str,
    pub(super) receipt_cursor: Option<u64>,
    kind: AgentAction,
    action: &'a str,
    task: &'a str,
    task_label: &'static str,
    error: &'a str,
}

impl AgentCard<'_> {
    pub(super) fn created(&self) -> bool {
        matches!(self.kind, AgentAction::Create) && self.receipt_cursor.is_some()
    }

    pub(super) fn delivered(&self) -> bool {
        matches!(self.kind, AgentAction::Send) && self.receipt_cursor.is_some()
    }

    pub(super) fn delegation_header(
        &self,
        width: usize,
        open: bool,
        facts: Option<&AgentFacts>,
        checks: usize,
    ) -> Vec<Row> {
        let frame = CardFrame::agent(width);
        let title = format!("Agent {}", single_line(self.address));
        let mut rows = vec![Row::plain(frame.title(&title, MODEL))];
        let (label, color) = facts.map_or(("Delivered", Style::new().fg(MUTED)), |facts| {
            (facts.status.label(), activity_style(facts.status))
        });
        let space = frame.content_width();
        let mut body = Line::from(Span::styled(shorten(label, space), color));
        let reply = facts.and_then(|facts| first_content_line(&facts.preview));
        let (caption, excerpt) = match reply {
            Some(text) if facts.is_some_and(|facts| facts.status == Activity::Failed) => {
                ("Error", text)
            }
            Some(text) => ("Reply", text),
            None => ("Task", first_content_line(self.task).unwrap_or_default()),
        };
        if !excerpt.is_empty() && space >= body.width() + caption.len() + 7 {
            body.push_span(Span::styled(
                format!("   {caption}  "),
                Style::new().fg(MUTED),
            ));
            let excerpt = excerpt
                .trim_start_matches('#')
                .strip_prefix(' ')
                .unwrap_or(excerpt);
            body.push_span(Span::styled(
                shorten(
                    &single_line(excerpt),
                    (space - body.width()).min(ACTIVITY_EXCERPT_CELLS),
                ),
                Style::default(),
            ));
        }
        rows.push(Row::plain(frame.line(body)));
        let marker = if open { "▾" } else { "▸" };
        let timeline = if checks == 0 {
            format!("{marker} Timeline")
        } else {
            format!(
                "{marker} Timeline · {} calls · {checks} status {}",
                checks + 2,
                if checks == 1 { "check" } else { "checks" }
            )
        };
        rows.push(Row::plain(frame.foot(&timeline)));
        if !open {
            rows.push(Row::plain(Line::default()));
        }
        rows
    }

    pub(super) fn choice_summary(&self) -> String {
        let task = self.task.trim();
        let text = if task.is_empty() { self.action } else { task };
        shorten(&single_line(text), MAX_CHOICE_CELLS)
    }

    pub(super) fn render(
        &self,
        width: usize,
        details: ToolDetails,
        facts: Option<&AgentFacts>,
        in_timeline: bool,
    ) -> Vec<Row> {
        let frame = CardFrame::agent(width);
        let width = if in_timeline {
            width
        } else {
            frame.content_width()
        };
        let inset = body_inset(width);
        let body_inset = inset + body_inset(width.saturating_sub(usize::from(inset)));
        let expanded = details == ToolDetails::Expanded;
        let header = self.header(width.saturating_sub(usize::from(inset)), facts, in_timeline);
        let mut rows: Vec<_> = render_rows(header, width, 3, inset).collect();
        if let Some(row) = rows.first_mut().filter(|_| in_timeline && inset != 0) {
            let marker = if expanded { "▾ " } else { "▸ " };
            row.line.spans[0] = Span::styled(marker, emphasis(MODEL));
        }
        let preview = facts.map_or("", |facts| facts.preview.as_str());
        if !in_timeline || !expanded {
            let (label, text) = if !self.error.is_empty() {
                ("Error", self.error)
            } else if !preview.is_empty() {
                let label = if facts.is_some_and(|facts| facts.status == Activity::Failed) {
                    "Error"
                } else {
                    "Reply"
                };
                (label, preview)
            } else if matches!(self.kind, AgentAction::Send) {
                ("Task", self.task)
            } else {
                ("", "")
            };
            if let Some(excerpt) = first_content_line(text) {
                let mut line = Line::from(Span::styled(format!("{label}  "), emphasis(MUTED)));
                let remaining = width.saturating_sub(usize::from(body_inset) + line.width());
                line.push_span(Span::styled(
                    shorten(&single_line(excerpt), remaining.min(ACTIVITY_EXCERPT_CELLS)),
                    Style::new().fg(MUTED),
                ));
                rows.extend(render_rows(vec![line], width, 1, body_inset));
            }
        }
        let summary_rows = rows.len();
        if expanded {
            render_section(&mut rows, self.task_label, self.task, width, body_inset);
            let label = match facts.map(|facts| facts.status) {
                Some(Activity::Streaming) => "Live Agent reply",
                Some(Activity::Failed) => "Latest Agent error",
                _ => "Latest Agent reply",
            };
            render_section(&mut rows, label, preview, width, body_inset);
            if !self.error.is_empty() {
                rows.extend(render_rows(
                    render_literal(self.error, Style::new().fg(ERROR)),
                    width,
                    MAX_SECTION_ROWS,
                    body_inset,
                ));
            }
        }
        if !in_timeline {
            let mut framed = vec![Row::plain(
                frame.title(&format!("Agent {}", single_line(self.address)), MODEL),
            )];
            let details = rows.split_off(summary_rows);
            framed.extend(rows.into_iter().map(|row| Row::plain(frame.line(row.line))));
            let label = if expanded {
                "▾ Details"
            } else {
                "▸ Details"
            };
            if details.is_empty() {
                framed.push(Row::plain(frame.foot(label)));
            } else {
                framed.push(Row::plain(frame.divider(label)));
                framed.extend(
                    details
                        .into_iter()
                        .map(|row| Row::plain(frame.line(row.line))),
                );
                if frame.bordered() {
                    framed.push(Row::plain(frame.border('╰', '╯')));
                }
            }
            rows = framed;
        }
        rows.push(Row::plain(Line::default()));
        rows
    }

    fn header(
        &self,
        width: usize,
        facts: Option<&AgentFacts>,
        include_address: bool,
    ) -> Vec<Line<'static>> {
        let title = format!("Agent {}", single_line(self.address));
        let action = single_line(self.action);
        let style = Style::new().fg(MUTED);
        let mut inline = Line::default();
        if include_address {
            inline.push_span(Span::styled(format!("{title} · "), emphasis(MODEL)));
        }
        inline.push_span(Span::styled(action.clone(), style));
        if let Some(facts) = facts {
            inline.push_span(Span::styled(
                format!(" · Now {}", facts.status.label()),
                activity_style(facts.status),
            ));
        }
        if inline.width() <= width {
            return vec![inline];
        }
        let mut lines = Vec::new();
        if include_address {
            lines.push(Line::styled(shorten(&title, width), emphasis(MODEL)));
        }
        lines.push(Line::styled(shorten(&action, width), style));
        if let Some(facts) = facts {
            lines.push(Line::styled(
                shorten(facts.status.label(), width),
                activity_style(facts.status),
            ));
        }
        lines
    }
}

fn first_content_line(text: &str) -> Option<&str> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("```") && !line.starts_with("~~~"))
}

fn activity_style(activity: Activity) -> Style {
    let color = match activity {
        Activity::Finished => SUCCESS,
        Activity::Failed | Activity::Interrupted => ERROR,
        Activity::Waiting | Activity::Stopping => WARNING,
        _ => ACCENT,
    };
    emphasis(color)
}

fn render_section(rows: &mut Vec<Row>, label: &'static str, text: &str, width: usize, inset: u16) {
    if text.trim().is_empty() {
        return;
    }
    let mut source = vec![Line::styled(label, emphasis(MUTED))];
    source.extend(render_markdown(
        text,
        width.saturating_sub(usize::from(inset)),
        MAX_SECTION_ROWS,
    ));
    rows.extend(render_rows(source, width, MAX_SECTION_ROWS, inset));
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use crate::app::Activity;

    use super::super::{DetailKey, Transcript, TranscriptView};

    fn apply(transcript: &mut Transcript, id: u64, mut record: Value) {
        record["id"] = json!(id);
        record["agent"] = json!("main");
        record["turn"] = json!(7);
        record["turn_number"] = json!(1);
        transcript.apply(serde_json::from_value(record).unwrap(), None);
    }

    fn settled_agent_calls() -> Transcript {
        let mut transcript = Transcript::default();
        apply(
            &mut transcript,
            1,
            json!({
                "type": "tool_call", "tool": "create_agent", "tool_call_id": "create",
                "args": {"address": "proposed", "instructions": "Inspect documentation\nUse precise citations"}
            }),
        );
        apply(
            &mut transcript,
            2,
            json!({
                "type": "tool_result", "tool_call_id": "create", "status": "ok",
                "summary": "session operation committed",
                "structured": {"agent": "reviewer", "command_id": "tool:main:7:create", "event_id": 1}
            }),
        );
        apply(
            &mut transcript,
            3,
            json!({
                "type": "tool_call", "tool": "send_message", "tool_call_id": "send",
                "args": {"to": "reviewer", "message": "Verify the new CLI\nList failing cases"}
            }),
        );
        apply(
            &mut transcript,
            4,
            json!({
                "type": "tool_result", "tool_call_id": "send", "status": "ok",
                "summary": "session operation committed",
                "structured": {"agent": "reviewer", "command_id": "tool:main:7:send", "event_id": 3}
            }),
        );
        transcript
    }

    fn agent_choices(transcript: &Transcript) -> Vec<(DetailKey, String, String)> {
        transcript
            .visible_detail_choices()
            .into_iter()
            .filter(|(_, label, _)| label.starts_with("Agent "))
            .collect()
    }

    fn toggle_row(transcript: &mut Transcript, row: usize) -> bool {
        transcript
            .detail_at(row)
            .cloned()
            .is_some_and(|key| transcript.toggle_detail(&key))
    }

    fn lines(view: &TranscriptView) -> Vec<String> {
        view.lines.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn collapsed_reply_skips_fences_and_keeps_the_first_content_line() {
        assert_eq!(
            super::first_content_line("\n```powershell\nCaption : Windows 11\n```"),
            Some("Caption : Windows 11")
        );
        assert_eq!(super::first_content_line("~~~text\n~~~"), None);
    }

    #[test]
    fn receipts_link_collapsed_cards_without_asserting_that_work_finished() {
        let mut transcript = settled_agent_calls();
        apply(
            &mut transcript,
            5,
            json!({
                "type":"tool_call", "tool":"run_command", "tool_call_id":"command", "args":{"command":"verify-fixtures","brief":"Verify fixtures"}
            }),
        );
        let view = transcript.project(90, 24);
        let plain = lines(&view).join("\n");
        assert!(
            plain.contains("╭─ Agent reviewer") && plain.matches("Agent reviewer").count() == 1
        );
        assert!(plain.contains("Verify the new CLI") && !plain.contains("List failing cases"));
        assert!(plain.contains("Verify fixtures") && !plain.contains("verify-fixtures"));
        assert!(
            !plain.contains("session operation committed")
                && !plain.contains("proposed")
                && !plain.contains("Finished")
        );
        assert_eq!(Some("Ctrl+O show · /details"), view.tool_hint.as_deref());
        assert_eq!(None, transcript.detail_at(0));
        let choices = agent_choices(&transcript);
        assert_eq!(1, choices.len());
        assert_eq!("Verify the new CLI List failing cases", choices[0].2);

        assert!(transcript.toggle_details());
        let expanded = transcript.project(90, 24);
        let body = lines(&expanded).join("\n");
        assert!(body.contains("Requested task") && body.contains("List failing cases"));
        assert!(body.contains("Requested instructions") && body.contains("Use precise citations"));
        assert!(body.contains("Verify fixtures"));
        assert_eq!(
            Some("Ctrl+O hide · /details"),
            expanded.tool_hint.as_deref()
        );

        assert!(!transcript.toggle_details());
        let collapsed = transcript.project(90, 24);
        assert_eq!(
            Some("Ctrl+O show · /details"),
            collapsed.tool_hint.as_deref()
        );
        let body = lines(&collapsed).join("\n");
        assert!(
            body.contains("Verify fixtures")
                && !body.contains("Requested task")
                && !body.contains("verify-fixtures")
        );
        assert!(transcript.toggle_details());
        let body = lines(&transcript.project(90, 24)).join("\n");
        assert!(body.contains("Requested task") && body.contains("Verify fixtures"));
    }

    #[test]
    fn a_command_and_a_delegation_keep_separate_frames_and_a_visible_details_control() {
        let mut transcript = Transcript::default();
        let command = |id: &str| {
            json!({
                "type": "tool_call", "tool": "run_command", "tool_call_id": id,
                "args": {"command": "example", "brief": id}
            })
        };
        apply(
            &mut transcript,
            1,
            json!({"type":"assistant","text":"before paragraph\n\nbefore paragraph\n\nbefore paragraph\n\nbefore paragraph"}),
        );
        apply(&mut transcript, 2, command("First check"));
        apply(
            &mut transcript,
            3,
            json!({
                "type":"tool_call","tool":"send_message","tool_call_id":"assign",
                "args":{"to":"reviewer","message":"Review the code\nCheck for missing tests"}
            }),
        );
        apply(&mut transcript, 4, command("Second check"));
        apply(
            &mut transcript,
            5,
            json!({"type":"assistant","text":"after paragraph\n\nafter paragraph\n\nafter paragraph\n\nafter paragraph"}),
        );
        let all = lines(&transcript.project(90, 100));
        let header = all
            .iter()
            .position(|line| line.contains("╭─ Agent reviewer"))
            .unwrap();
        let footer = all
            .iter()
            .position(|line| line.contains("▸ Details"))
            .unwrap();
        assert!(all[header - 1].trim().is_empty());
        assert!(all[footer + 1].trim().is_empty());
        assert!(all[footer + 2].contains("Second check"));

        let height = 8;
        transcript.scroll(-((all.len() - footer - 1) as isize), 90, height);
        let folded = lines(&transcript.project(90, height));
        assert!(folded[height - 1].contains("▸ Details"));
        let key = transcript.detail_at(height - 1).unwrap().clone();
        assert!(transcript.toggle_in_view(&key, 90, height));
        let open = lines(&transcript.project(90, height));
        let control = open
            .iter()
            .position(|line| line.contains("▾ Details"))
            .unwrap();
        assert!(open[control + 1].contains("Requested task"));
        assert!(open[control + 2].contains("Review the code"));
        assert_eq!(transcript.detail_at(control), Some(&key));
        assert!(transcript.toggle_in_view(&key, 90, height));
        let folded = lines(&transcript.project(90, height));
        assert!(folded[control].contains("▸ Details"));
    }

    #[test]
    fn picker_choice_uses_the_same_record_after_viewport_rows_change() {
        let mut transcript = settled_agent_calls();
        transcript.project(82, 25);
        let choices = agent_choices(&transcript);
        let earlier = &choices[0].0;
        transcript.project(82, 3);
        assert_eq!(1, agent_choices(&transcript).len());
        assert!(transcript.toggle_detail(earlier));
        let rendered = lines(&transcript.project(82, 25)).join("\n");
        assert!(rendered.contains("Requested instructions"));
        assert!(rendered.contains("Requested task"));
    }

    #[test]
    fn individual_card_hit_ranges_follow_the_rendered_rows_and_track_real_replies() {
        let mut transcript = settled_agent_calls();
        transcript.sync_agent_activity(
            5,
            [(
                "reviewer",
                Activity::Streaming,
                Some("# 检查\n\n| Item | Result |\n|---|---|\n| Build | 通过 |"),
            )],
        );
        transcript.sync_agent_activity(
            5,
            std::iter::once_with(|| panic!("unchanged source must stay lazy")),
        );
        let view = lines(&transcript.project(82, 27));
        let header = view
            .iter()
            .position(|line| line.contains("╭─ Agent reviewer"))
            .unwrap();
        assert!(view[header + 1].contains("Streaming reply"));
        assert!(toggle_row(&mut transcript, header + 1));
        let expanded = transcript.project(82, 27);
        let rendered = lines(&expanded).join("\n");
        assert!(
            rendered.contains("Requested instructions") && rendered.contains("Live Agent reply")
        );
        let second = lines(&expanded)
            .iter()
            .position(|line| line.contains("Input delivered"))
            .unwrap();
        assert!(toggle_row(&mut transcript, second));
        assert!(!lines(&transcript.project(82, 27))
            .join("\n")
            .contains("Live Agent reply"));
        assert!(toggle_row(&mut transcript, second));
        let expanded = transcript.project(60, 25);
        let rendered = lines(&expanded).join("\n");
        assert!(
            rendered.contains("Live Agent reply") && rendered.contains("检查"),
            "{rendered}"
        );
        assert!(expanded.lines.iter().all(|line| line.width() <= 60));
        transcript.sync_agent_activity(
            6,
            [("reviewer", Activity::Finished, Some("Completed checks"))],
        );
        let settled = lines(&transcript.project(60, 25)).join("\n");
        assert!(
            settled.contains("Input delivered · Now Finished")
                && settled.contains("Latest Agent reply")
                && settled.contains("Completed checks")
        );
        assert!(!settled.contains("通过"));
        transcript.sync_agent_activity(7, []);
        let cleared = lines(&transcript.project(60, 25)).join("\n");
        assert!(!cleared.contains("Completed checks") && !cleared.contains("Finished"));
        assert!(!toggle_row(&mut transcript, 99));
    }

    #[test]
    fn two_deliveries_to_the_same_agent_only_project_current_progress_on_the_latest_receipt() {
        let mut transcript = Transcript::default();
        for (id, call, task) in [
            (1, "first", "Verify the new CLI"),
            (2, "second", "Check the script"),
        ] {
            apply(
                &mut transcript,
                id,
                json!({
                    "type": "tool_call", "tool": "send_message", "tool_call_id": call,
                    "args": {"to": "reviewer", "message": task}
                }),
            );
        }
        for (id, call) in [(4, "second"), (6, "first")] {
            apply(
                &mut transcript,
                id,
                json!({
                    "type": "tool_result", "tool_call_id": call, "status": "ok",
                    "structured": {"agent": "reviewer", "command_id": format!("tool:main:7:{call}"), "event_id": id - 1}
                }),
            );
        }
        transcript.sync_agent_activity(
            8,
            [(
                "reviewer",
                Activity::Streaming,
                Some("Checking the CLI now"),
            )],
        );
        let view = lines(&transcript.project(84, 20));
        let sends: Vec<_> = view
            .iter()
            .filter(|row| row.contains("Input delivered"))
            .collect();
        assert_eq!(2, sends.len());
        assert!(sends[0].contains("Streaming reply"));
        assert!(!sends[1].contains("Streaming reply"));
        let rendered = view.join("\n");
        assert!(rendered.contains("Check the script"));
        assert_eq!(1, rendered.matches("Checking the CLI now").count());
    }
}
