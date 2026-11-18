use std::collections::{BTreeMap, HashSet};

use kuku_client::{
    AgentProgress, AgentSettlement, AgentStatus, AgentSummary, Observation, Snapshot,
};
use serde_json::Value;

use crate::text::cap;

use super::{Activity, ExecutionProgress};

const MAX_AGENTS: usize = 64;
const PREVIEW_BYTES: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AgentActivity<'a> {
    pub(crate) address: &'a str,
    pub(crate) status: Activity,
    pub(crate) preview: Option<&'a str>,
}

#[derive(Debug, Default)]
pub(crate) struct AgentActivities {
    agents: BTreeMap<String, TrackedAgent>,
    revision: u64,
    minimum_cursor: u64,
}

impl AgentActivities {
    pub(crate) fn iter(&self) -> impl Iterator<Item = AgentActivity<'_>> {
        self.agents.iter().map(|(address, agent)| AgentActivity {
            address,
            status: agent.activity(),
            preview: (!agent.preview.is_empty()).then_some(agent.preview.as_str()),
        })
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn status(&self, address: &str) -> Option<Activity> {
        self.agents.get(address).map(TrackedAgent::activity)
    }

    pub(super) fn invalidate(&mut self, cursor: u64) {
        self.agents.clear();
        self.minimum_cursor = cursor;
        self.changed();
    }

    pub(super) fn snapshot(&mut self, snapshot: &Snapshot, selected: &str) {
        let cursor = snapshot.cursor;
        if cursor < self.minimum_cursor {
            return;
        }
        let retained: HashSet<_> = snapshot.agents.iter().map(|agent| &agent.address).collect();
        let before = self.agents.len();
        self.agents
            .retain(|address, agent| agent.state_cursor > cursor || retained.contains(address));
        let mut changed = self.agents.len() != before;
        for summary in &snapshot.agents {
            if summary.address == selected {
                continue;
            }
            if let Some(agent) = self.agents.get_mut(&summary.address) {
                changed |= agent.replace_summary(summary, cursor);
            } else if self.agents.len() < MAX_AGENTS {
                self.agents.insert(
                    summary.address.clone(),
                    TrackedAgent::from_summary(summary, cursor),
                );
                changed = true;
            }
        }
        if changed {
            self.changed();
        }
    }

    pub(super) fn observe(&mut self, event: &Observation, selected: &str) {
        let changed = match event {
            Observation::Committed {
                id,
                agent: Some(address),
                ..
            } if *id > self.minimum_cursor && address != selected => {
                if let Some(agent) = self.agents.get_mut(address) {
                    agent.committed(event)
                } else {
                    self.insert_created_or_running(address, *id, event)
                }
            }
            Observation::Live {
                agent: address,
                execution_id,
                cursor,
                event,
                ..
            } if address != selected
                && *cursor >= self.minimum_cursor
                && *execution_id <= *cursor =>
            {
                self.observe_live(address, *execution_id, *cursor, event)
            }
            Observation::Committed { .. }
            | Observation::Live { .. }
            | Observation::Lagged { .. }
            | Observation::Heartbeat => false,
        };
        if changed {
            self.changed();
        }
    }

    fn observe_live(&mut self, address: &str, execution: u64, cursor: u64, event: &Value) -> bool {
        if let Some(agent) = self.agents.get_mut(address) {
            return agent.live(execution, cursor, event);
        }
        if execution <= self.minimum_cursor || self.agents.len() >= MAX_AGENTS {
            return false;
        }
        let mut agent = TrackedAgent::new(AgentStatus::Running, Some(execution), execution);
        agent.live(execution, cursor, event);
        self.agents.insert(address.to_owned(), agent);
        true
    }

