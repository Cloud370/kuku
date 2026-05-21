use kuku::event::EventStore;
use kuku::session::{current_workspace, kuku_home, session_events_path};

use crate::cli_args::ShowArgs;
use crate::display::derive_final_output;

/// Show final output from a session: `kuku show <session_id>`
pub async fn run(args: ShowArgs) -> Result<(), Box<dyn std::error::Error>> {
    let home = kuku_home()?;
    let workspace = current_workspace()?;
    let path = session_events_path(&home, &workspace, &args.session_id)?;
    let events = EventStore::replay(&path)?;
    match derive_final_output(&events) {
        Some(text) => println!("{text}"),
        None => eprintln!("no final output found in session {}", args.session_id),
    }
    Ok(())
}
