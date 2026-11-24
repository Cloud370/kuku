use semver::Version;

use super::{LocalConnection, LocalError, LocalService, RestartOutcome, ephemeral_address};

/// Which local build the installed client expects and whether it can replace an idle instance.
#[derive(Clone, Copy)]
pub enum BuildPolicy<'a> {
    /// Select the client build at startup, preserving any newer release already in use.
    Start(&'a str),
    /// Reconnect to the matching build without replacing a different running build.
    Reconnect(&'a str),
}

impl<'a> BuildPolicy<'a> {
    /// Keep the expected build on subsequent connections.
    pub fn for_reconnect(self) -> Self {
        Self::Reconnect(self.version())
    }

    fn version(self) -> &'a str {
        match self {
            Self::Start(version) | Self::Reconnect(version) => version,
        }
    }
}

impl LocalService {
    /// Check the build of a running service without changing its lifecycle.
    pub async fn check_running_build(&self, policy: BuildPolicy<'_>) -> Result<(), LocalError> {
        if let Some(connection) = self.status().await? {
            if connection.status.version != policy.version() {
                return Err(mismatch(connection, policy.version()));
            }
        }
        Ok(())
    }

    /// Connect to the selected build of the managed service.
    pub async fn connect_build(
        &self,
        policy: BuildPolicy<'_>,
    ) -> Result<LocalConnection, LocalError> {
        let requested = policy.version();
        let _launcher = self.acquire_launcher().await?;
        let connection = match self.wait_for_live_instance().await? {
            None => self.start(&self.config_path, ephemeral_address()).await?,
            Some(current) if current.status.version == requested => current,
            Some(current) => {
                self.check_config(&current)?;
                if matches!(policy, BuildPolicy::Reconnect(_))
                    || is_newer_release(&current.status.version, requested)
                {
                    return Err(mismatch(current, requested));
                }
                let outcome = if current.status.busy.is_empty() {
                    self.replace(current).await?
                } else {
                    RestartOutcome::Deferred(current.status)
                };
                match outcome {
                    RestartOutcome::Restarted(connection) => connection,
                    RestartOutcome::Deferred(status) => {
                        return Err(LocalError::BuildDeferred {
                            running: Box::new(status),
                            requested: requested.to_owned(),
                        });
                    }
                }
            }
        };
        if connection.status.version != requested {
            return Err(mismatch(connection, requested));
        }
        self.compatible(connection)
    }
}

fn mismatch(connection: LocalConnection, requested: &str) -> LocalError {
    LocalError::BuildMismatch {
        running: connection.status.version,
        requested: requested.to_owned(),
    }
}

fn is_newer_release(running: &str, requested: &str) -> bool {
    let release = |build: &str| Version::parse(build.split_whitespace().next()?).ok();
    release(running)
        .zip(release(requested))
        .is_some_and(|(running, requested)| running.cmp_precedence(&requested).is_gt())
}
