use clap::Args;
use kuku::{query, PermissionChoice, UiEvent};

use crate::display;

#[derive(Args)]
pub struct QueryArgs {
    /// The prompt to run
    pub prompt: String,

    /// Print mode: output final text only, deny all permission requests
    #[arg(short = 'p')]
    pub print_mode: bool,

    /// Model alias
    #[arg(long = "model")]
    pub model: Option<String>,

    /// Verbose output
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,
}

pub async fn run(args: QueryArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut q = query(&args.prompt);
    if let Some(model) = &args.model {
        q = q.model(model.clone());
    }

    if args.print_mode {
        let output = q.run().await?;
        println!("{}", output.text);
        if args.verbose {
            eprintln!("session: {}", output.session_id);
        }
        return Ok(());
    }

    // Interactive mode
    let mut run = q.start().await?;
    let session_id = run.session_id().to_string();

    loop {
        match run.next().await? {
            Some(UiEvent::TextDelta { text }) => {
                display::text_delta(&text);
            }
            Some(UiEvent::ToolCall {
                tool_call_id,
                tool,
                summary,
            }) => {
                let extra = if args.verbose {
                    tool_call_id
                } else {
                    String::new()
                };
                display::tool_call(args.verbose, &tool, &summary, &extra);
            }
            Some(UiEvent::ToolResult {
                tool_call_id: _,
                summary,
            }) => {
                display::tool_result(args.verbose, &summary, "");
            }
            Some(UiEvent::PermissionRequested { request }) => {
                let prompt = display::permission_prompt(&request.tool, &request.summary);
                use std::io::{self, Write};
                print!("{prompt} ");
                io::stdout().flush()?;
                let mut input = String::new();
                io::stdin().read_line(&mut input)?;
                let choice = match input.trim() {
                    "y" | "" => PermissionChoice::Once,
                    "n" => PermissionChoice::Deny,
                    "session" => PermissionChoice::Session,
                    "project" => PermissionChoice::Project,
                    _ => PermissionChoice::Deny,
                };
                let _ = run.decide(&request.id, choice).await?;
            }
            Some(UiEvent::Done { output: _ }) => {
                display::session_separator(&session_id);
                break;
            }
            Some(_) => {}
            None => break,
        }
    }
    Ok(())
}
