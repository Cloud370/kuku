#![cfg(feature = "test-scenarios")]

use kuku_server::testing::{BarrierOutcome, ScenarioControl, ScenarioDriverFactory};

#[test]
fn scenario_fixture_is_provider_input_not_a_projection_script() {
    let factory = ScenarioDriverFactory::from_fixture("core_task", 7).unwrap();
    assert!(!factory.fixture_source().contains("TaskProjection"));
    assert!(!factory.fixture_source().contains("ledger"));
}

#[test]
fn equal_seed_produces_equal_deterministic_driver_events() {
    let mut left = ScenarioDriverFactory::from_fixture("core_task", 7).unwrap();
    let mut right = ScenarioDriverFactory::from_fixture("core_task", 7).unwrap();
    let left_events: Vec<_> = std::iter::from_fn(|| left.next_event()).collect();
    let right_events: Vec<_> = std::iter::from_fn(|| right.next_event()).collect();
    assert_eq!(left_events, right_events);
}

#[tokio::test]
async fn control_releases_only_declared_barriers() {
    let control = ScenarioControl::new(["after-tool"]);
    control.release("after-tool").await.unwrap();
    assert_eq!(
        control.wait("after-tool").await.unwrap(),
        BarrierOutcome::Released
    );
}
