use kuku_client::{AgentStatus, AgentSummary, Target};

use crate::keys::AgentDirection;
use crate::menu::{Choice, ChoiceKind, PickerKind};
use crate::text::single_line;

use super::{Activity, Address, App, ExecutionProgress, Modal, ServiceStatus};

#[derive(Debug, Default)]
pub(super) struct AgentNavigation {
    previous: Option<Address>,
    picker: Option<PickerState>,
}

#[derive(Debug, PartialEq, Eq)]
struct PickerState {
    id: u64,
    cursor: Option<u64>,
    related_revision: u64,
    activity: Activity,
    service: ServiceStatus,
}

impl App {
    pub(crate) fn select(&mut self, address: Address, exists: bool, title: String) {
        self.rewind_armed = false;
        if address.session != self.active.session {
            self.navigation = AgentNavigation::default();
            self.related_agents.reset_roster();
            // The current Session's panes share one durable roster. If we have
            // cached one of them, keep the most recent known roster available
            // while the new observation is starting (or unavailable).
            if let Some(snapshot) = self
                .cached
                .iter()
                .filter(|cached| cached.session == address.session)
                .filter_map(|cached| self.panes.get(cached)?.snapshot.as_deref())
                .filter(|snapshot| {
                    snapshot.session_id == address.session && snapshot.workspace == self.workspace
                })
                .max_by_key(|snapshot| snapshot.cursor)
            {
                self.related_agents.snapshot_roster(snapshot);
            }
        } else if address != self.active && exists && self.pane().exists {
            self.navigation.previous = Some(self.active.clone());
        }
        self.text_selection.clear();
        let pane = self.panes.entry(address.clone()).or_default();
        pane.exists |= exists;
        if pane.title.is_empty() {
            pane.title = title;
        }
        self.active = address;
        self.navigation.picker = None;
        self.cache_active_pane();
        self.notice = None;
        self.begin_observing();
    }

    pub(crate) fn is_subagent(&self) -> bool {
        self.active.agent != super::DEFAULT_AGENT
    }

    pub(crate) fn visible_recipient(&self) -> Option<&str> {
        (self.is_subagent()
            || self.previous_agent().is_some()
            || self.related_agents.roster().len() > 1
            || self.related_agents.iter().next().is_some())
        .then_some(self.active.agent.as_str())
    }

    pub(crate) fn agent_count(&self) -> usize {
        self.related_agents.roster().len()
    }

    pub(crate) fn previous_agent(&self) -> Option<&Address> {
        self.navigation.previous.as_ref().filter(|address| {
            address.session == self.active.session
                && address.agent != self.active.agent
                && self.panes.get(*address).is_some_and(|pane| pane.exists)
        })
    }

    pub(crate) fn cycle_agent(&mut self, direction: AgentDirection) {
        let Some(pane) = self.panes.get(&self.active).filter(|pane| pane.exists) else {
            return;
        };
        let size = self.related_agents.roster().len();
        if size < 2 {
            return;
        }
        let Some(current) = self
            .related_agents
            .roster()
            .position(|summary| summary.address == self.active.agent)
        else {
            return;
        };
        let next = match direction {
            AgentDirection::Next => (current + 1) % size,
            AgentDirection::Previous => (current + size - 1) % size,
        };
        let summary = self
            .related_agents
            .roster()
            .nth(next)
            .expect("roster index");
        let address = Address::new(self.active.session.clone(), summary.address.clone());
        self.select(address, true, pane.title.clone());
    }

    pub(crate) fn back_to_agent(&mut self) {
        if let Some(address) = self.previous_agent().cloned() {
            let title = self.pane().title.clone();
            self.select(address, true, title);
        } else {
            self.confirmation("No previous Agent in this Session");
        }
    }

    pub(crate) fn select_agent(&mut self, target: Target) {
        if target.workspace == self.workspace
            && target.session_id == self.active.session
            && target.agent != self.active.agent
        {
            let title = self.pane().title.clone();
            self.select(Address::from_target(&target), true, title);
        }
    }

