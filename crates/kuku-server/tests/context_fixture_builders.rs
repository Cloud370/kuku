#![allow(dead_code)]

mod common;

use common::context_fixtures::{
    agent_thread, assert_public_fixture, context_catalog, empty_snapshot, historical_snapshot,
    sdk_context_breakdown, snapshot_with_context_evidence,
};

#[test]
fn context_fixtures_round_trip_through_g1_contract_types() {
    let values = [
        serde_json::to_value(empty_snapshot()).unwrap(),
        serde_json::to_value(snapshot_with_context_evidence()).unwrap(),
        serde_json::to_value(historical_snapshot()).unwrap(),
        serde_json::to_value(agent_thread()).unwrap(),
        serde_json::to_value(context_catalog()).unwrap(),
    ];

    for value in values {
        let encoded = serde_json::to_string(&value).unwrap();
        let decoded: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, value);
    }
}

#[test]
fn context_fixtures_preserve_sdk_fact_provenance_and_public_boundaries() {
    let facts = sdk_context_breakdown();
    assert_eq!(facts.skills[0].source.id, "source:project");
    assert_eq!(facts.observations[0].tool_call_id, "call_read_file");
    assert_eq!(facts.token_estimate, Some(1_024));

    assert_public_fixture(&empty_snapshot());
    assert_public_fixture(&snapshot_with_context_evidence());
    assert_public_fixture(&historical_snapshot());
    assert_public_fixture(&agent_thread());
    assert_public_fixture(&context_catalog());
}