    fn insert_created_or_running(
        &mut self,
        address: &str,
        cursor: u64,
        event: &Observation,
    ) -> bool {
        if self.agents.len() >= MAX_AGENTS {
            return false;
        }
        let Observation::Committed {
            operation: Some(operation),
            ..
        } = event
        else {
            return false;
        };
        let agent = match operation.as_str() {
            "agent_created" => TrackedAgent::new(AgentStatus::Idle, None, cursor),
            "execution_started" => TrackedAgent::new(AgentStatus::Running, Some(cursor), cursor),
            _ => return false,
        };
        self.agents.insert(address.to_owned(), agent);
        true
    }

    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

#[derive(Debug)]
struct TrackedAgent {
    status: AgentStatus,
    execution: Option<u64>,
    settled: bool,
    failed: bool,
    state_cursor: u64,
    phase_cursor: u64,
    progress: ExecutionProgress,
    preview: String,
    snapshot_prefix: bool,
}

impl TrackedAgent {
    fn new(status: AgentStatus, execution: Option<u64>, cursor: u64) -> Self {
        Self {
            status,
            execution,
            settled: false,
            failed: false,
            state_cursor: cursor,
            phase_cursor: cursor,
            progress: ExecutionProgress::default(),
            preview: String::new(),
            snapshot_prefix: false,
        }
    }

    fn from_summary(summary: &AgentSummary, cursor: u64) -> Self {
        let mut agent = Self::new(summary.status, summary.execution_id, cursor);
        agent.settled = summary.turn.is_some();
        agent.failed = summary.last_error_preview.is_some();
        agent.progress = ExecutionProgress::from_snapshot(summary.progress.as_ref());
        if let Some(AgentProgress::Streaming { preview }) = &summary.progress {
            agent.preview = excerpt(Some(preview));
            agent.snapshot_prefix = true;
        } else if summary.status != AgentStatus::Running {
            agent.preview = excerpt(
                summary
                    .last_error_preview
                    .as_deref()
                    .or(summary.last_output_preview.as_deref()),
            );
        }
        agent
    }

    fn replace_summary(&mut self, summary: &AgentSummary, cursor: u64) -> bool {
        if cursor < self.state_cursor {
            return false;
        }
        if summary.status == AgentStatus::Running
            && self.status == AgentStatus::Running
            && summary.execution_id == self.execution
        {
            if summary.progress.is_some()
                && (cursor > self.phase_cursor
                    || cursor == self.phase_cursor && self.activity() == Activity::Running)
            {
                return self.replace(Self::from_summary(summary, cursor));
            }
            self.state_cursor = cursor;
            return false;
        }
        self.replace(Self::from_summary(summary, cursor))
    }

    fn committed(&mut self, event: &Observation) -> bool {
        let Observation::Committed {
            id,
            kind,
            operation,
            settlement,
            cleanup_pending,
            tool_call_id,
            ..
        } = event
        else {
            return false;
        };
        let cursor = *id;
        if cursor > self.state_cursor {
            match operation.as_deref() {
                Some("execution_started") => {
                    return self.replace(Self::new(AgentStatus::Running, Some(cursor), cursor));
                }
                Some("turn_settled") => {
                    if let Some(settlement) = settlement {
                        return self.settled(settlement, cursor);
                    }
                }
                Some("permission_waiting") => {
                    return self.replace(Self::new(AgentStatus::Waiting, self.execution, cursor));
                }
                Some("agent_stopped") => {
                    let execution = self.execution.filter(|_| *cleanup_pending == Some(true));
                    return self.replace(Self::new(AgentStatus::Stopped, execution, cursor));
                }
                _ => {}
            }
        }
        self.phase(kind, tool_call_id.as_deref(), cursor)
    }

    fn settled(&mut self, settlement: &AgentSettlement, cursor: u64) -> bool {
        let execution = self
            .execution
            .filter(|_| settlement.status == AgentStatus::Waiting);
        let mut agent = Self::new(settlement.status, execution, cursor);
        agent.settled = settlement.turn.is_some() || settlement.output.is_some();
        agent.failed = settlement.error.is_some();
        agent.preview = excerpt(settlement.error.as_deref().or(settlement.output.as_deref()));
        self.replace(agent)
    }

