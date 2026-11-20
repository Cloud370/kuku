use kuku_client::{HistoryEntry, HistoryKind, RewindGroup, RewindGroupPage};
use ratatui::style::Style;
use ratatui::text::Span;

use crate::markup::{render_literal, wrap_bounded};
use crate::text::shorten;
use crate::theme::{emphasis, ACCENT, ERROR, MUTED};

use super::{Anchor, AnchorLine, Key, Reading, Row, Transcript, REWIND_GROUP_PAGE};

const MAX_GROUP_ROWS: usize = 768;
const MAX_GROUP_PAGES: usize = 4;
const MAX_GROUP_ENTRIES: usize = REWIND_GROUP_PAGE * MAX_GROUP_PAGES;
const GROUP_LABEL: &str = "Rewound content · read-only · excluded from model context";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RewindGroupRequest {
    pub(crate) rewind_event_id: u64,
    pub(crate) before: Option<u64>,
    pub(crate) epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RewindGroupAction {
    Toggle,
    Retry,
    More,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RewindGroupChoice {
    pub(crate) rewind_event_id: u64,
    pub(crate) target_event_id: u64,
    pub(crate) agent: String,
    pub(crate) label: String,
    pub(crate) summary: String,
    pub(crate) expanded: bool,
    pub(crate) can_load_more: bool,
    pub(crate) can_retry: bool,
}

#[derive(Debug, Clone)]
enum PageState {
    Idle,
    Loading(Option<u64>),
    Failed(Option<u64>, String),
}

#[derive(Debug, Clone)]
struct GroupEntry {
    event_id: u64,
    text: String,
}

#[derive(Debug, Clone)]
pub(super) struct RewindGroupState {
    pub(super) summary: RewindGroup,
    pub(super) key: Key,
    pub(super) expanded: bool,
    pub(super) rows: Vec<Row>,
    row_entries: Vec<Option<(u64, usize)>>,
    row_actions: Vec<Option<RewindGroupAction>>,
    entries: Vec<GroupEntry>,
    next_before: Option<u64>,
    loaded_first: bool,
    newer_entries_discarded: bool,
    truncated: bool,
    state: PageState,
}

impl RewindGroupState {
    pub(super) fn new(summary: RewindGroup) -> Self {
        Self {
            key: Key::RewindGroup(summary.rewind_event_id),
            summary,
            expanded: false,
            rows: Vec::new(),
            row_entries: Vec::new(),
            row_actions: Vec::new(),
            entries: Vec::new(),
            next_before: None,
            loaded_first: false,
            newer_entries_discarded: false,
            truncated: false,
            state: PageState::Idle,
        }
    }

    pub(super) fn update_summary(&mut self, summary: RewindGroup) {
        self.summary = summary;
        self.rows.clear();
        self.row_entries.clear();
    }

    pub(super) fn position(&self) -> u64 {
        self.summary.target_event_id
    }

    pub(super) fn render(&mut self, width: usize) {
        self.rows.clear();
        self.row_entries.clear();
        self.row_actions.clear();
        self.truncated = false;
        let mut title = GROUP_LABEL.to_owned();
        if let Some(turn) = self.summary.turn_number {
            title.push_str(&format!(" · Turn {turn}"));
        }
        title.push_str(&format!(" · {} entries", self.summary.entry_count));
        self.push_text(
            &title,
            width,
            emphasis(ACCENT),
            Some(RewindGroupAction::Toggle),
            0,
        );
        if !self.expanded {
            return;
        }
        if self.newer_entries_discarded {
            self.push_text(
                "Newer entries are not retained in this bounded view · collapse and reopen to restart",
                width,
                Style::default().fg(MUTED),
                None,
                2,
            );
        }
        match self.state.clone() {
            PageState::Loading(_) => {
                self.push_text(
                    "Loading rewound content…",
                    width,
                    Style::default().fg(MUTED),
                    None,
                    2,
                );
            }
            PageState::Failed(_, error) => {
                self.push_text(
                    &format!(
                        "Could not load rewound content · {} · click or use Details to retry",
                        shorten(&error, 120)
                    ),
                    width,
                    Style::default().fg(ERROR),
                    Some(RewindGroupAction::Retry),
                    2,
                );
            }
            PageState::Idle => {}
        }
        let entries = std::mem::take(&mut self.entries);
        for entry in &entries {
            self.push_entry(&entry.text, width, entry.event_id);
        }
        self.entries = entries;
        if self.entries.is_empty()
            && self.loaded_first
            && !self.is_loading()
            && self.error().is_none()
        {
            self.push_text(
                "No displayable entries.",
                width,
                Style::default().fg(MUTED),
                None,
                2,
            );
        }
        if self.next_before.is_some() {
            self.push_text(
                "Load older rewound content · click or use Details to load earlier",
                width,
                Style::default().fg(ACCENT),
                Some(RewindGroupAction::More),
                2,
            );
        }
        if self.truncated {
            if let Some(row) = self.rows.last_mut() {
                let inset = 2.min(width) as u16;
                let mut line = wrap_bounded(
                    render_literal("… Display limit reached", Style::default().fg(MUTED)),
                    width.saturating_sub(usize::from(inset)),
                    1,
                )
                .into_iter()
                .next()
                .unwrap_or_default();
                if inset > 0 && !line.spans.is_empty() {
                    line.spans
                        .insert(0, Span::raw(" ".repeat(usize::from(inset))));
                }
                row.line = line;
                row.inset = inset;
                row.links.clear();
                if let Some(entry) = self.row_entries.last_mut() {
                    *entry = None;
                }
                if let Some(action) = self.row_actions.last_mut() {
                    *action = self.next_before.map(|_| RewindGroupAction::More);
                }
            }
        }
    }

    pub(super) fn action_at(&self, row: usize) -> Option<RewindGroupAction> {
        self.row_actions.get(row).copied().flatten()
    }

    pub(super) fn cancel_pending(&mut self) {
        if let PageState::Loading(before) = self.state.clone() {
            self.state =
                PageState::Failed(before, "History refreshed before this page loaded".into());
            self.rows.clear();
        }
    }

    pub(super) fn entry_at(&self, row: usize) -> Option<(u64, usize)> {
        self.row_entries.get(row).copied().flatten()
    }

    pub(super) fn row_for_entry(&self, event_id: u64, line: usize) -> Option<usize> {
        self.row_entries
            .iter()
            .position(|entry| *entry == Some((event_id, line)))
    }

    pub(super) fn choice(&self) -> RewindGroupChoice {
        let turn = self
            .summary
            .turn_number
            .map_or_else(String::new, |turn| format!("Turn {turn} · "));
        let status = match &self.state {
            PageState::Loading(_) => " · loading".to_owned(),
            PageState::Failed(_, _) => " · retry available".to_owned(),
            PageState::Idle if self.next_before.is_some() => {
                " · older entries available".to_owned()
            }
            PageState::Idle => String::new(),
        };
        RewindGroupChoice {
            rewind_event_id: self.summary.rewind_event_id,
            target_event_id: self.summary.target_event_id,
            agent: self.summary.agent.clone(),
            label: GROUP_LABEL.to_owned(),
            summary: format!("{turn}{} entries{status}", self.summary.entry_count),
            expanded: self.expanded,
            can_load_more: self.expanded
                && self.next_before.is_some()
                && matches!(&self.state, PageState::Idle),
            can_retry: self.expanded && matches!(&self.state, PageState::Failed(..)),
        }
    }

    pub(super) fn toggle(&mut self, epoch: u64) -> Option<RewindGroupRequest> {
        if self.expanded {
            self.expanded = false;
            self.state = PageState::Idle;
            if self.newer_entries_discarded {
                self.entries.clear();
                self.next_before = None;
                self.loaded_first = false;
                self.newer_entries_discarded = false;
            }
            None
        } else {
            self.expanded = true;
            if !self.loaded_first {
                self.begin(None, epoch)
            } else {
                None
            }
        }
    }

    pub(super) fn more(&mut self, epoch: u64) -> Option<RewindGroupRequest> {
        match &self.state {
            PageState::Failed(before, _) => self.begin(*before, epoch),
            PageState::Idle => self
                .next_before
                .and_then(|before| self.begin(Some(before), epoch)),
            PageState::Loading(_) => None,
        }
    }

    fn begin(&mut self, before: Option<u64>, epoch: u64) -> Option<RewindGroupRequest> {
        if matches!(&self.state, PageState::Loading(_))
            || (before.is_none() && self.loaded_first)
            || before.is_some_and(|before| self.next_before != Some(before))
        {
            return None;
        }
        self.state = PageState::Loading(before);
        Some(RewindGroupRequest {
            rewind_event_id: self.summary.rewind_event_id,
            before,
            epoch,
        })
    }

    pub(super) fn apply(
        &mut self,
        request: &RewindGroupRequest,
        result: Result<RewindGroupPage, String>,
    ) -> bool {
        if self.summary.rewind_event_id != request.rewind_event_id
            || !matches!(&self.state, PageState::Loading(before) if *before == request.before)
        {
            return false;
        }
        match result {
            Ok(page) => {
                if request.before.is_none() {
                    self.loaded_first = true;
                }
                for entry in page.entries {
                    if let Some(text) = entry_text(&entry) {
                        if !self.entries.iter().any(|old| old.event_id == entry.id) {
                            self.entries.push(GroupEntry {
                                event_id: entry.id,
                                text,
                            });
                        }
                    }
                }
                self.entries.sort_by_key(|entry| entry.event_id);
                if self.entries.len() > MAX_GROUP_ENTRIES {
                    self.entries.truncate(MAX_GROUP_ENTRIES);
                    self.newer_entries_discarded = true;
                }
                self.next_before = page.next_before;
                self.state = PageState::Idle;
            }
            Err(error) => self.state = PageState::Failed(request.before, error),
        }
        self.rows.clear();
        true
    }

    fn push_text(
        &mut self,
        text: &str,
        width: usize,
        style: Style,
        action: Option<RewindGroupAction>,
        inset: u16,
    ) {
        self.push_rows(text, width, style, action, inset, None);
    }

    fn push_entry(&mut self, text: &str, width: usize, event_id: u64) {
        self.push_rows(
            text,
            width,
            Style::default().fg(MUTED),
            None,
            2,
            Some(event_id),
        );
    }

    fn push_rows(
        &mut self,
        text: &str,
        width: usize,
        style: Style,
        action: Option<RewindGroupAction>,
        inset: u16,
        event_id: Option<u64>,
    ) {
        if self.rows.len() >= MAX_GROUP_ROWS {
            self.truncated = true;
            return;
        }
        let first = self.rows.len();
        let remaining = MAX_GROUP_ROWS.saturating_sub(first);
        let inset = usize::from(inset).min(width);
        let mut lines = wrap_bounded(
            render_literal(text, style),
            width.saturating_sub(inset),
            remaining,
        );
        if lines.len() > remaining {
            self.truncated = true;
            lines.truncate(remaining);
        }
        self.rows.extend(lines.into_iter().map(|mut line| {
            if inset > 0 && !line.spans.is_empty() {
                line.spans.insert(0, Span::raw(" ".repeat(inset)));
            }
            Row {
                line,
                inset: inset as u16,
                links: Vec::new(),
            }
        }));
        self.row_entries.extend(
            (first..self.rows.len()).map(|line| event_id.map(|event_id| (event_id, line - first))),
        );
        self.row_actions.resize(self.rows.len(), None);
        if let Some(action) = action {
            self.row_actions[first..].fill(Some(action));
        }
    }

    fn is_loading(&self) -> bool {
        matches!(&self.state, PageState::Loading(_))
    }

    fn error(&self) -> Option<&str> {
        match &self.state {
            PageState::Failed(_, error) => Some(error),
            PageState::Idle | PageState::Loading(_) => None,
        }
    }
}

fn entry_text(entry: &HistoryEntry) -> Option<String> {
    let text = entry.text.as_deref().unwrap_or_default();
    Some(match entry.kind {
        HistoryKind::User => format!("User · {text}"),
        HistoryKind::Assistant => format!("Assistant · {text}"),
        HistoryKind::ToolCall => format!(
            "Tool call · {}",
            entry
                .details
                .get("tool")
                .and_then(|value| value.as_str())
                .unwrap_or("Tool")
        ),
        HistoryKind::ToolResult => format!(
            "Tool result · {}",
            entry
                .details
                .get("summary")
                .and_then(|value| value.as_str())
                .unwrap_or(text)
        ),
        HistoryKind::Question => format!("Status · Question · {text}"),
        HistoryKind::Permission => format!("Status · Permission · {text}"),
        HistoryKind::QuestionReply => format!("Status · Answer · {text}"),
        HistoryKind::PermissionReply => format!("Status · Decision · {text}"),
        HistoryKind::TurnSummary => "Status · Turn summary".to_owned(),
        HistoryKind::Status => format!("Status · {text}"),
        HistoryKind::Error => format!("Status · Error · {text}"),
        HistoryKind::Retry => format!("Status · Retry · {text}"),
        HistoryKind::Other => return None,
    })
}

impl Transcript {
    pub(super) fn reset_pending_rewind_group_pages(&mut self) {
        for group in &mut self.recent.rewind_groups {
            group.cancel_pending();
        }
        if let Some(reading) = &mut self.reading {
            for group in &mut reading.window.rewind_groups {
                group.cancel_pending();
            }
        }
    }

    fn sync_rewind_group_state(&mut self, rewind_event_id: u64) {
        let state = self
            .reading
            .as_ref()
            .and_then(|reading| {
                reading
                    .window
                    .rewind_groups
                    .iter()
                    .find(|group| group.summary.rewind_event_id == rewind_event_id)
            })
            .or_else(|| {
                self.recent
                    .rewind_groups
                    .iter()
                    .find(|group| group.summary.rewind_event_id == rewind_event_id)
            })
            .cloned();
        let Some(state) = state else { return };
        if let Some(group) = self
            .recent
            .rewind_groups
            .iter_mut()
            .find(|group| group.summary.rewind_event_id == rewind_event_id)
        {
            *group = state.clone();
        }
        if let Some(group) = self.reading.as_mut().and_then(|reading| {
            reading
                .window
                .rewind_groups
                .iter_mut()
                .find(|group| group.summary.rewind_event_id == rewind_event_id)
        }) {
            *group = state;
        }
    }

    fn anchor_expanded_group(&mut self, rewind_event_id: u64, was_following: bool) {
        if !was_following
            || !self
                .recent
                .rewind_groups
                .iter()
                .any(|group| group.summary.rewind_event_id == rewind_event_id && group.expanded)
        {
            return;
        }
        let window = self.recent.clone();
        self.reading = Some(Reading {
            window,
            anchor: Anchor {
                key: Key::RewindGroup(rewind_event_id),
                line: AnchorLine::Group(0),
            },
            newer: std::collections::VecDeque::new(),
            unseen: false,
            manual: true,
        });
    }

    pub(crate) fn rewind_group_choices(&self) -> Vec<RewindGroupChoice> {
        let window = self
            .reading
            .as_ref()
            .map_or(&self.recent, |reading| &reading.window);
        window
            .rewind_groups
            .iter()
            .map(RewindGroupState::choice)
            .collect()
    }

    pub(crate) fn toggle_rewind_group(
        &mut self,
        rewind_event_id: u64,
    ) -> Option<RewindGroupRequest> {
        let epoch = self.readonly_epoch;
        let was_following = self.reading.is_none();
        let request = {
            let window = self
                .reading
                .as_mut()
                .map_or(&mut self.recent, |reading| &mut reading.window);
            window
                .rewind_groups
                .iter_mut()
                .find(|group| group.summary.rewind_event_id == rewind_event_id)?
                .toggle(epoch)
        };
        self.sync_rewind_group_state(rewind_event_id);
        self.anchor_expanded_group(rewind_event_id, was_following);
        request
    }

    pub(crate) fn load_more_rewind_group(
        &mut self,
        rewind_event_id: u64,
    ) -> Option<RewindGroupRequest> {
        let epoch = self.readonly_epoch;
        let was_following = self.reading.is_none();
        let request = {
            let window = self
                .reading
                .as_mut()
                .map_or(&mut self.recent, |reading| &mut reading.window);
            window
                .rewind_groups
                .iter_mut()
                .find(|group| group.summary.rewind_event_id == rewind_event_id)?
                .more(epoch)
        };
        self.sync_rewind_group_state(rewind_event_id);
        self.anchor_expanded_group(rewind_event_id, was_following);
        request
    }

    pub(crate) fn retry_rewind_group(
        &mut self,
        rewind_event_id: u64,
    ) -> Option<RewindGroupRequest> {
        self.load_more_rewind_group(rewind_event_id)
    }

    pub(crate) fn apply_rewind_group_page(
        &mut self,
        request: &RewindGroupRequest,
        result: Result<RewindGroupPage, String>,
    ) -> bool {
        if request.epoch != self.readonly_epoch {
            return false;
        }
        let mut updated = None;
        if let Some(group) = self.reading.as_mut().and_then(|reading| {
            reading
                .window
                .rewind_groups
                .iter_mut()
                .find(|group| group.summary.rewind_event_id == request.rewind_event_id)
        }) {
            if group.apply(request, result.clone()) {
                updated = Some(group.clone());
            }
        }
        if updated.is_none() {
            if let Some(group) = self
                .recent
                .rewind_groups
                .iter_mut()
                .find(|group| group.summary.rewind_event_id == request.rewind_event_id)
            {
                if group.apply(request, result) {
                    updated = Some(group.clone());
                }
            }
        }
        let Some(updated) = updated else { return false };
        if let Some(group) = self
            .recent
            .rewind_groups
            .iter_mut()
            .find(|group| group.summary.rewind_event_id == request.rewind_event_id)
        {
            *group = updated.clone();
        }
        if let Some(group) = self.reading.as_mut().and_then(|reading| {
            reading
                .window
                .rewind_groups
                .iter_mut()
                .find(|group| group.summary.rewind_event_id == request.rewind_event_id)
        }) {
            *group = updated;
        }
        true
    }

    pub(crate) fn rewind_group_action_at(&self, row: usize) -> Option<(u64, RewindGroupAction)> {
        self.visible_rewind_actions
            .iter()
            .find(|(visible_row, _, _)| *visible_row == row)
            .map(|(_, group_id, action)| (*group_id, *action))
    }
}
