use std::collections::{HashMap, HashSet};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use kuku::adapter::inspect_model;
use kuku::event::{EventPayload, EventStore};
use kuku::query::ModelPhase;
use kuku::session::{AgentSnapshot, AgentStatus, ReplyPreviews, Session};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Params;
use super::workspaces::resolve_workspace;
use super::{SessionTarget, WorkspaceQuery, canonical_home, journal_task, manager};
use crate::AppState;
use crate::error_mapping::{ApiError, Result};
use crate::run_manager::RunManager;
use crate::wire::{agent_summary, event_agent};

const TITLE_CHARS: usize = 160;
const PROGRESS_WINDOW: usize = 64;

#[derive(Deserialize)]
pub(super) struct ListQuery {
    workspace: Option<PathBuf>,
    search: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct SnapshotQuery {
    workspace: PathBuf,
    agent: Option<String>,
    #[serde(default)]
    progress: bool,
}

pub(super) async fn list(
    State(state): State<Arc<AppState>>,
    Params(query): Params<ListQuery>,
) -> Result<Json<Value>> {
    let manager = manager(&state).await;
    let permit = manager.catalog_permit().await;
    journal_task(move || {
        let _permit = permit;
        list_sessions(&state.kuku_home, &manager, query)
    })
    .await
}

fn list_sessions(home: &FsPath, manager: &RunManager, query: ListQuery) -> Result<Json<Value>> {
    let workspace = query
        .workspace
        .as_deref()
        .map(resolve_workspace)
        .transpose()?;
    let home = canonical_home(home)?;
    let search = query.search.unwrap_or_default().to_lowercase();
    let mut sessions = Vec::new();
    for file in kuku::session::session_files(&home, workspace.as_deref())? {
        let open = manager.open_session(file.events_path());
        let listing = match file.listing(open.as_ref()) {
            Ok(listing) => listing,
            Err(error) => {
                tracing::warn!(%error, path = %file.events_path().display(), "Session listing read failed");
                continue;
            }
        };
        let summary = listing.summary;
        let title = summary.title.chars().take(TITLE_CHARS).collect::<String>();
        if !search.is_empty()
            && !title.to_lowercase().contains(&search)
            && !summary.session_id.to_lowercase().contains(&search)
        {
            continue;
        }
        sessions.push(json!({
            "session_id":summary.session_id,
            "workspace":summary.workspace,
            "title":title,
            "created_at":summary.created_at,
            "turn_count":summary.turn_count,
            "status":summary.status,
            "mtime":summary.mtime,
            "size":summary.size,
            "waiting_agents":listing.waiting_agents,
        }));
    }
    Ok(Json(json!({"ok":true,"sessions":sessions})))
}

pub(super) async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Params(query): Params<WorkspaceQuery>,
) -> Result<Json<Value>> {
    let workspace = resolve_workspace(&query.workspace)?;
    let home = canonical_home(&state.kuku_home)?;
    manager(&state)
        .await
        .delete_session(&home, &workspace, &id)
        .await?;
    Ok(Json(json!({"ok":true})))
}

pub(super) async fn agents(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Params(query): Params<WorkspaceQuery>,
) -> Result<Json<Value>> {
    let target = SessionTarget::new(&state, &query.workspace, &id)?;
    target.require_exists()?;
    let manager = manager(&state).await;
    journal_task(move || {
        let open = manager.open_session(&target.path);
        let (agents, cursor) = match &open {
            Some(session) => session.agents_with_cursor()?,
            None => Session::read_agents_with_cursor(&target.path)?,
        };
        let agents: Vec<_> = agents.iter().map(agent_summary).collect();
        Ok(Json(
            json!({"ok":true,"session_id":id,"workspace":target.workspace,"cursor":cursor,"agents":agents}),
        ))
    }).await
}

pub(super) async fn snapshot(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Params(query): Params<SnapshotQuery>,
) -> Result<Json<Value>> {
    let target = SessionTarget::new(&state, &query.workspace, &id)?;
    target.require_exists()?;
    let manager = manager(&state).await;
    journal_task(move || session_snapshot(&state, &manager, &target, &id, query)).await
}

