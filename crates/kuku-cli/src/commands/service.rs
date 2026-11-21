//! Shared management commands for the managed and explicit HTTP service.

use kuku_client::local::{self, BuildPolicy, LocalError, RestartOutcome, ServiceStatus};
use kuku_client::ConnectionOptions;
use serde_json::json;

use crate::cli_args::{ConnectionArgs, ServerCommand, ServerCommandArgs};
use crate::commands::CliResult;

/// Manage the service without requiring an existing Session connection.
pub async fn manage(command: ServerCommand, options: ConnectionOptions) -> CliResult {
    let result = match command {
        ServerCommand::Status => {
            if let Some(endpoint) = &options.remote {
                let token = options
                    .token
                    .or_else(|| std::env::var("KUKU_TOKEN").ok())
                    .ok_or("remote authentication requires --token or KUKU_TOKEN")?;
                json!(local::service_status(endpoint, &token).await?)
            } else {
                match options.local_service()?.status().await? {
                    Some(connection) => json!(connection.status()),
                    None => json!({"state": "stopped"}),
                }
            }
        }
        ServerCommand::Start => {
            let connection = match options
                .local_service()?
                .connect_build(BuildPolicy::Start(kuku_server::BUILD_VERSION))
                .await
            {
                Ok(connection) => connection,
                Err(error) => {
                    if let LocalError::BuildDeferred { running, .. } = &error {
                        print_deferred(running)?;
                    }
                    return Err(error.into());
                }
            };
            json!({"state": "ready", "service": connection.status()})
        }
        ServerCommand::Restart => {
            let connection = restarted(options.local_service()?.restart().await?)?;
            json!({"state": "restarted", "service": connection.status()})
        }
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn restarted(
    outcome: RestartOutcome,
) -> Result<local::LocalConnection, Box<dyn std::error::Error>> {
    match outcome {
        RestartOutcome::Restarted(connection) => Ok(connection),
        RestartOutcome::Deferred(status) => {
            print_deferred(&status)?;
            Err("service restart deferred while work or cleanup is active; check `kuku server status` and retry after it settles".into())
        }
    }
}

fn print_deferred(status: &ServiceStatus) -> CliResult {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"state": "deferred", "service": status}))?
    );
    Ok(())
}

pub(super) async fn standalone(args: ServerCommandArgs, target: &ConnectionArgs) -> CliResult {
    if let Some(command) = args.command {
        return manage(command, target.options()).await;
    }
    if target.remote.is_some() {
        return Err(
            "start the foreground server on its host with `kuku server --listen ADDRESS`".into(),
        );
    }
    kuku_server::service::serve(args.into_server_args(target), None).await
}

pub(super) async fn managed(args: ServerCommandArgs, target: &ConnectionArgs) -> CliResult {
    let mut args = args.into_server_args(target);
    args.managed = true;
    kuku_server::service::serve(args, None).await
}
