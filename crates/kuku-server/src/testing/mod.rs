//! Deterministic support for the server's acceptance scenarios.
//!
//! This module contains scenario inputs and test controls. Route and runtime
//! registration is enabled only in test builds behind `test-scenarios`.

mod control;
mod scenario;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub(crate) use control::router;
pub use control::{BarrierOutcome, ControlError, ScenarioControl};
pub use scenario::{
    DeterministicClock, DriverEvent, ScenarioDriverFactory, ScenarioError, ScenarioFixture,
    ScenarioIds,
};

/// Deterministic services selected by the scenario environment variables.
pub struct ScenarioRuntime {
    factory: Arc<ScenarioDriverFactory>,
}

impl ScenarioRuntime {
    /// Loads the optional scenario selection from the process environment.
    pub fn from_environment() -> Result<Option<Self>, ScenarioError> {
        Self::from_values(
            std::env::var("KUKU_TEST_SCENARIO").ok(),
            std::env::var("KUKU_TEST_SEED").ok(),
        )
    }

    fn from_values(
        scenario: Option<String>,
        seed: Option<String>,
    ) -> Result<Option<Self>, ScenarioError> {
        match (scenario, seed) {
            (None, None) => Ok(None),
            (Some(name), Some(seed)) if !name.trim().is_empty() => {
                let seed = seed.parse::<u64>().map_err(|_| {
                    ScenarioError::InvalidFixture(
                        "KUKU_TEST_SEED must be an unsigned integer".to_owned(),
                    )
                })?;
                Ok(Some(Self {
                    factory: Arc::new(ScenarioDriverFactory::from_fixture(&name, seed)?),
                }))
            }
            _ => Err(ScenarioError::InvalidFixture(
                "KUKU_TEST_SCENARIO and KUKU_TEST_SEED must be set together".to_owned(),
            )),
        }
    }

    /// Returns the production driver boundary implementation.
    pub fn factory(&self) -> Arc<ScenarioDriverFactory> {
        Arc::clone(&self.factory)
    }

    /// Returns controls bound to the active seeded fixture.
    pub fn control(&self) -> ScenarioControl {
        self.factory.control()
    }

    /// Returns the deterministic provider connectivity probe.
    pub fn provider_probe(&self) -> Arc<dyn crate::platform::ProviderProbe> {
        Arc::new(ScenarioProviderProbe)
    }
}

struct ScenarioProviderProbe;

impl crate::platform::ProviderProbe for ScenarioProviderProbe {
    fn probe<'a>(
        &'a self,
        tier: &'a crate::api::TierSummary,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<crate::api::TestProviderResult, crate::api::ApiError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            Ok(crate::api::TestProviderResult {
                api_version: crate::api::ApiVersion,
                reachable: true,
                provider: tier.provider.clone(),
                model: tier.model.clone(),
                message: None,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_selection_requires_a_complete_valid_pair() {
        assert!(ScenarioRuntime::from_values(None, None).unwrap().is_none());
        assert!(ScenarioRuntime::from_values(Some("core_task".to_owned()), None).is_err());
        assert!(ScenarioRuntime::from_values(
            Some("core_task".to_owned()),
            Some("invalid".to_owned())
        )
        .is_err());
        assert!(
            ScenarioRuntime::from_values(Some("core_task".to_owned()), Some("7".to_owned()))
                .unwrap()
                .is_some()
        );
    }
}