fn session_snapshot(
    state: &AppState,
    manager: &RunManager,
    target: &SessionTarget,
    id: &str,
    query: SnapshotQuery,
) -> Result<Json<Value>> {
    let address = query.agent.as_deref().unwrap_or("main");
    let mut open = manager.open_session(&target.path);
    let read_open = |session: &Session| {
        if query.progress {
            session.agent_view_with_replies(address)
        } else {
            session
                .agent_view(address)
                .map(|view| (view, ReplyPreviews::default()))
        }
    };
    let (view, mut replies) = match &open {
        Some(session) => read_open(session),
        None => {
            let read = Session::read_view(&target.path, address)
                .map(|view| (view, ReplyPreviews::default()));
            open = manager.open_session(&target.path);
            open.as_ref().map_or(read, read_open)
        }
    }
    .map_err(agent_error)?;
    let mut progress = if query.progress {
        execution_progress(&target.path, &view.agents, view.cursor)?
    } else {
        HashMap::new()
    };
    let projected_progress = progress.remove(address);
    let selected_progress = if projected_progress
        .as_ref()
        .is_some_and(|progress| progress["retry"]["will_retry"] == false)
    {
        projected_progress
    } else {
        replies
            .selected
            .as_ref()
            .map(|reply| provider_progress(reply.phase, ""))
            .or(projected_progress)
    };
    let agents: Vec<_> = view
        .agents
        .iter()
        .map(|agent| {
            let mut summary = agent_summary(agent);
            let progress = if agent.address == address {
                selected_progress.clone()
            } else if let Some(preview) = replies.peers.remove(&agent.address) {
                Some(provider_progress(preview.phase, &preview.text))
            } else {
                progress.remove(agent.address.as_str())
            };
            if let Some(progress) = progress {
                summary["progress"] = progress;
            }
            summary
        })
        .collect();
    let mut value = json!({
        "ok":true,
        "session_id":id,
        "workspace":target.workspace,
        "cursor":view.cursor,
        "agent":view.snapshot,
        "agents":agents,
        "session_costs":view.session_costs(),
        "pending":view.pending_inputs,
        "selection":null,
    });
    if let Some(progress) = selected_progress {
        value["progress"] = progress;
    }
    if let (Some(session), Some(reply)) = (&open, replies.selected) {
        value["reply_preview"] = serde_json::to_value(&reply).map_err(kuku::Error::from)?;
        if let Some(number) = session.turn_number(address, reply.turn) {
            value["reply_preview"]["turn_number"] = json!(number);
        }
    }
    let effective = view
        .snapshot
        .active_config
        .as_ref()
        .unwrap_or(&view.snapshot.config);
    match selection(state, open.as_ref(), address, effective) {
        Ok(selection) => value["selection"] = selection,
        Err(error) => value["selection_error"] = ApiError::from(error).value(),
    }
    if effective != &view.snapshot.config {
        match selection(state, open.as_ref(), address, &view.snapshot.config) {
            Ok(selection) => value["pending_selection"] = selection,
            Err(error) => value["pending_selection_error"] = ApiError::from(error).value(),
        }
    }
    if let Some(session) = &open {
        value["cleaning_up"] = json!(
            view.snapshot.status == AgentStatus::Stopped
                && session
                    .active_executions()
                    .iter()
                    .any(|(address, _)| address == &view.snapshot.address)
        );
    }
    Ok(Json(value))
}

fn provider_progress(phase: ModelPhase, preview: &str) -> Value {
    if phase == ModelPhase::Streaming {
        json!({"phase":phase,"preview":preview})
    } else {
        json!({"phase":phase})
    }
}

