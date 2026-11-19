use kuku_client::{AgentStatus, AgentSummary, Target};

use crate::keys::AgentDirection;
use crate::menu::{Choice, ChoiceKind, PickerKind};
use crate::text::single_line;

use super::{Activity, Address, App, ExecutionProgress, Modal, ServiceStatus};

#[derive(Debug, Default)]
pub(super) struct AgentNavigation {
    previous: Option<Address>,
    roster: Vec<String>,
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
        if address.session != self.active.session {
            self.navigation = AgentNavigation::default();
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
        let roster = &mut self.navigation.roster;
        roster.push(self.active.agent.clone());
        if let Some(snapshot) = &pane.snapshot {
            roster.extend(snapshot.agents.iter().map(|agent| agent.address.clone()));
        }
        roster.extend(
            self.related_agents
                .iter()
                .map(|agent| agent.address.to_owned()),
        );
        roster.sort_unstable_by(|a, b| {
            (a != super::DEFAULT_AGENT, a).cmp(&(b != super::DEFAULT_AGENT, b))
        });
        roster.dedup();
        if roster.len() == 1 {
            return;
        }
        let current = roster
            .iter()
            .position(|address| address == &self.active.agent)
            .expect("the roster includes the active Agent");
        let next = match direction {
            AgentDirection::Next => (current + 1) % roster.len(),
            AgentDirection::Previous => (current + roster.len() - 1) % roster.len(),
        };
        let address = Address::new(self.active.session.clone(), roster[next].clone());
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
        if let Some(snapshot) = snapshot {
            choices.reserve(snapshot.agents.len() + 1);
            for summary in &snapshot.agents {
                let current = summary.address == self.active.agent;
                let activity = if service != ServiceStatus::Connected {
                    summary_activity(summary)
                } else if current {
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
        let has_snapshot = snapshot.is_some();
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
    use kuku_client::{HistoryEntry, HistoryPage, Snapshot};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use serde_json::{json, Value};

    use crate::jobs::{Job, Update};

    use super::{Address, App, ChoiceKind, Modal};

    fn connected() -> App {
        let workspace = PathBuf::from("workspace");
        let mut app = App::new(workspace.clone(), None);
        app.update(Update::Connected {
            endpoint: "http://127.0.0.1:6789".into(),
            workspace,
        });
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
        Box::new(
            serde_json::from_value(json!({
                "session_id": app.active.session,
                "workspace": app.workspace,
                "cursor": cursor,
                "agents": agents,
                "pending": [],
                "agent": {
                    "address": app.active.agent,
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
                next_before: None,
            },
        ));
        app.update(Update::WatchConnected(selection));
    }

    #[test]
    fn alt_arrows_preserve_each_agents_draft_and_reading_position() {
        let mut app = connected();
        let agents = json!([
            {"address":"main","status":"idle","pending_inputs":0,"admitted_task":"Ask the reviewer about this task"},
            {"address":"review","status":"idle","pending_inputs":0},
            {"address":"reviewer","status":"idle","pending_inputs":0}
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
        assert!(app.focus_context_control(crate::contextual::ContextControl::PreviousAgent {
            target: previous.clone(),
        }));
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
            {"address":"main","status":"idle","pending_inputs":0,"admitted_task":"Ask the reviewer"},
            {"address":"reviewer","status":"idle","pending_inputs":0},
            {"address":"review","status":"idle","pending_inputs":0}
        ]);
        let initial = snapshot(&app, 1, agents);
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
            "address":"reviewer","status":"running","pending_inputs":1,"execution_id":5,
            "progress":{"phase":"waiting_for_model"},
            "admitted_task":"Inspect receipts",
            "queued_task":"Check retry behavior"
        });
        let initial = snapshot(
            &app,
            6,
            json!([
                {"address":"main","status":"idle","pending_inputs":0}, peer
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
                {"address":"main","status":"idle","pending_inputs":0},
                {
                    "address":"reviewer","status":"waiting","pending_inputs":1,
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
        assert!(app.effects.is_empty());
    }
}
