use std::fs;
use std::io;

use kuku_client::ConnectionOptions;

use crate::setup::{self, SetupMode};

pub(crate) async fn run_if_needed(options: &ConnectionOptions) -> io::Result<bool> {
    if options.remote.is_some() || options.token.is_some() {
        return Ok(true);
    }
    let service = match options.local_service() {
        Ok(service) => service,
        Err(_) => return Ok(true),
    };
    let status = service.status().await;
    let live_service = match status {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(_) => return Ok(true),
    };
    let missing_config = matches!(
        fs::symlink_metadata(service.config_path()),
        Err(error) if error.kind() == io::ErrorKind::NotFound
    );
    if !should_offer_setup(Ok(live_service), missing_config) {
        return Ok(true);
    }
    setup::run(options.clone(), SetupMode::FirstRun).await
}

fn should_offer_setup(status: Result<bool, ()>, missing_config: bool) -> bool {
    matches!(status, Ok(false)) && missing_config
}

#[cfg(test)]
#[path = "onboarding_tests.rs"]
mod tests;
