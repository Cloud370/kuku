use kuku_server::api::ContextSnapshot;

#[test]
fn reconnect_fixture_preserves_task_revision_and_historical_exact_request() {
    let before: ContextSnapshot =
        serde_json::from_str(include_str!("fixtures/context/v1/historical.json")).unwrap();
    let after: ContextSnapshot =
        serde_json::from_value(serde_json::to_value(&before).unwrap()).unwrap();
    assert_eq!(before.task_revision, after.task_revision);
    assert_eq!(before.selected_request, after.selected_request);
    assert!(after.exact_request.is_some());
}
