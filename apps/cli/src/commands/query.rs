use kuku::{query, PermissionChoice, UiEvent};

use crate::display;

/// CLI arguments for running an interactive or print-mode query.
pub struct QueryArgs {
    pub prompt: Vec<String>,
    pub print_mode: bool,
    pub model: Option<String>,
    pub session: Option<String>,
    pub cont: bool,
}

pub async fn run(args: QueryArgs) -> Result<(), Box<dyn std::error::Error>> {
    let prompt = args.prompt.join(" ");
    let mut q = query(&prompt);
    if let Some(model) = &args.model {
        q = q.model(model.clone());
    }
    if let Some(session) = &args.session {
        q = q.session(session.clone());
    } else if args.cont {
        let home = kuku::session::kuku_home()?;
        let workspace = kuku::session::current_workspace()?;
        let sessions = kuku::session::list_sessions(&home, &workspace)?;
        let latest = sessions
            .iter()
            .max_by_key(|s| s.created_at.as_str())
            .ok_or("no sessions found")?;
        q = q.session(latest.session_id.clone());
    }

    if args.print_mode {
        let output = q.run().await?;
        println!("{}", output.text);
        return Ok(());
    }

    // Interactive mode
    let mut run = q.start().await?;
    let session_id = run.session_id().to_string();
    display::session_start(&session_id);
    let mut in_thinking = false;

    loop {
        let event = tokio::select! {
            result = run.next() => result?,
            _ = tokio::signal::ctrl_c() => {
                display::interrupted(&session_id);
                return Ok(());
            }
        };
        match event {
            Some(UiEvent::TextDelta { text }) => {
                if in_thinking {
                    display::thinking_end();
                    display::response_start();
                    in_thinking = false;
                }
                display::text_delta(&text);
            }
            Some(UiEvent::ThinkingDelta { text }) => {
                if !in_thinking {
                    display::thinking_start();
                    in_thinking = true;
                }
                display::thinking_delta(&text);
            }
            Some(UiEvent::ToolCall {
                tool_call_id: _,
                tool,
                summary,
            }) => {
                if in_thinking {
                    display::thinking_end();
                    in_thinking = false;
                }
                display::tool_call(&tool, &summary);
            }
            Some(UiEvent::ToolResult {
                tool_call_id: _,
                summary,
            }) => {
                display::tool_result(&summary);
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
                if in_thinking {
                    display::thinking_end();
                }
                println!();
                break;
            }
            Some(_) => {}
            None => {
                if in_thinking {
                    display::thinking_end();
                }
                break;
            }
        }
    }
    Ok(())
}
