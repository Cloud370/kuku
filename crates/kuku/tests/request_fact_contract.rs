use kuku::event::{
    ConversationId, CurrencyCode, DecimalCost, ExecutionScope, InteractionId, ProviderFact,
    ProviderFailureFact, ProviderFailureKind, ProviderUsage, RequestCause, RequestCompleted,
    RequestFailed, RequestId, RequestScope, RequestStarted, RunId, TaskId, TurnId, WorkspaceId,
};

fn request_scope() -> RequestScope {
    RequestScope {
        execution: ExecutionScope {
            workspace_id: WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap(),
            task_id: TaskId::parse("tsk_0123456789abcdef01234567").unwrap(),
            run_id: RunId::parse("run_0123456789abcdef01234567").unwrap(),
            turn_id: TurnId::parse("trn_0123456789abcdef01234567").unwrap(),
            conversation_id: ConversationId::parse("con_0123456789abcdef01234567").unwrap(),
            turn_index: 3,
        },
        request_id: RequestId::parse("req_0123456789abcdef01234567").unwrap(),
    }
}

#[test]
fn request_cause_is_tagged_and_parent_identity_is_not_another_scope() {
    let cause = RequestCause::InteractionResume {
        interaction_id: InteractionId::parse("int_0123456789abcdef01234567").unwrap(),
        parent_request_id: RequestId::parse("req_89abcdef0123456701234567").unwrap(),
    };
    let json = serde_json::to_value(&cause).unwrap();

    assert_eq!(json["kind"], "interaction_resume");
    assert!(json.get("execution").is_none());
    assert_eq!(serde_json::from_value::<RequestCause>(json).unwrap(), cause);
}

#[test]
fn request_lifecycle_facts_round_trip_with_nullable_normalized_metrics() {
    let scope = request_scope();
    let started = RequestStarted {
        scope: scope.clone(),
        cause: RequestCause::UserSubmission,
        provider: ProviderFact::Anthropic,
        model: "claude-sonnet".to_string(),
        started_at: "2026-07-18T12:00:00Z".to_string(),
    };
    let completed = RequestCompleted {
        scope: scope.clone(),
        usage: ProviderUsage {
            input_tokens: Some(100),
            output_tokens: Some(20),
            cached_input_tokens: None,
            cache_creation_input_tokens: Some(10),
        },
        elapsed_ms: Some(1_250),
        provider_request_id: None,
        cost: Some(DecimalCost {
            currency: CurrencyCode::Usd,
            micros: 42,
        }),
    };
    let failed = RequestFailed {
        scope,
        usage: None,
        elapsed_ms: None,
        provider_request_id: None,
        cost: None,
        failure: ProviderFailureFact {
            kind: ProviderFailureKind::ServerRestarted,
            summary: "server restarted".to_string(),
        },
    };

    for value in [
        serde_json::to_value(&started).unwrap(),
        serde_json::to_value(&completed).unwrap(),
        serde_json::to_value(&failed).unwrap(),
    ] {
        assert!(!value.to_string().contains("secret"));
    }
    assert_eq!(
        serde_json::from_value::<RequestStarted>(serde_json::to_value(&started).unwrap()).unwrap(),
        started
    );
    assert_eq!(
        serde_json::from_value::<RequestCompleted>(serde_json::to_value(&completed).unwrap())
            .unwrap(),
        completed
    );
    assert_eq!(
        serde_json::from_value::<RequestFailed>(serde_json::to_value(&failed).unwrap()).unwrap(),
        failed
    );
}

#[test]
fn request_wire_metrics_reject_values_above_json_safe_integer_max() {
    let too_large = 9_007_199_254_740_992_u64;
    let usage = serde_json::json!({
        "input_tokens": too_large,
        "output_tokens": null,
        "cached_input_tokens": null,
        "cache_creation_input_tokens": null
    });
    assert!(serde_json::from_value::<ProviderUsage>(usage).is_err());

    let mut scope = serde_json::to_value(request_scope()).unwrap();
    scope["execution"]["turn_index"] = serde_json::json!(too_large);
    assert!(serde_json::from_value::<RequestScope>(scope).is_err());
}
