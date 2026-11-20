use std::io;

use kuku_client::ConnectionOptions;

use crate::setup::{self, SetupMode};

pub(crate) async fn run_if_needed(options: &ConnectionOptions) -> io::Result<bool> {
    if options.remote.is_some() || options.token.is_some() {
        return Ok(true);
    }
    setup::run(options.clone(), SetupMode::FirstRun).await
}

#[cfg(test)]
#[path = "onboarding_tests.rs"]
mod tests;
