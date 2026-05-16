use clap::{Args, Subcommand};
use kuku::event::EventStore;
use kuku::session::{current_workspace, kuku_home, list_sessions, session_events_path};

use crate::view::{derive_final_output, render_event_brief};

#[derive(Subcommand)]
/// Subcommands for inspecting session data.
pub enum ShowCommand {
    /// Show final output from a session
    Output { session_id: String },
    /// Show events from a session
    Events {
        session_id: String,
        #[arg(short = 'v', long = "verbose")]
        verbose: bool,
    },
}

#[derive(Args)]
/// CLI arguments for listing sessions.
pub struct ListArgs {
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,
}

pub async fn run(cmd: ShowCommand) -> Result<(), Box<dyn std::error::Error>> {
    let home = kuku_home()?;
    let workspace = current_workspace()?;

    match cmd {
        ShowCommand::Output { session_id } => {
            let path = session_events_path(&home, &workspace, &session_id)?;
            let events = EventStore::replay(&path)?;
            match derive_final_output(&events) {
                Some(text) => println!("{text}"),
                None => eprintln!("no final output found in session {session_id}"),
            }
        }
        ShowCommand::Events {
            session_id,
            verbose,
        } => {
            let path = session_events_path(&home, &workspace, &session_id)?;
            let events = EventStore::replay(&path)?;
            for event in &events {
                println!("{}", render_event_brief(event, verbose));
            }
        }
    }
    Ok(())
}

pub async fn list(args: ListArgs) -> Result<(), Box<dyn std::error::Error>> {
    let home = kuku_home()?;
    let workspace = current_workspace()?;
    let sessions = list_sessions(&home, &workspace)?;
    if sessions.is_empty() {
        println!("no sessions in this workspace");
        return Ok(());
    }
    for s in &sessions {
        if args.verbose {
            println!(
                "{}  created:{}  turns:{}",
                s.session_id, s.created_at, s.turn_count
            );
        } else {
            println!("{}", s.session_id);
        }
    }
    Ok(())
}
