mod common;
#[path = "query_runtime/handoff.rs"]
mod query_runtime_handoff;
#[cfg(unix)]
#[path = "query_runtime/mutator_ordering.rs"]
mod query_runtime_mutator_ordering;

use common::{anthropic_sse_response, test_config, TestEnv};

use httpmock::prelude::{HttpMockRequest, MockServer, POST};
use kuku::agent::registry::AgentRegistry;
use kuku::context::replay::rebuild_history;
use kuku::conversation::address::ConversationAddress;
use kuku::event::{EventPayload, EventStore};
use kuku::log::{LogLevel, LogRecord, LogScope};
use kuku::prompt::builtin_prompt_catalog;
use kuku::{query, Error, PermissionChoice, PermissionRequest, Provider, Run, UiEvent};

async fn next_permission_request(run: &mut Run) -> PermissionRequest {
    let mut event = run.next().await.unwrap().expect("event");
    while !matches!(event, UiEvent::PermissionRequested { .. }) {
        event = run.next().await.unwrap().expect("event");
    }
    match event {
        UiEvent::PermissionRequested { request } => request,
        _ => unreachable!(),
    }
}

async fn next_tool_end(run: &mut Run, tool_call_id: &str) -> UiEvent {
    loop {
        let event = run.next().await.unwrap().expect("event");
        if matches!(&event, UiEvent::ToolEnd { id, .. } if id == tool_call_id) {
            return event;
        }
    }
}

fn anthro_with_agents(query_text: &str, server: &MockServer) -> kuku::query::Query {
    query(query_text)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .agents(
            AgentRegistry::builder()
                .builtins(&builtin_prompt_catalog())
                .build(),
        )
}

fn request_body_contains(req: &HttpMockRequest, text: &str) -> bool {
    req.body.as_ref().is_some_and(|body| {
        body.windows(text.len())
            .any(|window| window == text.as_bytes())
    })
}

fn assert_failed_turn_facts(events: &[kuku::event::StoredEvent], turn: u64) {
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ModelError { turn: event_turn, .. } if *event_turn == turn
    )));
    assert_single_terminal_kind(events, turn, "turn.interrupted");
}

fn assert_single_terminal_kind(
    events: &[kuku::event::StoredEvent],
    turn: u64,
    expected_kind: &str,
) {
    let terminal_events: Vec<&kuku::event::StoredEvent> = events
        .iter()
        .filter(|event| match &event.payload {
            EventPayload::TurnCompleted {
                conversation,
                turn: event_turn,
                ..
            }
            | EventPayload::TurnCancelled {
                conversation,
                turn: event_turn,
                ..
            }
            | EventPayload::TurnInterrupted {
                conversation,
                turn: event_turn,
                ..
            } => *event_turn == turn && conversation == "main",
            _ => false,
        })
        .collect();

    assert_eq!(
        terminal_events.len(),
        1,
        "expected exactly one terminal event"
    );
    assert_eq!(terminal_events[0].payload.kind_name(), expected_kind);
}

fn list_event_files(kuku_home: &std::path::Path) -> Vec<std::path::PathBuf> {
    fn visit(dir: &std::path::Path, paths: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, paths);
            } else if path.file_name().is_some_and(|name| name == "events.jsonl") {
                paths.push(path);
            }
        }
    }

    let mut paths = Vec::new();
    let root = kuku_home.join("p");
    if root.exists() {
        visit(&root, &mut paths);
    }
    paths
}

include!("query_runtime/lifecycle.rs");
include!("query_runtime/context.rs");
include!("query_runtime/diagnostics.rs");
include!("query_runtime/agents.rs");
include!("query_runtime/permissions.rs");
include!("query_runtime/permission_recovery.rs");