    fn phase(&mut self, kind: &str, tool_call_id: Option<&str>, cursor: u64) -> bool {
        if self.status != AgentStatus::Running || cursor <= self.phase_cursor {
            return false;
        }
        let before = self.activity();
        match kind {
            "runtime.step_prepared" | "runtime.model_retry" | "model.recovery" => {
                self.phase_cursor = cursor;
                self.progress.committed(kind, None, None);
                return self.clear_preview() || before != self.activity();
            }
            "model.response" | "model.error" | "runtime.tool_dispatched" | "tool.result" => {
                self.phase_cursor = cursor;
                self.progress
                    .committed(kind, None, tool_call_id.map(str::to_owned));
            }
            _ => {}
        }
        before != self.activity()
    }

    fn live(&mut self, execution: u64, cursor: u64, event: &Value) -> bool {
        if cursor < self.phase_cursor {
            return false;
        }
        let before = self.activity();
        let mut preview_changed = false;
        if self.execution != Some(execution) {
            if execution <= self.state_cursor {
                return false;
            }
            preview_changed = !self.preview.is_empty();
            *self = Self::new(AgentStatus::Running, Some(execution), execution);
        } else if self.status != AgentStatus::Running {
            return false;
        }
        self.phase_cursor = cursor;
        self.progress.live(event);
        match event["type"].as_str() {
            Some("text") if !self.snapshot_prefix => {
                if let Some(text) = event["content"].as_str() {
                    let suffix = cap(text, PREVIEW_BYTES - self.preview.len());
                    self.preview.push_str(suffix);
                    preview_changed |= !suffix.is_empty();
                }
            }
            Some("model_request" | "model_recovery") => preview_changed |= self.clear_preview(),
            _ => {}
        }
        preview_changed || before != self.activity()
    }

    fn clear_preview(&mut self) -> bool {
        let changed = !self.preview.is_empty();
        self.preview.clear();
        self.snapshot_prefix = false;
        changed
    }

    fn replace(&mut self, new: Self) -> bool {
        let changed = self.activity() != new.activity() || self.preview != new.preview;
        *self = new;
        changed
    }

    fn activity(&self) -> Activity {
        match self.status {
            AgentStatus::Running => self.progress.activity(),
            AgentStatus::Waiting => Activity::Waiting,
            AgentStatus::Stopped if self.execution.is_some() => Activity::Stopping,
            AgentStatus::Stopped => Activity::Stopped,
            AgentStatus::Interrupted => Activity::Interrupted,
            AgentStatus::Idle if self.failed => Activity::Failed,
            AgentStatus::Idle if self.settled => Activity::Finished,
            AgentStatus::Idle => Activity::Ready,
        }
    }
}

fn excerpt(text: Option<&str>) -> String {
    cap(text.unwrap_or_default(), PREVIEW_BYTES).to_owned()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use kuku_client::{AgentProgress, HistoryPage, Observation, Snapshot};
    use serde_json::{json, Value};

    use super::{Activity, AgentActivity, PREVIEW_BYTES};
    use crate::app::{Address, App};
    use crate::jobs::{Selection, Update};

    fn attached() -> App {
        let workspace = PathBuf::from("/workspace");
        let mut app = App::new(workspace.clone(), None);
        app.update(Update::Connected {
            endpoint: "http://127.0.0.1:6789".into(),
            workspace,
        });
        app.select(Address::main("session-1".into()), true, String::new());
        load(&mut app, 6, "idle", None, None);
        app
    }

    fn snapshot(
        app: &App,
        cursor: u64,
        status: &str,
        execution: Option<u64>,
        preview: Option<&str>,
    ) -> Box<Snapshot> {
        Box::new(serde_json::from_value(json!({
            "session_id":app.active.session,"workspace":app.workspace,"cursor":cursor,"pending":[],
            "agent":{"address":app.active.agent,"status":"idle","config":{},"pending_inputs":0,
                "usage":{"model_requests":0,"unreported_requests":0}},
            "agents":[{"address":"reviewer","status":status,"execution_id":execution,"pending_inputs":0,
                "turn":preview.map(|_| 7),"last_output_preview":preview}],
        })).unwrap())
    }

    fn load(
        app: &mut App,
        cursor: u64,
        status: &str,
        execution: Option<u64>,
        preview: Option<&str>,
    ) {
        let selection = app.selection();
        let snapshot = snapshot(app, cursor, status, execution, preview);
        app.update(Update::Loaded(
            selection.clone(),
            snapshot,
            HistoryPage {
                entries: Vec::new(),
                next_before: None,
            },
        ));
        app.update(Update::WatchConnected(selection));
    }

    fn reviewer(app: &App) -> Option<AgentActivity<'_>> {
        app.related_agents
            .iter()
            .find(|activity| activity.address == "reviewer")
    }

