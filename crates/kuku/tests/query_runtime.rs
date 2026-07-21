mod common;

use common::{anthropic_sse_response, test_config, TestEnv};

use httpmock::prelude::*;
use kuku::agent::registry::AgentRegistry;
use kuku::context::replay::rebuild_history;
use kuku::conversation::address::ConversationAddress;
use kuku::event::{EventPayload, EventStore, RequestCause, TaskEvent, TaskLedgerRecord};
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

include!("query_runtime/session_and_logging.rs");
include!("query_runtime/skills_and_agents.rs");
include!("query_runtime/permissions.rs");
include!("query_runtime/instructions.rs");
