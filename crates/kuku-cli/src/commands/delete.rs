use std::io::{self, Write};

use kuku_client::{SessionStatus, SessionSummary};

use crate::cli_args::{ConnectionArgs, DeleteArgs};
use crate::commands::CliResult;

/// Delete a session: `kuku delete <session_id> [-w <workspace>]`
pub async fn run(args: DeleteArgs, target: &ConnectionArgs) -> CliResult {
    let options = target.options();
    let client = target.client().await?;
    let workspace = if target.workspace.is_some() || target.remote.is_some() {
        Some(client.workspace(options.workspace()?).await?)
    } else {
        None
    };
    let sessions = client
        .sessions(workspace.as_deref(), Some(&args.session_id))
        .await?;
    let Some(session) = sessions
        .iter()
        .find(|session| session.session_id == args.session_id)
    else {
        eprintln!("session not found: {}", args.session_id);
        return Ok(());
    };
    if confirm(session)? {
        client
            .delete(&session.workspace, &session.session_id)
            .await?;
        println!("deleted {}", session.session_id);
    } else {
        println!("cancelled");
    }
    Ok(())
}

fn confirm(session: &SessionSummary) -> io::Result<bool> {
    let status = match session.status {
        SessionStatus::Active => "active",
        SessionStatus::Done => "done",
        SessionStatus::Interrupted => "interrupted",
    };
    println!(
        "Delete session '{}'?\n  title: {}\n  workspace: {}\n  turns: {}\n  status: {}",
        session.session_id,
        session.title,
        session.workspace.display(),
        session.turn_count,
        status
    );
    print!("Confirm? [y/N]: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().eq_ignore_ascii_case("y"))
}
