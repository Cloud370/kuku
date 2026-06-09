use kuku::event::EventStore;
use kuku::session::{current_workspace, kuku_home, session_events_path};

use crate::cli_args::EventsArgs;
use crate::display::{filter_events_for_conversation, render_event_brief};

/// Show events from a session: `kuku events <session_id> [-v]`
pub async fn run(args: EventsArgs) -> Result<(), Box<dyn std::error::Error>> {
    let home = kuku_home()?;
    let workspace = current_workspace()?;
    let path = session_events_path(&home, &workspace, &args.session_id)?;
    let events = EventStore::replay(&path)?;
    let filtered = match args.conversation.as_deref() {
        Some(conversation) => filter_events_for_conversation(&events, conversation),
        None => events.iter().collect(),
    };
    for event in filtered {
        println!("{}", render_event_brief(event, args.verbose));
    }
    Ok(())
}
