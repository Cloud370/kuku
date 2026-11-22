use std::io;

use kuku_client::ConnectionOptions;

use crate::setup;

pub(crate) async fn required(options: &ConnectionOptions) -> io::Result<bool> {
    if options.remote.is_some() || options.token.is_some() {
        return Ok(false);
    }
    setup::first_run_required(options.clone()).await
}

#[cfg(test)]
#[path = "onboarding_tests.rs"]
mod tests;