    pub(crate) fn open_agents(&mut self) {
        if !self.pane().exists {
            self.problem("Send the first prompt to establish a Session before switching Agents");
            return;
        }
        self.picker(PickerKind::Agents, Vec::new());
        self.sync_agent_picker();
    }

    pub(super) fn sync_agent_picker(&mut self) {
        let Some(Modal::Picker(picker)) = &self.modal else {
            return;
        };
        if !matches!(picker.kind, PickerKind::Agents) {
            return;
        }
        let pane = self.pane();
        let snapshot = pane.snapshot.as_deref();
        let state = PickerState {
            id: picker.ticket.id,
            cursor: snapshot.map(|snapshot| snapshot.cursor),
            related_revision: self.related_agents.revision(),
            activity: pane.activity(),
            service: self.service_status(),
        };
        if self.navigation.picker.as_ref() == Some(&state) {
            return;
        }
        let selected = picker
            .selected_choice()
            .unwrap_or_else(|| ChoiceKind::Agent {
                target: self.target(&self.active),
            });
        let service = state.service;
        let mut choices = Vec::new();
        let has_snapshot = self.related_agents.has_roster();
        if has_snapshot {
            choices.reserve(self.related_agents.roster().len() + 1);
            for summary in self.related_agents.roster() {
                let current = summary.address == self.active.agent;
                let activity = if service != ServiceStatus::Connected {
                    summary_activity(summary)
                } else if current && snapshot.is_some() {
                    state.activity
                } else {
                    self.related_agents
                        .status(&summary.address)
                        .unwrap_or_else(|| summary_activity(summary))
                };
                let target = Target {
                    workspace: self.workspace.clone(),
                    session_id: self.active.session.clone(),
                    agent: summary.address.clone(),
                };
                choices.push(agent_choice(summary, target, activity, current));
            }
            choices.push(Choice::new(
                "Create Agent",
                "Start with independent context",
                ChoiceKind::CreateAgent,
            ));
        }
        self.navigation.picker = Some(state);
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        picker.loading = !has_snapshot
            && matches!(
                service,
                ServiceStatus::Connecting | ServiceStatus::Reconnecting | ServiceStatus::Restarting
            );
        picker.problem = match (service, has_snapshot) {
            (ServiceStatus::Connected, _) => None,
            (ServiceStatus::SetupRequired, _) => Some("Use /model to set up a model".into()),
            (_, true) => Some(format!("{} · last observed state", service.label())),
            (_, false) if !picker.loading => Some(format!("{} · Ctrl+R", service.label())),
            _ => None,
        };
        picker.set_local(choices);
        if let Some(index) =
            picker
                .visible_choices()
                .iter()
                .position(|choice| match (&choice.kind, &selected) {
                    (ChoiceKind::Agent { target }, ChoiceKind::Agent { target: previous }) => {
                        target == previous
                    }
                    (ChoiceKind::CreateAgent, ChoiceKind::CreateAgent) => true,
                    _ => false,
                })
        {
            picker.selected = index;
        }
    }
}

fn summary_activity(summary: &AgentSummary) -> Activity {
    match summary.status {
        AgentStatus::Running => {
            ExecutionProgress::from_snapshot(summary.progress.as_ref()).activity()
        }
        AgentStatus::Waiting => Activity::Waiting,
        AgentStatus::Stopped if summary.execution_id.is_some() => Activity::Stopping,
        AgentStatus::Stopped => Activity::Stopped,
        AgentStatus::Interrupted => Activity::Interrupted,
        AgentStatus::Idle if summary.last_error_preview.is_some() => Activity::Failed,
        AgentStatus::Idle if summary.turn.is_some() => Activity::Finished,
        AgentStatus::Idle => Activity::Ready,
    }
}

