use crate::cli_args::{ConnectionArgs, EventsArgs};
use crate::commands::CliResult;
use crate::display::{filter_events_for_conversation, render_event_brief};

/// Read a bounded page of durable events without advancing an agent.
pub async fn run(args: EventsArgs, options: &ConnectionArgs) -> CliResult {
    if !(1..=256).contains(&args.limit) {
        return Err("event limit must be between 1 and 256".into());
    }
    let connection = options.connect().await?;
    let page = connection
        .client
        .events(
            &connection.workspace,
            &args.session_id,
            None,
            args.after,
            args.limit,
        )
        .await?;
    let filtered = match args.conversation.as_deref() {
        Some(address) => filter_events_for_conversation(&page.events, address),
        None => page.events.iter().collect(),
    };
    for event in filtered {
        if args.json {
            println!("{}", serde_json::to_string(event)?);
        } else {
            println!("{}", render_event_brief(event, args.verbose));
        }
    }
    eprintln!("next event cursor: {}", page.next_after);
    Ok(())
}
