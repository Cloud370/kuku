use std::path::PathBuf;

use kuku::event::EventStore;
use kuku::session::{current_workspace, kuku_home, session_events_path};

use crate::cli_args::EventsArgs;
use crate::display::{filter_events_for_conversation, render_event_brief};

/// Read a bounded page of durable events without advancing an agent.
pub async fn run(args: EventsArgs) -> Result<(), Box<dyn std::error::Error>> {
    if !(1..=256).contains(&args.limit) {
        return Err("event limit must be between 1 and 256".into());
    }
    let home = match args.data_dir {
        Some(path) => PathBuf::from(path),
        None => kuku_home()?,
    };
    let workspace = match args.workspace {
        Some(path) => PathBuf::from(path),
        None => current_workspace()?,
    };
    let path = session_events_path(&home, &workspace, &args.session_id)?;
    if !path.exists() {
        return Err(format!("session '{}' does not exist", args.session_id).into());
    }
    let events = EventStore::read_page(&path, args.after, args.limit)?;
    let next_after = events.last().map(|event| event.id).unwrap_or(args.after);
    let filtered = match args.conversation.as_deref() {
        Some(address) => filter_events_for_conversation(&events, address),
        None => events.iter().collect(),
    };
    for event in filtered {
        if args.json {
            println!("{}", serde_json::to_string(event)?);
        } else {
            println!("{}", render_event_brief(event, args.verbose));
        }
    }
    eprintln!("next event cursor: {next_after}");
    Ok(())
}