    fn observe(app: &mut App, event: Value) {
        app.update(Update::Observation(
            app.selection(),
            serde_json::from_value(event).unwrap(),
        ));
    }

    fn committed(app: &mut App, kind: &str, id: u64, fields: Value) {
        let mut frame = json!({"type":"committed","agent":"reviewer","kind":kind,"id":id});
        frame
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        observe(app, frame);
    }

    fn live_frame(execution: u64, cursor: u64, text: &str) -> Value {
        json!({"type":"live","agent":"reviewer","execution_id":execution,"cursor":cursor,
            "event":{"type":"text","content":text}})
    }

    fn apply_live(app: &mut App, selection: Selection, execution: u64, cursor: u64, text: &str) {
        let frame = serde_json::from_value(live_frame(execution, cursor, text)).unwrap();
        app.update(Update::Observation(selection, frame));
    }

    #[test]
    fn related_columns_use_durable_boundaries_and_matching_live_text() {
        let mut app = attached();
        assert_eq!(Activity::Ready, reviewer(&app).unwrap().status);
        committed(
            &mut app,
            "runtime.operation",
            8,
            json!({"operation":"execution_started"}),
        );
        assert_eq!(Activity::Running, reviewer(&app).unwrap().status);
        committed(&mut app, "runtime.step_prepared", 9, json!({}));
        assert_eq!(Activity::WaitingForModel, reviewer(&app).unwrap().status);
        observe(&mut app, live_frame(7, 9, "stale root"));
        assert_eq!(Activity::WaitingForModel, reviewer(&app).unwrap().status);

        observe(
            &mut app,
            json!({
                "type":"live", "agent":"reviewer", "execution_id":8, "cursor":9,
                "event":{"type":"thinking"},
            }),
        );
        let thinking = reviewer(&app).unwrap();
        assert_eq!(Activity::Thinking, thinking.status);
        assert_eq!(None, thinking.preview);

        observe(&mut app, live_frame(8, 9, &"界".repeat(PREVIEW_BYTES)));
        let streaming = reviewer(&app).unwrap();
        assert_eq!(Activity::Streaming, streaming.status);
        assert_eq!(PREVIEW_BYTES, streaming.preview.unwrap().len());
        assert_eq!(Activity::Ready, app.pane().activity());
        let revision = app.related_agents.revision();
        observe(&mut app, live_frame(8, 9, "extremely long stream"));
        assert_eq!(revision, app.related_agents.revision());

        committed(&mut app, "model.response", 10, json!({}));
        committed(
            &mut app,
            "runtime.tool_dispatched",
            11,
            json!({"tool_call_id":"verify"}),
        );
        assert_eq!(Activity::RunningTools, reviewer(&app).unwrap().status);
        observe(&mut app, live_frame(8, 9, "stale first step"));
        assert_eq!(Activity::RunningTools, reviewer(&app).unwrap().status);
        committed(
            &mut app,
            "tool.result",
            12,
            json!({"tool_call_id":"verify"}),
        );
        assert_eq!(Activity::Running, reviewer(&app).unwrap().status);
        committed(
            &mut app,
            "runtime.operation",
            13,
            json!({
                "operation":"turn_settled","settlement":{"status":"idle","turn":7,"output":"Verified."}
            }),
        );
        let finished = reviewer(&app).unwrap();
        assert_eq!(Activity::Finished, finished.status);
        assert_eq!(Some("Verified."), finished.preview);
        observe(&mut app, live_frame(8, 12, "late text"));
        assert_eq!(Some("Verified."), reviewer(&app).unwrap().preview);
    }

