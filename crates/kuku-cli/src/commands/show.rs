use crate::cli_args::{ConnectionArgs, ShowArgs};
use crate::commands::CliResult;

/// Show the last durable response from one agent without opening an executor.
pub async fn run(args: ShowArgs, options: &ConnectionArgs) -> CliResult {
    let connection = options.connect().await?;
    let address = args.conversation.as_deref().unwrap_or("main");
    let target = connection.target(&args.session_id, address);
    let agent = connection.client.snapshot(&target).await?.agent;
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