fn execution_progress<'a>(
    path: &FsPath,
    agents: &'a [AgentSnapshot],
    cursor: u64,
) -> Result<HashMap<&'a str, Value>> {
    let mut scans: HashMap<_, _> = agents
        .iter()
        .filter_map(|agent| {
            let stopped_retry_only = agent.last_error.is_some()
                && matches!(agent.status, AgentStatus::Idle | AgentStatus::Interrupted);
            let preserve_stopped_retry = agent.last_error.is_some()
                && matches!(
                    agent.status,
                    AgentStatus::Idle | AgentStatus::Running | AgentStatus::Interrupted
                );
            if agent.status != AgentStatus::Running && !preserve_stopped_retry {
                return None;
            }
            let execution = agent
                .execution_id
                .or_else(|| stopped_retry_only.then_some(0))?;
            Some((
                agent.address.as_str(),
                ProgressScan {
                    execution,
                    preserve_stopped_retry,
                    stopped_retry_only,
                    ..ProgressScan::default()
                },
            ))
        })
        .collect();
    let Some(earliest) = scans.values().map(|scan| scan.execution).min() else {
        return Ok(HashMap::new());
    };
    let count = cursor.saturating_sub(earliest).min(PROGRESS_WINDOW as u64) as usize;
    let events = EventStore::read_page_before(path, Some(cursor.saturating_add(1)), count)?;
    for event in events.into_iter().rev() {
        if let Some(scan) = event_agent(&event.payload).and_then(|address| scans.get_mut(address)) {
            scan.observe(event.id, &event.payload);
        }
    }
    Ok(scans
        .into_iter()
        .filter_map(|(address, scan)| Some((address, scan.projection()?)))
        .collect())
}

#[derive(Default)]
struct ProgressScan {
    execution: u64,
    complete: bool,
    prepared: bool,
    settled: HashSet<String>,
    running: Vec<String>,
    retry: Option<Value>,
    preserve_stopped_retry: bool,
    stopped_retry_only: bool,
    terminal_error: bool,
}

impl ProgressScan {
    fn observe(&mut self, cursor: u64, payload: &EventPayload) {
        if self.complete || cursor <= self.execution {
            return;
        }
        match payload {
            EventPayload::ToolResult { tool_call_id, .. } => {
                self.settled.insert(tool_call_id.clone());
            }
            EventPayload::Unknown(record) if record["kind"] == "runtime.tool_dispatched" => {
                if let Some(id) = record["tool_call_id"].as_str() {
                    if !self.settled.contains(id) {
                        self.running.push(id.to_owned());
                    }
                }
            }
            EventPayload::Unknown(record) if record["kind"] == "runtime.step_prepared" => {
                if !self.terminal_error {
                    self.prepared = true;
                }
                self.complete = true;
            }
            EventPayload::TurnStarted { .. } => self.complete = true,
            EventPayload::Unknown(record)
                if record["kind"] == "runtime.operation"
                    && record["record"]["operation"] == "execution_started" =>
            {
                self.complete = true;
            }
            EventPayload::TurnCancelled { .. } => self.complete = true,
            EventPayload::Unknown(record)
                if record["kind"] == "runtime.operation"
                    && record["record"]["operation"] == "agent_stopped" =>
            {
                self.complete = true;
            }
            EventPayload::ModelResponse { .. } => self.complete = true,
            EventPayload::ModelError { .. }
                if self.preserve_stopped_retry && !self.terminal_error =>
            {
                self.terminal_error = true;
            }
            EventPayload::ModelError { .. } => self.complete = true,
            EventPayload::Unknown(record) if record["kind"] == "runtime.model_retry" => {
                let retry = crate::wire::retry_metadata(payload);
                if !self.terminal_error
                    || retry
                        .as_ref()
                        .is_some_and(|retry| retry["will_retry"] == false)
                {
                    self.retry = retry;
                }
                self.running.clear();
                self.prepared = false;
                self.complete = true;
            }
            EventPayload::ModelRecovery { .. } => {
                if !self.terminal_error {
                    self.retry = crate::wire::retry_metadata(payload);
                }
                self.running.clear();
                self.prepared = false;
                self.complete = true;
            }
            _ => {}
        }
    }

    fn projection(self) -> Option<Value> {
        if self.stopped_retry_only {
            return self
                .retry
                .filter(|retry| retry["will_retry"] == false)
                .map(|retry| json!({"phase":"retry_stopped","retry":retry}));
        }
        if let Some(retry) = self.retry {
            if retry["will_retry"] == false {
                Some(json!({"phase":"retry_stopped","retry":retry}))
            } else if retry["will_retry"] == true || retry["phase"] == "started" {
                Some(json!({"phase":"retrying_model","retry":retry}))
            } else {
                Some(json!({"phase":"retry_decision_unknown","retry":retry}))
            }
        } else if !self.running.is_empty() {
            Some(json!({"phase":"running_tools","tool_call_ids":self.running}))
        } else if self.prepared {
            Some(json!({"phase":"waiting_for_model"}))
        } else {
            None
        }
    }
}