    #[test]
    fn related_activity_discards_stale_previews_and_recovers_at_snapshot_cursor() {
        let mut app = attached();
        load(&mut app, 10, "running", Some(8), Some("previous turn"));
        assert_eq!(Activity::Running, reviewer(&app).unwrap().status);
        assert_eq!(None, reviewer(&app).unwrap().preview);
        let original = app.selection();
        apply_live(&mut app, original.clone(), 8, 10, "first attachment");
        assert_eq!(Some("first attachment"), reviewer(&app).unwrap().preview);

        let stale = snapshot(&app, 9, "running", Some(8), None);
        app.update(Update::WatchReconnecting(
            original.clone(),
            "offline".into(),
        ));
        app.update(Update::Observation(
            original.clone(),
            Observation::Lagged { skipped: 6 },
        ));
        assert!(reviewer(&app).is_none());
        app.update(Update::WatchConnected(original.clone()));
        app.update(Update::Refreshed(original.clone(), Ok(stale)));
        assert!(reviewer(&app).is_none());

        let mut running_tools = snapshot(&app, 11, "running", Some(8), Some("old output"));
        running_tools.agents[0].progress = Some(AgentProgress::RunningTools {
            tool_call_ids: vec!["verify".into()],
        });
        app.update(Update::Refreshed(original.clone(), Ok(running_tools)));
        assert_eq!(Activity::RunningTools, reviewer(&app).unwrap().status);
        assert_eq!(None, reviewer(&app).unwrap().preview);

        let settled = snapshot(&app, 13, "idle", None, Some("durable reply"));
        app.update(Update::Refreshed(original.clone(), Ok(settled)));
        assert_eq!(Activity::Finished, reviewer(&app).unwrap().status);
        assert_eq!(Some("durable reply"), reviewer(&app).unwrap().preview);
        apply_live(&mut app, original.clone(), 8, 12, "lost stream");
        assert_eq!(Some("durable reply"), reviewer(&app).unwrap().preview);

        let revision = app.related_agents.revision();
        app.select(Address::main("session-2".into()), true, String::new());
        load(&mut app, 3, "idle", None, None);
        assert_ne!(revision, app.related_agents.revision());
        apply_live(&mut app, original, 8, 12, "different session");
        assert_eq!(Activity::Ready, reviewer(&app).unwrap().status);
        assert_eq!(None, reviewer(&app).unwrap().preview);
    }

    #[test]
    fn a_live_delta_can_precede_delivery_of_its_durable_step() {
        let mut app = attached();
        observe(&mut app, live_frame(8, 10, "observed text"));
        committed(
            &mut app,
            "runtime.operation",
            8,
            json!({"operation":"execution_started"}),
        );
        committed(&mut app, "runtime.step_prepared", 10, json!({}));
        assert_eq!(Activity::Streaming, reviewer(&app).unwrap().status);
        assert_eq!(Some("observed text"), reviewer(&app).unwrap().preview);
        committed(&mut app, "model.response", 11, json!({}));
        assert_eq!(Activity::Running, reviewer(&app).unwrap().status);
    }
}
