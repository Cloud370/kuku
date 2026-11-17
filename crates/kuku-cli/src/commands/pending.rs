//! Recover script commands whose HTTP acknowledgement may have been lost.

use kuku_client::{Acknowledgement, Command, Connection};

use crate::cli_args::{ConnectionArgs, PendingArgs, PendingCommand, PendingTargetArgs};
use crate::commands::CliResult;

/// Reconcile, retry, or discard a privately stored command for this server and workspace.
pub async fn run(args: PendingArgs, options: &ConnectionArgs) -> CliResult {
    let connection = options.connect().await?;
    match args.command.unwrap_or(PendingCommand::List) {
        PendingCommand::List => list(&connection).await,
        PendingCommand::Resend(target) => {
            let command = require(&connection, target)?;
            let acknowledgement = match connection
                .pending
                .reconcile(&connection.client, &command)
                .await?
            {
                Some(acknowledgement) => acknowledgement,
                None => {
                    connection
                        .pending
                        .submit_saved(&connection.client, &command)
                        .await?
                }
            };
            print_receipt(&command, &acknowledgement);
            Ok(())
        }
        PendingCommand::Discard(target) => {
            let command = require(&connection, target)?;
            match connection
                .pending
                .reconcile(&connection.client, &command)
                .await
            {
                Ok(Some(acknowledgement)) => print_receipt(&command, &acknowledgement),
                Ok(None) => discard(&connection, &command)?,
                Err(error) if error.code() == Some("command_conflict") => {
                    discard(&connection, &command)?
                }
                Err(error) => return Err(error.into()),
            }
            Ok(())
        }
    }
}

pub(super) fn find(
    connection: &Connection,
    id: &str,
    session: Option<&str>,
) -> Result<Option<Command>, Box<dyn std::error::Error>> {
    let mut matching = connection.pending.list()?.into_iter().filter(|command| {
        command.target.workspace == connection.workspace
            && command.command_id == id
            && session.is_none_or(|id| id == command.target.session_id)
    });
    let first = matching.next();
    if matching.next().is_some() {
        return Err(format!(
            "pending command {id:?} is in multiple Sessions. Select one with --session"
        )
        .into());
    }
    Ok(first)
}

fn require(
    connection: &Connection,
    target: PendingTargetArgs,
) -> Result<Command, Box<dyn std::error::Error>> {
    find(connection, &target.command_id, target.session.as_deref())?.ok_or_else(|| {
        format!(
            "pending command {:?} was not found in this workspace",
            target.command_id
        )
        .into()
    })
}

async fn list(connection: &Connection) -> CliResult {
    let mut found = false;
    let mut failure = None;
    for command in connection
        .pending
        .list()?
        .into_iter()
        .filter(|command| command.target.workspace == connection.workspace)
    {
        found = true;
        match connection
            .pending
            .reconcile(&connection.client, &command)
            .await
        {
            Ok(Some(acknowledgement)) => print_receipt(&command, &acknowledgement),
            Ok(None) => println!(
                "pending command {} in session {} agent {}",
                command.command_id, command.target.session_id, command.target.agent
            ),
            Err(error) => {
                eprintln!(
                    "pending command {} in session {} agent {}: {error}",
                    command.command_id, command.target.session_id, command.target.agent
                );
                failure = Some(error);
            }
        }
    }
    if !found {
        println!("no unconfirmed commands in this workspace");
    }
    failure.map_or(Ok(()), |error| Err(error.into()))
}

fn discard(connection: &Connection, command: &Command) -> CliResult {
    connection.pending.remove(command)?;
    println!(
        "discarded command {} in session {}",
        command.command_id, command.target.session_id
    );
    Ok(())
}

fn print_receipt(command: &Command, acknowledgement: &Acknowledgement) {
    println!(
        "command {} committed in session {} agent {} at event {}",
        command.command_id,
        command.target.session_id,
        command.target.agent,
        acknowledgement.receipt.event_id
    );
}