fn selection(
    state: &AppState,
    open: Option<&Session>,
    address: &str,
    config: &kuku::AgentConfig,
) -> kuku::Result<Value> {
    if let Some(session) = open {
        return Ok(serde_json::to_value(
            session.selection_for_config(address, config.clone())?,
        )?);
    }
    let mut selection = serde_json::to_value(inspect_model(&state.config.load(), config)?)?;
    selection["tools"] = json!([]);
    Ok(selection)
}

fn agent_error(error: kuku::Error) -> ApiError {
    if matches!(error, kuku::Error::InvalidArgument(_)) {
        ApiError::new(StatusCode::NOT_FOUND, "agent_not_found", error.to_string())
    } else {
        error.into()
    }
}

#[cfg(test)]
mod retry_progress_tests {
    use kuku::event::{EventPayload, EventStore};
    use kuku::session::{AgentConfig, AgentSnapshot, AgentStatus, UsageTotals};
    use serde_json::{Value, json};
    use tempfile::tempdir;

    use super::{ProgressScan, execution_progress};

    fn retry(phase: &str, fields: Value) -> EventPayload {
        let mut event = json!({
            "kind":"runtime.model_retry","agent":"main","turn":3,"phase":phase
        });
        event
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        EventPayload::Unknown(event)
    }

    fn response() -> EventPayload {
        EventPayload::ModelResponse {
            conversation: Some("main".into()),
            turn: 3,
            ts: "2025-01-01T00:00:00Z".into(),
            request_id: "request-3".into(),
            text: "done".into(),
            thinking: None,
            stop_reason: None,
            input_tokens_total: None,
            output_tokens_total: None,
        }
    }

    fn error() -> EventPayload {
        EventPayload::ModelError {
            conversation: Some("main".into()),
            turn: 3,
            ts: "2025-01-01T00:00:00Z".into(),
            request_id: "request-3".into(),
            kind: "transport".into(),
            message: "connection failed".into(),
        }
    }

    fn interrupted_agent() -> AgentSnapshot {
        AgentSnapshot {
            address: "main".into(),
            config: AgentConfig::default(),
            active_config: None,
            status: AgentStatus::Interrupted,
            pending_inputs: 0,
            running_jobs: 0,
            admitted_task: None,
            queued_task: None,
            turn: Some(3),
            turn_started_at_ms: None,
            turn_elapsed_ms: None,
            cache_warning: None,
            last_model_route: None,
            execution_id: None,
            last_output: None,
            last_error: Some("connection failed".into()),
            permission: None,
            question: None,
            usage: UsageTotals::default(),
        }
    }

    fn persisted_failure(path: &std::path::Path, explicit_stop: bool) -> u64 {
        let mut store = EventStore::open(path).unwrap();
        store
            .append(retry(
                "stopped",
                json!({"attempt":2,"will_retry":false,"stop_reason":"attempt_limit"}),
            ))
            .unwrap();
        if explicit_stop {
            store
                .append(EventPayload::Unknown(json!({
                    "kind":"runtime.operation","record":{
                        "operation":"agent_stopped","address":"main",
                        "interruption":{"turn":3}
                    }
                })))
                .unwrap();
        } else {
            store.append(error()).unwrap();
            store
                .append(EventPayload::TurnInterrupted {
                    ts: "2025-01-01T00:00:00Z".into(),
                    conversation: "main".into(),
                    turn: 3,
                    partial_reply: None,
                    reason: "transport".into(),
                })
                .unwrap();
        }
        let settled = store
            .append(EventPayload::Unknown(json!({
                "kind":"runtime.operation","record":{
                    "operation":"turn_settled","address":"main",
                    "status":"interrupted","turn":3,"error":"connection failed"
                }
            })))
            .unwrap();
        drop(store);
        settled.id
    }

