use std::path::PathBuf;

use kuku::session::{current_workspace, kuku_home, session_events_path, Session};

use crate::cli_args::ShowArgs;

/// Show the last durable response from one agent without opening an executor.
pub async fn run(args: ShowArgs) -> Result<(), Box<dyn std::error::Error>> {
    let home = match args.data_dir {
        Some(path) => PathBuf::from(path),
        None => kuku_home()?,
    };
    let workspace = match args.workspace {
        Some(path) => PathBuf::from(path),
        None => current_workspace()?,
    };
    let path = session_events_path(&home, &workspace, &args.session_id)?;
    let address = args.conversation.as_deref().unwrap_or("main");
    let agent = Session::read_agents(&path)?
        .into_iter()
        .find(|agent| agent.address == address)
        .ok_or_else(|| {
            format!(
                "agent '{address}' was not found in session {}",
                args.session_id
            )
        })?;
    if args.json {
        println!("{}", serde_json::to_string(&agent)?);
        return Ok(());
    }
    match agent.last_output {
        Some(text) => println!("{text}"),
        None => eprintln!(
            "no final output found in session {} agent {address}",
            args.session_id
        ),
    }
    Ok(())
}
