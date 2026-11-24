//! Shared management commands for the managed and explicit HTTP service.

use kuku_client::ConnectionOptions;
use kuku_client::local::{self, RestartOutcome};
use serde_json::json;

use crate::cli_args::{ConnectionArgs, ServerCommand, ServerCommandArgs};
use crate::commands::CliResult;

/// Show status or request a normal restart without requiring Session protocol compatibility.
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
        ServerCommand::Restart => match options.local_service()?.restart().await? {
            RestartOutcome::Deferred(status) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({"state": "deferred", "service": status}))?
                );
                return Err("service restart deferred while work or cleanup is active; check `kuku server status` and retry after it settles".into());
            }
            RestartOutcome::Restarted(connection) => {
                json!({"state": "restarted", "service": connection.status()})
            }
        },
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
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
