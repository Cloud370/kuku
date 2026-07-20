use kuku::context::{UsageAggregate, UsageReductionError};
use kuku::event::{
    ConversationId, CurrencyCode, DecimalCost, ExecutionScope, ProviderFact, ProviderFailureFact,
    ProviderFailureKind, ProviderUsage, RequestCause, RequestCompleted, RequestFailed, RequestId,
    RequestScope, RequestStarted, RunId, TaskEvent, TaskId, TurnId, WorkspaceId,
};

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

fn started(index: u8) -> TaskEvent {
    TaskEvent::RequestStarted(RequestStarted {
        scope: scope(index),
        cause: RequestCause::UserSubmission,
        provider: ProviderFact::Anthropic,
        model: "model-a".to_string(),
        started_at: "2026-07-21T00:00:00Z".to_string(),
    })
}

fn completed(
    index: u8,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    elapsed_ms: Option<u64>,
    cost_micros: Option<u64>,
) -> TaskEvent {
    TaskEvent::RequestCompleted(RequestCompleted {
        scope: scope(index),
        usage: ProviderUsage {
            input_tokens,
            output_tokens,
            cached_input_tokens,
            cache_creation_input_tokens,
        },
        elapsed_ms,
        provider_request_id: None,
        cost: cost_micros.map(|micros| DecimalCost {
            currency: CurrencyCode::Usd,
            micros,
        }),
    })
}

fn failed(index: u8, usage: Option<ProviderUsage>, elapsed_ms: Option<u64>) -> TaskEvent {
    TaskEvent::RequestFailed(RequestFailed {
        scope: scope(index),
        usage,
        elapsed_ms,
        provider_request_id: None,
        cost: None,
        failure: ProviderFailureFact {
            kind: ProviderFailureKind::Transport,
            summary: "provider transport failed".to_string(),
        },
    })
}

#[test]
fn terminal_lifecycle_is_the_only_usage_source_and_replay_is_idempotent() {
    let terminal = completed(1, Some(100), None, Some(80), None, Some(40), None);
    let aggregate =
        UsageAggregate::from_lifecycle([started(1), terminal.clone(), terminal]).unwrap();

    let summary = aggregate.summary().unwrap();

    assert_eq!(1, summary.request_count);
    assert_eq!(Some(100), summary.input_tokens);
    assert_eq!(Some(0.8), summary.cached_input_ratio);
    assert_eq!(None, summary.cache_creation_input_tokens);
}

#[test]
fn failed_request_retains_only_metrics_the_provider_reported() {
    let aggregate = UsageAggregate::from_lifecycle([failed(
        2,
        Some(ProviderUsage {
            input_tokens: Some(12),
            output_tokens: None,
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
        }),
        Some(25),
    )])
    .unwrap();

    let summary = aggregate.summary().unwrap();

    assert_eq!(Some(12), summary.input_tokens);
    assert_eq!(None, summary.output_tokens);
    assert_eq!(Some(25), summary.elapsed_ms);
}

#[test]
fn restart_failure_has_no_invented_elapsed_time() {
    let restarted = TaskEvent::RequestFailed(RequestFailed {
        scope: scope(3),
        usage: None,
        elapsed_ms: None,
        provider_request_id: None,
        cost: None,
        failure: ProviderFailureFact {
            kind: ProviderFailureKind::ServerRestarted,
            summary: "server restarted".to_string(),
        },
    });
    let aggregate = UsageAggregate::from_lifecycle([restarted]).unwrap();

    let summary = aggregate.summary().unwrap();

    assert_eq!(1, summary.request_count);
    assert_eq!(None, summary.elapsed_ms);
    assert_eq!(None, summary.input_tokens);
}

#[test]
fn task_summary_sums_reported_values_without_zero_filling_missing_metrics() {
    let aggregate = UsageAggregate::from_lifecycle([
        completed(1, Some(100), Some(20), Some(25), None, Some(40), Some(5)),
        failed(
            2,
            Some(ProviderUsage {
                input_tokens: Some(50),
                output_tokens: None,
                cached_input_tokens: Some(5),
                cache_creation_input_tokens: Some(10),
            }),
            None,
        ),
    ])
    .unwrap();

    let summary = aggregate.summary().unwrap();

    assert_eq!(2, summary.request_count);
    assert_eq!(Some(150), summary.input_tokens);
    assert_eq!(Some(20), summary.output_tokens);
    assert_eq!(Some(30), summary.cached_input_tokens);
    assert_eq!(Some(10), summary.cache_creation_input_tokens);
    assert_eq!(Some(0.2), summary.cached_input_ratio);
    assert_eq!(Some(40), summary.elapsed_ms);
    assert_eq!(
        Some(DecimalCost {
            currency: CurrencyCode::Usd,
            micros: 5,
        }),
        summary.cost
    );
}

#[test]
fn conflicting_terminal_facts_are_typed_ledger_corruption() {
    let error = UsageAggregate::from_lifecycle([
        completed(1, Some(10), None, None, None, None, None),
        completed(1, Some(11), None, None, None, None, None),
    ])
    .unwrap_err();

    assert!(matches!(
        error,
        UsageReductionError::ConflictingTerminal { request_id: actual_request_id }
            if actual_request_id == request_id(1)
    ));
}
