use std::io::{self, Write};

use kuku::{query, PermissionChoice, UiEvent};

use crate::cli_args::RunArgs;
use crate::display::{Display, OutputLine, Verbosity};

fn resolve_config_path(
    custom: Option<&str>,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    if let Some(p) = custom {
        return Ok(std::path::PathBuf::from(p));
    }
    let home = kuku::session::kuku_home()?;
    Ok(home.join("config.toml"))
}

/// Non-interactive run: `kuku run "prompt" [flags]`
pub async fn run(args: RunArgs) -> Result<(), Box<dyn std::error::Error>> {
    let prompt = args.prompt.join(" ");
    let verbosity = if args.verbose {
        Verbosity::Verbose
    } else {
        Verbosity::Concise
    };
    let display = Display::new(verbosity);

    let config_path = resolve_config_path(args.config.as_deref())?;
    if !config_path.exists() {
        eprintln!("error: 未找到配置文件 {}", config_path.display());
        eprintln!("提示: 运行 `kuku init` 初始化配置");
        std::process::exit(1);
    }

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
        q = q.session(latest.session_id.clone());
    }

    // JSON single-result path: use run(), output one final JSON line
    if args.json {
        let output = q.run().await?;
        let line = OutputLine::session_completed(output.session_id, 0, 0, 0);
        println!("{}", line.to_json_line());
        return Ok(());
    }

    let use_stream_json = args.stream_json;
    let mut run = q.start().await?;
    let session_id = run.session_id().to_string();

    if use_stream_json {
        println!(
            "{}",
            OutputLine::session_started(session_id.clone(), "default".into(), "normal".into())
                .to_json_line()
        );
    } else {
        println!(
            "{}",
            display.session_start(&session_id, "default", "normal")
        );
    }

    let mut in_thinking = false;
    let thinking_tokens: u64 = 0;

    loop {
        let event = tokio::select! {
            result = run.next() => result?,
            _ = tokio::signal::ctrl_c() => {
                if use_stream_json {
                    println!("{}", OutputLine::session_interrupted(
                        session_id.clone(), 0,
                    ).to_json_line());
                } else {
                    eprintln!("{}", display.session_interrupted(&session_id, 0));
                }
                std::process::exit(130);
            }
        };

        match event {
            Some(UiEvent::TextDelta { text }) => {
                if in_thinking {
                    in_thinking = false;
                    if !use_stream_json {
                        println!("{}", display.thinking_end(thinking_tokens));
                    }
                }
                if use_stream_json {
                    println!("{}", OutputLine::text_delta(text).to_json_line());
                } else {
                    print!("{text}");
                }
            }
            Some(UiEvent::ThinkingDelta { text }) => {
                if !in_thinking {
                    in_thinking = true;
                    if !use_stream_json {
                        println!("{}", display.thinking_start(0));
                    }
                }
                if let Some(rendered) = display.thinking_text(&text) {
                    if !use_stream_json {
                        print!("{rendered}");
                    }
                }
            }
            Some(UiEvent::ToolCall {
                tool_call_id,
                tool,
                summary,
            }) => {
                if in_thinking {
                    in_thinking = false;
                    if !use_stream_json {
                        println!("{}", display.thinking_end(thinking_tokens));
                    }
                }
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
                summary,
            }) => {
                if use_stream_json {
                    println!(
                        "{}",
                        OutputLine::tool_result(tool_call_id, "ok".into(), summary, None, false)
                            .to_json_line()
                    );
                } else {
                    println!("{}", display.tool_result("ok", &summary, &tool_call_id));
                }
            }
            Some(UiEvent::PermissionRequested { request }) => {
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
                }
            }
            Some(UiEvent::Done { .. }) => {
                if in_thinking && !use_stream_json {
                    println!("{}", display.thinking_end(thinking_tokens));
                }
                break;
            }
            Some(_) => {}
            None => break,
        }
    }

    println!();
    if use_stream_json {
        println!(
            "{}",
            OutputLine::session_completed(session_id.clone(), 0, 0, 0).to_json_line()
        );
    } else {
        println!("{}", display.session_completed(&session_id, 0, 0, 0));
    }
    Ok(())
}

/// Interactive mode: bare `kuku` (no subcommand).
/// Currently uses CLI text streaming; future TUI.
pub async fn interactive(config: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let config_path = resolve_config_path(config.as_deref())?;
    if !config_path.exists() {
        eprintln!("error: 未找到配置文件 {}", config_path.display());
        eprintln!("提示: 运行 `kuku init` 初始化配置");
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
        verbose: false,
        config,
    };
    run(args).await
}
