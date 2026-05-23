use std::io::{self, Write};
use std::time::Instant;

use kuku::subagent::registry::SubagentRegistry;
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

fn build_skill_body(
    skill_name: &str,
    registry: &kuku::skill::registry::SkillRegistry,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Some(def) = registry.get(skill_name) else {
        return Ok(None);
    };
    let dir = def
        .source_path
        .as_deref()
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            format!(
                "{}/{}",
                match def.source {
                    kuku::skill::definition::SkillSource::ClaudeCodeUser => "~/.claude/skills",
                    kuku::skill::definition::SkillSource::ClaudeCodeProject => ".claude/skills",
                    kuku::skill::definition::SkillSource::OpenCodeUser =>
                        "~/.config/opencode/skills",
                    kuku::skill::definition::SkillSource::OpenCodeProject => ".opencode/skills",
                    kuku::skill::definition::SkillSource::KukuUser => "~/.kuku/skills",
                    kuku::skill::definition::SkillSource::KukuProject => ".kuku/skills",
                },
                skill_name,
            )
        });
    let content = std::fs::read_to_string(std::path::Path::new(&dir).join("SKILL.md"))?;
    let (_, body) = kuku::subagent::compat::claude_code::split_yaml_frontmatter(&content);
    Ok(Some(format!("<!-- loaded: {dir} -->\n\n{body}")))
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

    // Parse slash command (unless skill_body already provided by caller)
    let (user_prompt, skill_body) = if let Some(body) = args.skill_body {
        (prompt, Some(body))
    } else if prompt.starts_with('/') && !args.no_skills {
        let workspace = kuku::session::current_workspace()?;
        let registry = kuku::skill::registry::SkillRegistry::builder()
            .load_claude_user_skills()
            .and_then(|b| b.load_claude_project_skills(&workspace))
            .and_then(|b| b.load_opencode_user_skills())
            .and_then(|b| b.load_opencode_project_skills(&workspace))
            .and_then(|b| b.load_kuku_user_skills())
            .and_then(|b| b.load_kuku_project_skills(&workspace))
            .map(|b| b.build())
            .ok();
        match registry {
            Some(ref reg) => {
                let (skill_name, rest) = parse_slash_command(&prompt);
                match build_skill_body(&skill_name, reg) {
                    Ok(Some(body)) => (
                        if rest.is_empty() { String::new() } else { rest },
                        Some(body),
                    ),
                    Ok(None) => {
                        eprintln!("Unknown skill: {skill_name}. Run 'kuku skills list' to see available skills.");
                        std::process::exit(1);
                    }
                    Err(e) => {
                        eprintln!("Error loading skill '{skill_name}': {e}");
                        std::process::exit(1);
                    }
                }
            }
            None => (prompt, None),
        }
    } else if prompt.starts_with('/') && args.no_skills {
        eprintln!("warning: slash command used with --no-skills; skill injection skipped");
        (prompt, None)
    } else {
        (prompt, None)
    };

    let mut q = query(&user_prompt).config_path(config_path);
    if let Some(body) = skill_body {
        q = q.skill_body(body);
    }
    if args.no_skills {
        q = q.no_skills();
    }
    if args.no_agents {
        q = q.no_agents();
    } else {
        let workspace = kuku::session::current_workspace()?;
        let registry = SubagentRegistry::builder()
            .builtins()
            .load_claude_user_agents()?
            .load_claude_project_agents(&workspace)?
            .load_opencode_user_agents()?
            .load_opencode_project_agents(&workspace)?
            .load_kuku_user_agents()?
            .load_kuku_project_agents(&workspace)?
            .build();
        q = q.subagents(registry);
    }
    if let Some(ref dir) = args.prompts_dir {
        q = q.prompts_dir(std::path::PathBuf::from(dir));
    }
    if let Some(model) = &args.model {
        if cfg.tier(model).is_some() {
            q = q.tier(model.clone());
        } else {
            q = q.model(model.clone());
        }
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
                        if let Some(input) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
                            let cache_read = usage
                                .get("cache_read_input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            let cache_creation = usage
                                .get("cache_creation_input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            previous_input_tokens = input + cache_read + cache_creation;
                            break;
                        }
                    }
                }
            }
        }
    }

    // JSON single-result path: use run(), output one final JSON line
    if args.json {
        let json_start = std::time::Instant::now();
        let output = q.run().await?;
        let json_elapsed = json_start.elapsed();
        let (input_tokens, output_tokens, cache_read, cache_creation) = output
            .usage
            .as_ref()
            .map(|u| {
                (
                    u.input_tokens.unwrap_or(0),
                    u.output_tokens.unwrap_or(0),
                    u.cache_read_input_tokens.unwrap_or(0),
                    u.cache_creation_input_tokens.unwrap_or(0),
                )
            })
            .unwrap_or((0, 0, 0, 0));
        let line = OutputLine::session_completed(
            output.session_id,
            tier_name.clone(),
            model_name.clone(),
            output.turn,
            (input_tokens, output_tokens, cache_read, cache_creation),
            json_elapsed.as_millis() as u64,
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
    let mut total_cache_read_input_tokens: u64 = 0;
    let mut total_cache_creation_input_tokens: u64 = 0;
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
            Some(UiEvent::ToolStart {
                id,
                tool,
                summary,
                kind: _,
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
                        OutputLine::tool_call(tool, id, summary, serde_json::Value::Null,)
                            .to_json_line()
                    );
                } else {
                    println!("\n{}", display.tool_call(&tool, &summary, &id));
                }
            }
            Some(UiEvent::ToolEnd {
                id,
                status,
                summary,
                result: _,
            }) => {
                if use_stream_json {
                    println!(
                        "{}",
                        OutputLine::tool_result(id, status, summary, None, false).to_json_line()
                    );
                } else {
                    println!("{}", display.tool_result(&status, &summary, &id));
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
                    let _ = run
                        .decide(&request.id, PermissionChoice::Once, None)
                        .await?;
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
                    let _ = run.decide(&request.id, decision, None).await?;
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
                    total_cache_read_input_tokens += u.cache_read_input_tokens.unwrap_or(0);
                    total_cache_creation_input_tokens += u.cache_creation_input_tokens.unwrap_or(0);
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
                (
                    total_input_tokens,
                    total_output_tokens,
                    total_cache_read_input_tokens,
                    total_cache_creation_input_tokens,
                ),
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
                total_cache_read_input_tokens,
                total_cache_creation_input_tokens,
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

    let workspace = kuku::session::current_workspace()?;
    let skill_registry = kuku::skill::registry::SkillRegistry::builder()
        .load_claude_user_skills()
        .and_then(|b| b.load_claude_project_skills(&workspace))
        .and_then(|b| b.load_opencode_user_skills())
        .and_then(|b| b.load_opencode_project_skills(&workspace))
        .and_then(|b| b.load_kuku_user_skills())
        .and_then(|b| b.load_kuku_project_skills(&workspace))
        .map(|b| b.build())
        .ok();

    loop {
        print!("> ");
        io::stdout().flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            break;
        }
        let prompt = input.trim().to_string();
        if prompt.is_empty() {
            continue;
        }
        if prompt == "exit" || prompt == "quit" {
            break;
        }

        let (user_prompt, skill_body) = if prompt.starts_with('/') {
            if let Some(ref registry) = skill_registry {
                let (skill_name, rest) = parse_slash_command(&prompt);
                match build_skill_body(&skill_name, registry) {
                    Ok(Some(body)) => (
                        if rest.is_empty() { String::new() } else { rest },
                        Some(body),
                    ),
                    Ok(None) => {
                        eprintln!(
                            "Unknown skill: {skill_name}. Run 'kuku skills list' to see available skills."
                        );
                        continue;
                    }
                    Err(e) => {
                        eprintln!("Error loading skill '{skill_name}': {e}");
                        continue;
                    }
                }
            } else {
                (prompt, None)
            }
        } else {
            (prompt, None)
        };

        let args = RunArgs {
            prompt: vec![user_prompt],
            auto_yes: false,
            model: None,
            session: None,
            cont: false,
            json: false,
            stream_json: false,
            show_thinking: false,
            raw: false,
            config: config.clone(),
            prompts_dir: None,
            no_agents: false,
            no_skills: false,
            skill_body,
        };
        if let Err(e) = run(args).await {
            eprintln!("error: {e}");
        }
    }
    Ok(())
}

fn parse_slash_command(input: &str) -> (String, String) {
    let without_slash = input[1..].trim_start();
    match without_slash.find(char::is_whitespace) {
        Some(pos) => (
            without_slash[..pos].to_string(),
            without_slash[pos..].trim().to_string(),
        ),
        None => (without_slash.to_string(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_slash_command;

    #[test]
    fn slash_command_with_prompt() {
        let (name, rest) = parse_slash_command("/tdd implement login");
        assert_eq!(name, "tdd");
        assert_eq!(rest, "implement login");
    }

    #[test]
    fn slash_command_without_prompt() {
        let (name, rest) = parse_slash_command("/review");
        assert_eq!(name, "review");
        assert_eq!(rest, "");
    }

    #[test]
    fn slash_command_with_multiple_words() {
        let (name, rest) = parse_slash_command("/code-review check auth module");
        assert_eq!(name, "code-review");
        assert_eq!(rest, "check auth module");
    }

    #[test]
    fn slash_command_trims_leading_whitespace() {
        let (name, rest) = parse_slash_command("/  tdd implement login");
        assert_eq!(name, "tdd");
        assert_eq!(rest, "implement login");
    }
}
