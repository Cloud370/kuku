//! Deterministic support for the server's acceptance scenarios.
//!
//! This module contains scenario inputs and test controls. Route and runtime
//! registration is enabled only in test builds behind `test-scenarios`.

mod control;
mod scenario;

pub use control::{BarrierOutcome, ControlError, ScenarioControl};
pub use scenario::{
    DeterministicClock, DriverEvent, ScenarioDriverFactory, ScenarioError, ScenarioFixture,
    ScenarioIds,
};
