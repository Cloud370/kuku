mod api {
    pub use kuku_server::api::{ContextUsage, UsageSummary};
}

#[path = "../src/context/usage_reducer.rs"]
mod usage_reducer;

use kuku::event::{
    ConversationId, ExecutionScope, ProviderUsage, RequestCompleted, RequestFailed, RequestId,
    RequestScope, RunId, TaskEvent, TaskId, TurnId, WorkspaceId,
};
use usage_reducer::UsageReducer;

fn request_id(index: u8) -> RequestId {
    RequestId::parse(format!("req_{index:024x}")).unwrap()
}

fn scope(index: u8) -> RequestScope {
    RequestScope {
        execution: ExecutionScope {
            workspace_id: WorkspaceId::parse("wsp_000000000000000000000001").unwrap(),
            task_id: TaskId::parse("tsk_000000000000000000000001").unwrap(),
            run_id: RunId::parse("run_000000000000000000000001").unwrap(),
            turn_id: TurnId::parse("trn_000000000000000000000001").unwrap(),
            conversation_id: ConversationId::parse("con_000000000000000000000001").unwrap(),
            turn_index: 1,
        },
        request_id: request_id(index),
    }
}

fn completed(index: u8, input: Option<u64>, cached: Option<u64>) -> TaskEvent {
    TaskEvent::RequestCompleted(RequestCompleted {
        scope: scope(index),
        usage: ProviderUsage {
            input_tokens: input,
            output_tokens: None,
            cached_input_tokens: cached,
            cache_creation_input_tokens: None,
        },
        elapsed_ms: None,
        provider_request_id: None,
        cost: None,
    })
}

#[test]
fn request_and_task_usage_share_the_canonical_api_shape() {
    let reducer = UsageReducer::from_lifecycle([
        completed(1, Some(100), Some(20)),
        completed(2, Some(50), Some(10)),
    ])
    .unwrap();

    let request = reducer.for_request(&request_id(1)).unwrap().unwrap();
    let task = reducer.for_task().unwrap();

    assert_eq!(1, request.request_count);
    assert_eq!(Some(100), request.input_tokens);
    assert_eq!(Some(0.2), request.cached_input_ratio);
    assert_eq!(2, task.request_count);
    assert_eq!(Some(150), task.input_tokens);
    assert_eq!(Some(0.2), task.cached_input_ratio);
}

#[test]
fn context_usage_selects_one_request_without_changing_task_totals() {
    let reducer = UsageReducer::from_lifecycle([
        completed(1, Some(100), None),
        TaskEvent::RequestFailed(RequestFailed {
            scope: scope(2),
            usage: Some(ProviderUsage {
                input_tokens: Some(12),
                output_tokens: None,
                cached_input_tokens: None,
                cache_creation_input_tokens: None,
            }),
            elapsed_ms: None,
            provider_request_id: None,
            cost: None,
            failure: kuku::event::ProviderFailureFact {
                kind: kuku::event::ProviderFailureKind::ServerRestarted,
                summary: "server restarted".to_string(),
            },
        }),
    ])
    .unwrap();

    let usage = reducer.for_context(Some(&request_id(2))).unwrap();

    assert_eq!(Some(12), usage.this_request.unwrap().input_tokens);
    assert_eq!(Some(112), usage.this_task.input_tokens);
    assert_eq!(None, usage.this_task.elapsed_ms);
}