    #[test]
    fn snapshot_projection_stops_at_newer_output_or_execution() {
        let mut scan = ProgressScan {
            execution: 10,
            ..ProgressScan::default()
        };
        scan.observe(
            11,
            &retry(
                "waiting",
                json!({"attempt":2,"max_attempts":4,"will_retry":true}),
            ),
        );
        let progress = scan.projection().unwrap();
        assert_eq!("retrying_model", progress["phase"]);
        assert_eq!(2, progress["retry"]["attempt"]);

        let mut boundary = ProgressScan {
            execution: 10,
            ..ProgressScan::default()
        };
        boundary.observe(12, &response());
        boundary.observe(11, &retry("waiting", json!({"attempt":2})));
        assert_eq!(None, boundary.projection());

        let mut newer_execution = ProgressScan {
            execution: 10,
            preserve_stopped_retry: true,
            ..ProgressScan::default()
        };
        newer_execution.observe(
            12,
            &EventPayload::Unknown(json!({
                "kind":"runtime.operation","record":{"operation":"execution_started"}
            })),
        );
        newer_execution.observe(
            11,
            &retry(
                "stopped",
                json!({"attempt":2,"will_retry":false,"stop_reason":"attempt_limit"}),
            ),
        );
        assert_eq!(None, newer_execution.projection());
    }

    #[test]
    fn stopped_retry_survives_the_final_model_error_snapshot() {
        let mut scan = ProgressScan {
            execution: 10,
            preserve_stopped_retry: true,
            ..ProgressScan::default()
        };
        scan.observe(12, &error());
        scan.observe(
            11,
            &retry(
                "stopped",
                json!({"attempt":2,"will_retry":false,"stop_reason":"attempt_limit"}),
            ),
        );
        let progress = scan.projection().unwrap();
        assert_eq!("retry_stopped", progress["phase"]);
        assert_eq!(false, progress["retry"]["will_retry"]);
        assert_eq!("attempt_limit", progress["retry"]["stop_reason"]);
    }

    #[test]
    fn unknown_phase_without_will_retry_is_not_projected_as_active_or_stopped() {
        let mut scan = ProgressScan {
            execution: 10,
            ..ProgressScan::default()
        };
        scan.observe(11, &retry("future_phase", json!({"attempt":2})));
        let progress = scan.projection().unwrap();
        assert_eq!("retry_decision_unknown", progress["phase"]);
        assert_eq!(2, progress["retry"]["attempt"]);
    }

    #[test]
    fn snapshot_route_preserves_failed_retry_for_interrupted_agent() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("events.jsonl");
        let cursor = persisted_failure(&path, false);
        let agents = [interrupted_agent()];

        let progress = execution_progress(&path, &agents, cursor).unwrap();

        assert_eq!("retry_stopped", progress["main"]["phase"]);
        assert_eq!("attempt_limit", progress["main"]["retry"]["stop_reason"]);
    }

    #[test]
    fn snapshot_route_does_not_restore_unsettled_retry_for_interrupted_agent() {
        let directory = tempdir().unwrap();
        let agents = [interrupted_agent()];
        for (phase, fields) in [
            ("waiting", json!({"attempt":2,"will_retry":true})),
            ("started", json!({"attempt":2,"will_retry":true})),
            ("future_phase", json!({"attempt":2})),
        ] {
            let path = directory.path().join(format!("{phase}.jsonl"));
            let cursor = EventStore::open(&path)
                .unwrap()
                .append(retry(phase, fields))
                .unwrap()
                .id;

            let progress = execution_progress(&path, &agents, cursor).unwrap();

            assert!(!progress.contains_key("main"), "{phase}: {progress:?}");
        }
    }

    #[test]
    fn snapshot_route_does_not_restore_old_tool_progress_for_interrupted_agent() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("events.jsonl");
        let cursor = EventStore::open(&path)
            .unwrap()
            .append(EventPayload::Unknown(json!({
                "kind":"runtime.tool_dispatched","agent":"main","tool_call_id":"old-tool"
            })))
            .unwrap()
            .id;
        let agents = [interrupted_agent()];

        let progress = execution_progress(&path, &agents, cursor).unwrap();

        assert!(!progress.contains_key("main"));
    }

    #[test]
    fn snapshot_route_does_not_restore_retry_across_explicit_stop() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("events.jsonl");
        let cursor = persisted_failure(&path, true);
        let agents = [interrupted_agent()];

        let progress = execution_progress(&path, &agents, cursor).unwrap();

        assert!(!progress.contains_key("main"));
    }
}