fn agent_choice(
    summary: &AgentSummary,
    target: Target,
    activity: Activity,
    current: bool,
) -> Choice {
    let mut label = format!("{} · {}", single_line(&summary.address), activity.label());
    if current {
        label.push_str(" · Current");
    }
    let mut details = Vec::new();
    if let Some(request) = &summary.question {
        label.push_str(" · Question pending");
        details.push(single_line(&request.text));
    }
    if let Some(request) = &summary.permission {
        label.push_str(" · Approval pending");
        details.push(single_line(&request.summary));
    }
    if summary.pending_inputs > 0 {
        label.push_str(&format!(" · {} queued", summary.pending_inputs));
    }
    if let Some(task) = &summary.admitted_task {
        details.push(format!("Admitted: {}", single_line(task)));
    }
    if let Some(task) = &summary.queued_task {
        details.push(format!("Queued: {}", single_line(task)));
    }
    Choice::new(label, details.join(" · "), ChoiceKind::Agent { target })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use kuku_client::{Error, HistoryEntry, HistoryPage, Snapshot};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use serde_json::{Value, json};

    use crate::jobs::{Job, Update};

    use super::{Address, App, ChoiceKind, Modal};

    fn connected() -> App {
        let workspace = PathBuf::from("workspace");
        let mut app = App::new(workspace.clone(), None);
        app.update(Update::Connected {
            endpoint: "http://127.0.0.1:6789".into(),
            workspace,
        });
        app.complete_test_permissions();
        app.select(app.active.clone(), true, "Navigation fixture".into());
        app.effects.clear();
        app
    }

    fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.event(
            Event::Key(KeyEvent::new(code, modifiers)),
            &Buffer::empty(Rect::default()),
        );
    }

    fn type_in_picker(app: &mut App, text: &str) {
        app.event(Event::Paste(text.into()), &Buffer::empty(Rect::default()));
    }

    fn snapshot(app: &App, cursor: u64, agents: Value) -> Box<Snapshot> {
        let creation = agents
            .as_array()
            .unwrap()
            .iter()
            .find(|summary| summary["address"] == app.active.agent)
            .and_then(|summary| summary["creation_event_id"].as_u64())
            .unwrap_or(1);
        Box::new(
            serde_json::from_value(json!({
                "session_id": app.active.session,
                "workspace": app.workspace,
                "cursor": cursor,
                "agents": agents,
                "pending": [],
                "agent": {
                    "address": app.active.agent,
                    "creation_event_id": creation,
                    "status": "idle",
                    "pending_inputs": 0,
                    "config": {},
                    "usage": {"model_requests": 0, "unreported_requests": 0}
                }
            }))
            .unwrap(),
        )
    }

    fn load(app: &mut App, snapshot: Box<Snapshot>, entries: Vec<HistoryEntry>) {
        let selection = app.selection();
        app.update(Update::Loaded(
            selection.clone(),
            snapshot,
            HistoryPage {
                entries,
                rewind_groups: Vec::new(),
                next_before: None,
            },
        ));
        app.update(Update::WatchConnected(selection));
    }

    fn addresses(app: &App) -> Vec<String> {
        let Some(Modal::Picker(picker)) = &app.modal else {
            panic!("expected Agent picker");
        };
        picker
            .visible_choices()
            .into_iter()
            .filter_map(|choice| match &choice.kind {
                ChoiceKind::Agent { target } => Some(target.agent.clone()),
                _ => None,
            })
            .collect()
    }

    fn selected(app: &App) -> String {
        let Some(Modal::Picker(picker)) = &app.modal else {
            panic!("expected Agent picker");
        };
        let Some(ChoiceKind::Agent { target }) = picker.selected_choice() else {
            panic!("expected Agent selection");
        };
        target.agent
    }

    #[test]
    fn roster_follows_creation_ids_not_live_arrival_or_snapshot_order() {
        let mut app = connected();
        let initial = json!([
            {"address":"main","creation_event_id":1,"status":"idle","pending_inputs":0}
        ]);
        let state = snapshot(&app, 2, initial.clone());
        load(&mut app, state, Vec::new());
        app.open_agents();
        assert_eq!(["main"], addresses(&app).as_slice());
        // Later Agent's live traffic may reach the client before creation is observed.
        app.update(Update::Observation(
            app.selection(),
            serde_json::from_value(json!({
                "type":"live", "agent":"a-tests", "cursor":9, "execution_id":8,
                "event":{"type":"text","content":"Starting tests"}
            }))
            .unwrap(),
        ));
        assert_eq!(["main"], addresses(&app).as_slice());
        // Even a delayed creation event has its actual journal position, not arrival order.
        for (address, id) in [("a-tests", 6), ("z-review", 4)] {
            app.update(Update::Observation(
                app.selection(),
                serde_json::from_value(json!({
                    "type":"committed", "agent":address, "id":id, "kind":"runtime.operation",
                    "operation":"agent_created", "state_changed":true
                }))
                .unwrap(),
            ));
            if address == "a-tests" {
                key(&mut app, KeyCode::Down, KeyModifiers::NONE);
                assert_eq!("a-tests", selected(&app));
            }
        }
        assert_eq!(["main", "z-review", "a-tests"], addresses(&app).as_slice());
        assert_eq!("a-tests", selected(&app));

        let stale = snapshot(&app, 3, initial);
        app.update(Update::Refreshed(app.selection(), Ok(stale)));
        assert_eq!(["main", "z-review", "a-tests"], addresses(&app).as_slice());
        assert_eq!("a-tests", selected(&app));
        let roster = json!([
            {"address":"a-tests","creation_event_id":6,"status":"running","pending_inputs":0,"execution_id":8},
            {"address":"main","creation_event_id":1,"status":"idle","pending_inputs":0},
            {"address":"z-review","creation_event_id":4,"status":"waiting","pending_inputs":0}
        ]);
        app.update(Update::WatchReconnecting(
            app.selection(),
            "connection closed".into(),
        ));
        let state = snapshot(&app, 10, roster.clone());
        load(&mut app, state, Vec::new());
        assert_eq!(["main", "z-review", "a-tests"], addresses(&app).as_slice());
        assert_eq!("a-tests", selected(&app));

        let mut reopened = connected();
        let state = snapshot(&reopened, 10, roster);
        load(&mut reopened, state, Vec::new());
        reopened.open_agents();
        assert_eq!(
            ["main", "z-review", "a-tests"],
            addresses(&reopened).as_slice()
        );
        key(&mut reopened, KeyCode::Esc, KeyModifiers::NONE);
        for address in ["z-review", "a-tests", "main"] {
            key(&mut reopened, KeyCode::Right, KeyModifiers::ALT);
            assert_eq!(address, reopened.active.agent);
        }
        key(&mut reopened, KeyCode::Left, KeyModifiers::ALT);
        assert_eq!("a-tests", reopened.active.agent);
    }

    #[test]
    fn http_refresh_updates_roster_without_reviving_live_activity_when_watch_is_unavailable() {
        for failed in [false, true] {
            let mut app = connected();
            let initial = json!([
                {"address":"main","creation_event_id":1,"status":"idle","pending_inputs":0},
                {"address":"z-review","creation_event_id":3,"status":"running","pending_inputs":0,"execution_id":4}
            ]);
            let state = snapshot(&app, 6, initial.clone());
            load(&mut app, state, Vec::new());
            app.update(Update::Observation(
                app.selection(),
                serde_json::from_value(json!({
                    "type":"live", "agent":"z-review", "cursor":6, "execution_id":4,
                    "event":{"type":"text","content":"previous live reply"}
                }))
                .unwrap(),
            ));
            assert_eq!(
                Some("previous live reply"),
                app.related_agents.iter().next().unwrap().preview
            );
            app.open_agents();
            key(&mut app, KeyCode::Down, KeyModifiers::NONE);
            assert_eq!("z-review", selected(&app));
            let selection = app.selection();
            if failed {
                app.update(Update::WatchFailed(
                    selection.clone(),
                    Error::HttpStatus(403),
                ));
            } else {
                app.update(Update::WatchReconnecting(
                    selection.clone(),
                    "connection closed".into(),
                ));
            }
            assert!(app.related_agents.iter().next().is_none());

            let refreshed = snapshot(
                &app,
                9,
                json!([
                    {"address":"a-tests","creation_event_id":7,"status":"idle","pending_inputs":0},
                    {"address":"main","creation_event_id":1,"status":"idle","pending_inputs":0},
                    {"address":"z-review","creation_event_id":3,"status":"waiting","pending_inputs":2,"queued_task":"Review changes"}
                ]),
            );
            app.update(Update::Refreshed(selection.clone(), Ok(refreshed)));
            assert_eq!(["main", "z-review", "a-tests"], addresses(&app).as_slice());
            assert_eq!("z-review", selected(&app));
            let Some(Modal::Picker(picker)) = &app.modal else {
                panic!("expected Agent picker");
            };
            let choices = picker.visible_choices();
            assert_eq!("z-review · Waiting · 2 queued", choices[1].label);
            assert_eq!("Queued: Review changes", choices[1].detail);
            assert!(app.related_agents.iter().next().is_none());

            // Even while disconnected, a later-arriving older response cannot
            // replace the newly observed summaries or discard their rows.
            let stale = snapshot(&app, 6, initial);
            app.update(Update::Refreshed(selection, Ok(stale)));
            assert_eq!(["main", "z-review", "a-tests"], addresses(&app).as_slice());
            assert_eq!("z-review", selected(&app));
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            for address in ["z-review", "a-tests", "main"] {
                key(&mut app, KeyCode::Right, KeyModifiers::ALT);
                assert_eq!(address, app.active.agent);
            }
        }
    }

    #[test]
    fn returning_to_a_cached_session_uses_its_newest_known_roster_not_its_old_live_activity() {
        let mut app = connected();
        let main = app.active.clone();
        let state = snapshot(
            &app,
            4,
            json!([
                {"address":"main","creation_event_id":1,"status":"idle","pending_inputs":0},
                {"address":"z-review","creation_event_id":3,"status":"idle","pending_inputs":0}
            ]),
        );
        load(&mut app, state, Vec::new());
        app.select(
            Address::new(main.session.clone(), "z-review".into()),
            true,
            String::new(),
        );
        let state = snapshot(
            &app,
            9,
            json!([
                {"address":"a-tests","creation_event_id":7,"status":"running","pending_inputs":0,"execution_id":8},
                {"address":"z-review","creation_event_id":3,"status":"idle","pending_inputs":0},
                {"address":"main","creation_event_id":1,"status":"idle","pending_inputs":0}
            ]),
        );
        load(&mut app, state, Vec::new());
        app.update(Update::Observation(
            app.selection(),
            serde_json::from_value(json!({
                "type":"live", "agent":"a-tests", "cursor":9, "execution_id":8,
                "event":{"type":"text","content":"old peer live reply"}
            }))
            .unwrap(),
        ));
        assert!(
            app.related_agents
                .iter()
                .any(|agent| agent.preview.is_some())
        );

        app.select(Address::main("another-session".into()), true, String::new());
        let state = snapshot(
            &app,
            3,
            json!([
                {"address":"main","creation_event_id":1,"status":"idle","pending_inputs":0},
                {"address":"other","creation_event_id":2,"status":"idle","pending_inputs":0}
            ]),
        );
        load(&mut app, state, Vec::new());

        app.select(main, true, String::new());
        // No new observation yet. The previously selected peer's snapshot is
        // newer than main's cached snapshot and contains one additional Agent.
        assert!(app.related_agents.iter().next().is_none());
        app.open_agents();
        assert_eq!(["main", "z-review", "a-tests"], addresses(&app).as_slice());
        assert_eq!("main", selected(&app));
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        key(&mut app, KeyCode::Right, KeyModifiers::ALT);
        assert_eq!("z-review", app.active.agent);
    }

    #[test]
    fn main_identity_color_does_not_depend_on_the_active_or_highlighted_agent() {
        let mut app = connected();
        let state = snapshot(
            &app,
            4,
            json!([
                {"address":"main","creation_event_id":1,"status":"idle","pending_inputs":0},
                {"address":"z-review","creation_event_id":3,"status":"idle","pending_inputs":0}
            ]),
        );
        load(&mut app, state, Vec::new());
        app.select(
            Address::new(app.active.session.clone(), "z-review".into()),
            true,
            String::new(),
        );
        app.open_agents();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        for highlight_main in [false, true] {
            if highlight_main {
                key(&mut app, KeyCode::Up, KeyModifiers::NONE);
            }
            let buffer = terminal
                .draw(|frame| crate::ui::render(frame, &mut app))
                .unwrap()
                .buffer;
            let colors = |label: &str| {
                let cell = buffer
                    .content
                    .chunks(usize::from(buffer.area.width))
                    .find_map(|row| {
                        row.windows(label.chars().count())
                            .find(|window| {
                                window
                                    .iter()
                                    .zip(label.chars())
                                    .all(|(cell, ch)| cell.symbol() == ch.to_string())
                            })
                            .map(|window| &window[0])
                    })
                    .expect("picker label visible");
                (cell.fg, cell.bg)
            };
            let (main_fg, main_bg) = colors("main · Ready");
            let (peer_fg, peer_bg) = colors("z-review · Ready");
            assert_eq!(crate::theme::MAIN_AGENT, main_fg);
            assert_ne!(main_fg, peer_fg);
            assert_ne!(main_bg, peer_bg);
            assert_eq!(
                if highlight_main {
                    crate::theme::focused_row().bg
                } else {
                    crate::theme::panel().bg
                },
                Some(main_bg)
            );
        }
    }

    #[test]
    fn alt_arrows_preserve_each_agents_draft_and_reading_position() {
        let mut app = connected();
        let agents = json!([
            {"address":"main","creation_event_id":1,"status":"idle","pending_inputs":0,"admitted_task":"Ask the reviewer about this task"},
            {"address":"review","creation_event_id":2,"status":"idle","pending_inputs":0},
            {"address":"reviewer","creation_event_id":3,"status":"idle","pending_inputs":0}
        ]);
        let main_history = (1..=36)
            .map(|id| {
                serde_json::from_value(json!({
                    "id":id, "agent":"main", "type":"assistant", "text":format!("Reply {id}")
                }))
                .unwrap()
            })
            .collect::<Vec<HistoryEntry>>();
        let main = snapshot(&app, 36, agents.clone());
        load(&mut app, main, main_history.clone());
        app.pane_mut()
            .draft
            .set_text("Keep the parent draft".into());
        key(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
        let reading = app.pane_mut().transcript.project(80, 16);
        assert!(!reading.following);
        key(&mut app, KeyCode::Down, KeyModifiers::ALT);
        assert_eq!("main", app.active.agent);
        key(&mut app, KeyCode::Right, KeyModifiers::ALT);
        assert_eq!("review", app.active.agent);
        assert!(app.modal.is_none());
        app.effects.clear();
        key(&mut app, KeyCode::Right, KeyModifiers::ALT);
        assert_eq!("reviewer", app.active.agent);
        assert!(matches!(
            app.effects.as_slice(),
            [Job::Observe(selection)] if selection == &app.selection()
        ));
        let reviewer = snapshot(&app, 36, agents.clone());
        load(&mut app, reviewer, Vec::new());
        app.pane_mut().draft.set_text("Keep the child draft".into());
        app.effects.clear();
        key(&mut app, KeyCode::Right, KeyModifiers::ALT);
        assert_eq!("main", &app.active.agent);
        assert_eq!("Keep the parent draft", app.pane().draft.text());
        assert!(
            matches!(app.effects.as_slice(), [Job::Observe(selection)] if selection == &app.selection())
        );
        let main = snapshot(&app, 37, agents);
        load(&mut app, main, main_history);
        let restored = app.pane_mut().transcript.project(80, 16);
        assert!(!restored.following);
        assert_eq!(reading.lines, restored.lines);

        let previous = app.previous_agent().cloned().unwrap();
        let previous_draft = app.panes.get(&previous).unwrap().draft.text().to_owned();
        key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
        assert!(matches!(app.pane().contextual_focus.cursor,
            Some(crate::contextual::ContextControl::PreviousAgent { ref target }) if target == &previous));
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(previous.agent, app.active.agent);
        assert_eq!(previous_draft, app.pane().draft.text());

        key(&mut app, KeyCode::Left, KeyModifiers::ALT);
        assert_eq!("review", app.active.agent);
        key(&mut app, KeyCode::Left, KeyModifiers::ALT);
        assert_eq!("main", app.active.agent);
        key(&mut app, KeyCode::Left, KeyModifiers::ALT);
        assert_eq!("reviewer", app.active.agent);

        let old_target = app.selection().target;
        app.select(
            Address::main("another-session".into()),
            true,
            "Another conversation".into(),
        );
        key(&mut app, KeyCode::Left, KeyModifiers::ALT);
        app.select_agent(old_target);
        assert_eq!(Address::main("another-session".into()), app.active);
        assert!(app.previous_agent().is_none());
    }

    #[test]
    fn picker_ranks_the_address_before_matches_in_task_text() {
        let mut app = connected();
        let agents = json!([
            {"address":"main","creation_event_id":1,"status":"idle","pending_inputs":0,"admitted_task":"Ask the reviewer"},
            {"address":"reviewer","creation_event_id":2,"status":"idle","pending_inputs":0},
            {"address":"review","creation_event_id":3,"status":"idle","pending_inputs":0}
        ]);
        let initial = snapshot(&app, 3, agents);
        load(&mut app, initial, Vec::new());
        app.open_agents();
        type_in_picker(&mut app, "review");
        let Some(Modal::Picker(picker)) = &app.modal else {
            panic!("expected Agent picker")
        };
        let addresses = picker
            .visible_choices()
            .into_iter()
            .filter_map(|choice| match &choice.kind {
                ChoiceKind::Agent { target } => Some(target.agent.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(["review", "reviewer", "main"], addresses.as_slice());
    }

    #[test]
    fn picker_searches_durable_tasks_and_pending_requests_and_drops_live_state_on_disconnect() {
        let mut app = connected();
        let peer = json!({
            "address":"reviewer","creation_event_id":2,"status":"running","pending_inputs":1,"execution_id":5,
            "progress":{"phase":"waiting_for_model"},
            "admitted_task":"Inspect receipts",
            "queued_task":"Check retry behavior"
        });
        let initial = snapshot(
            &app,
            6,
            json!([
                {"address":"main","creation_event_id":1,"status":"idle","pending_inputs":0}, peer
            ]),
        );
        load(&mut app, initial, Vec::new());
        app.open_agents();
        type_in_picker(&mut app, "receipts retry");
        let choice = |app: &App| match &app.modal {
            Some(Modal::Picker(picker)) => {
                let matches = picker.visible_choices();
                assert_eq!(1, matches.len());
                (
                    matches[0].label.clone(),
                    matches[0].detail.clone(),
                    picker.problem.clone(),
                )
            }
            _ => panic!("expected Agent picker"),
        };
        let (label, detail, _) = choice(&app);
        assert_eq!("reviewer · Waiting for model · 1 queued", label);
        assert_eq!(
            "Admitted: Inspect receipts · Queued: Check retry behavior",
            detail
        );
        app.update(Update::Observation(
            app.selection(),
            serde_json::from_value(json!({
                "type":"live","agent":"reviewer","cursor":6,"execution_id":5,
                "event":{"type":"text","content":"First live tokens"}
            }))
            .unwrap(),
        ));
        assert_eq!("reviewer · Streaming reply · 1 queued", choice(&app).0);
        app.update(Update::WatchReconnecting(
            app.selection(),
            "connection closed".into(),
        ));
        let (label, _, problem) = choice(&app);
        assert_eq!("reviewer · Waiting for model · 1 queued", label);
        assert_eq!(
            Some("Reconnecting · last observed state"),
            problem.as_deref()
        );

        let waiting = snapshot(
            &app,
            10,
            json!([
                {"address":"main","creation_event_id":1,"status":"idle","pending_inputs":0},
                {
                    "address":"reviewer","creation_event_id":2,"status":"waiting","pending_inputs":1,
                    "admitted_task":"Inspect receipts",
                    "queued_task":"Check retry behavior",
                    "question":{"id":"question","agent":"reviewer","turn":5,"tool_call_id":"ask","text":"Which branch?"}
                }
            ]),
        );
        load(&mut app, waiting, Vec::new());
        assert_eq!(
            "reviewer · Waiting · Question pending · 1 queued",
            choice(&app).0
        );
        assert!(choice(&app).1.contains("Which branch?"));
        assert!(choice(&app).2.is_none());
        assert!(matches!(app.effects.as_slice(), [Job::PendingApprovals(_)]));
    }
}
