use kuku::event::EventStore;
use kuku::session::{current_workspace, kuku_home, session_events_path};

use crate::cli_args::EventsArgs;
use crate::display::render_event_brief;

pub async fn run(args: EventsArgs) -> Result<(), Box<dyn std::error::Error>> {
    let home = kuku_home()?;
    let workspace = current_workspace()?;
    let path = session_events_path(&home, &workspace, &args.session_id)?;
    let events = EventStore::replay(&path)?;
    for event in &events {
        println!("{}", render_event_brief(event, args.verbose));
    }
    Ok(())
}
