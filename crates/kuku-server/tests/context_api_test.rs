use kuku_server::api::ContextSnapshot;

#[test]
fn current_and_historical_fixtures_have_the_same_snapshot_shape() {
    let current: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/context/v1/current.json")).unwrap();
    let historical: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/context/v1/historical.json")).unwrap();
    let _: ContextSnapshot = serde_json::from_value(current.clone()).unwrap();
    let _: ContextSnapshot = serde_json::from_value(historical.clone()).unwrap();
    assert_eq!(
        current.as_object().unwrap().keys().collect::<Vec<_>>(),
        historical.as_object().unwrap().keys().collect::<Vec<_>>()
    );
    assert!(!historical["request_history"].as_array().unwrap().is_empty());
    assert!(historical["exact_request"].is_object());
}
