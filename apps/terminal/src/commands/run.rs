use std::io::{self, Write};
use std::time::Instant;

use kuku::{query, PermissionChoice, UiEvent};

use crate::cli_args::RunArgs;
use crate::display::{Display, OutputLine, RenderMode};

fn resolve_config_path(
    custom: Option<&str>,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    if let Some(p) = custom {
        return Ok(std::path::PathBuf::from(p));
    }
    let home = kuku::session::kuku_home()?;
    Ok(home.join("config.toml"))
}

fn close_thinking(
    in_thinking: &mut bool,
    thinking_start: &mut Option<Instant>,
    display: &mut Display,
    use_stream_json: bool,
) {
    if *in_thinking {
        *in_thinking = false;
        let elapsed = thinking_start
            .take()
            .map(|s| s.elapsed())
            .unwrap_or_default();
        if !use_stream_json {
            println!("{}", display.thinking_end(elapsed));
        }
    }
}

/// Non-interactive run: `kuku run "prompt" [flags]`
pub async fn run(args: RunArgs) -> Result<(), Box<dyn std::error::Error>> {
    let prompt = args.prompt.join(" ");

    let config_path = resolve_config_path(args.config.as_deref())?;
    if !config_path.exists() {
        eprintln!("error: config file not found: {}", config_path.display());
        eprintln!("hint: run `kuku init` to initialize");
        std::process::exit(1);
    }

    let cfg = kuku::config::load_config(&config_path)
        .and_then(|f| f.resolve())
        .map_err(|e| format!("config error: {e}"))?;

    let tier_name = args
        .model
        .clone()
        .unwrap_or_else(|| cfg.default_tier().to_string());
    let model_name = cfg
        .tier(&tier_name)
        .map(|t| t.model.clone())
        .unwrap_or_else(|| tier_name.clone());

    use std::io::IsTerminal;
    let mode = if args.raw || !std::io::stdout().is_terminal() {
        RenderMode::Raw
    } else {
        RenderMode::Pretty
    };
    let think_level_str = cfg
        .tier(&tier_name)
        .map(|t| t.think.as_str())
        .unwrap_or("medium");
    let mut display = match mode {
        RenderMode::Pretty => Display::new(args.show_thinking, think_level_str),
        RenderMode::Raw => Display::new_raw(args.show_thinking, think_level_str),
    };

    let mut previous_input_tokens: u64 = 0;

    let mut q = query(&prompt).config_path(config_path);
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
        let session_id = latest.session_id.clone();
        q = q.session(session_id.clone());

        if let Ok(events_path) = kuku::session::session_events_path(&home, &workspace, &session_id)
        {
            if let Ok(events) = kuku::event::EventStore::replay(&events_path) {
                for event in events.iter().rev() {
                    if let kuku::event::EventPayload::ModelResponse { usage, .. } = &event.payload {
                        if let Some(tokens) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
                            previous_input_tokens = tokens;
                            break;
                        }
                    }
                }
            }
        }
    }

    // JSON single-result path: use run(), output one final JSON line
    if args.json {
        let output = q.run().await?;
        let line = OutputLine::session_completed(
            output.session_id,
            tier_name.clone(),
            model_name.clone(),
            0,
            0,
            0,
            0,
        );
        println!("{}", line.to_json_line());
        return Ok(());
    }

    let use_stream_json = args.stream_json;
    let mut run = q.start().await?;
    let session_id = run.session_id().to_string();

    let prev_tokens = if previous_input_tokens > 0 {
        Some(previous_input_tokens)
    } else {
        None
    };
    if use_stream_json {
        println!(
            "{}",
            OutputLine::session_started(
                session_id.clone(),
                tier_name.clone(),
                model_name.clone(),
                prev_tokens,
            )
            .to_json_line()
        );
    } else {
        println!(
            "{}",
            display.session_start(&session_id, &tier_name, &model_name)
        );
        if previous_input_tokens > 0 {
            println!("{}", display.context_previous(previous_input_tokens));
        }
    }

    let session_start = Instant::now();
    let mut in_thinking = false;
    let mut thinking_start: Option<Instant> = None;
    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;
    let mut current_turn: u64 = 0;

    loop {
        let event = tokio::select! {
            result = run.next() => result?,
            _ = tokio::signal::ctrl_c() => {
                if use_stream_json {
                    println!("{}", OutputLine::session_interrupted(
                        session_id.clone(), current_turn,
                    ).to_json_line());
                } else {
                    eprintln!("{}", display.session_interrupted(&session_id, current_turn));
                }
                std::process::exit(130);
            }
        };

        match event {
            Some(UiEvent::TextDelta { text }) => {
                close_thinking(
                    &mut in_thinking,
                    &mut thinking_start,
                    &mut display,
                    use_stream_json,
                );
                if use_stream_json {
                    println!("{}", OutputLine::text_delta(text).to_json_line());
                } else {
                    print!("{text}");
                }
            }
            Some(UiEvent::ThinkingDelta { text }) => {
                if !in_thinking {
                    in_thinking = true;
                    thinking_start = Some(Instant::now());
                    if !use_stream_json {
                        println!();
                        println!("{}", display.thinking_start());
                    }
                }
                if !use_stream_json {
                    if let Some(rendered) = display.thinking_line(&text) {
                        print!("{rendered}");
                    }
                }
            }
            Some(UiEvent::ToolCall {
                tool_call_id,
                tool,
                summary,
            }) => {
                close_thinking(
                    &mut in_thinking,
                    &mut thinking_start,
                    &mut display,
                    use_stream_json,
                );
                if use_stream_json {
                    println!(
                        "{}",
                        OutputLine::tool_call(
                            tool,
                            tool_call_id,
                            summary,
                            serde_json::Value::Null,
                        )
                        .to_json_line()
                    );
                } else {
                    println!("\n{}", display.tool_call(&tool, &summary, &tool_call_id));
                }
            }
            Some(UiEvent::ToolResult {
                tool_call_id,
                status,
                summary,
                structured: _,
            }) => {
                if use_stream_json {
                    println!(
                        "{}",
                        OutputLine::tool_result(tool_call_id, status, summary, None, false)
                            .to_json_line()
                    );
                } else {
                    println!("{}", display.tool_result(&status, &summary, &tool_call_id));
                }
            }
            Some(UiEvent::PermissionRequested { request }) => {
                close_thinking(
                    &mut in_thinking,
                    &mut thinking_start,
                    &mut display,
                    use_stream_json,
                );
                if args.auto_yes || use_stream_json {
                    let _ = run.decide(&request.id, PermissionChoice::Once).await?;
                    if use_stream_json {
                        println!(
                            "{}",
                            OutputLine::permission_decision(
                                request.id,
                                request.tool,
                                "allow".into(),
                                "posture".into(),
                            )
                            .to_json_line()
                        );
                    } else {
                        println!(
                            "{}",
                            display.permission_decision("allow", &request.tool, "posture")
                        );
                        println!("{}", display.tool_running());
                    }
                } else {
                    let prompt_line = display.permission_ask(&request.tool, &request.summary);
                    print!("{prompt_line} ");
                    io::stdout().flush()?;
                    let mut input = String::new();
                    io::stdin().read_line(&mut input)?;
                    let (decision, rule) = match input.trim() {
                        "y" | "" => (PermissionChoice::Once, "user"),
                        _ => (PermissionChoice::Deny, "user"),
                    };
                    let _ = run.decide(&request.id, decision).await?;
                    let decision_str = if matches!(decision, PermissionChoice::Once) {
                        "allow"
                    } else {
                        "deny"
                    };
                    println!(
                        "{}",
                        display.permission_decision(decision_str, &request.tool, rule)
                    );
                    println!("{}", display.tool_running());
                }
            }
            Some(UiEvent::Done { usage, turn, .. }) => {
                close_thinking(
                    &mut in_thinking,
                    &mut thinking_start,
                    &mut display,
                    use_stream_json,
                );
                if let Some(u) = &usage {
                    total_input_tokens += u.input_tokens.unwrap_or(0);
                    total_output_tokens += u.output_tokens.unwrap_or(0);
                }
                current_turn = turn;
                break;
            }
            Some(_) => {}
            None => break,
        }
    }

    let session_elapsed = session_start.elapsed();
    println!();
    if use_stream_json {
        println!(
            "{}",
            OutputLine::session_completed(
                session_id.clone(),
                tier_name.clone(),
                model_name.clone(),
                current_turn,
                total_input_tokens,
                total_output_tokens,
                session_elapsed.as_millis() as u64,
            )
            .to_json_line()
        );
    } else {
        println!(
            "{}",
            display.session_completed(
                &session_id,
                current_turn,
                total_input_tokens,
                total_output_tokens,
                session_elapsed,
            )
        );
    }
    Ok(())
}

/// Interactive mode: bare `kuku` (no subcommand).
/// Currently uses CLI text streaming; future TUI.
pub async fn interactive(config: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let config_path = resolve_config_path(config.as_deref())?;
    if !config_path.exists() {
        eprintln!("error: config file not found: {}", config_path.display());
        eprintln!("hint: run `kuku init` to initialize");
        std::process::exit(1);
    }

    print!("> ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let prompt = input.trim().to_string();
    if prompt.is_empty() {
        return Ok(());
    }

    let args = RunArgs {
        prompt: vec![prompt],
        auto_yes: false,
        model: None,
        session: None,
        cont: false,
        json: false,
        stream_json: false,
        show_thinking: false,
        raw: false,
        config,
    };
    run(args).await
}
