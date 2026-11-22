use kuku_client::{AgentSummary, SessionStatus};

use crate::cli_args::{ConnectionArgs, ListArgs};
use crate::commands::CliResult;
use crate::display::util::truncate;

/// List sessions: `kuku list [-a] [-w <workspace>] [-v]`
pub async fn run(args: ListArgs, target: &ConnectionArgs) -> CliResult {
    let options = target.options();
    let client = target.client().await?;
    if let Some(session_id) = args.session_id.as_deref() {
        let workspace = client.workspace(options.workspace()?).await?;
        let agents = client.agents(&workspace, session_id).await?;
        if args.json {
            println!("{}", serde_json::to_string(&agents)?);
            return Ok(());
        }
        return render_agents(agents);
    }
    let show_workspace_col = args.all && target.workspace.is_none();
    let workspace = if show_workspace_col {
        None
    } else {
        Some(client.workspace(options.workspace()?).await?)
    };
    let sessions = client.sessions(workspace.as_deref(), None).await?;

    if args.json {
        println!("{}", serde_json::to_string(&sessions)?);
        return Ok(());
    }
    if sessions.is_empty() {
        println!("no sessions found");
        return Ok(());
    }

    if show_workspace_col {
        if args.verbose {
            println!(
                "{:<20} {:<30} {:<15} {:<12} {:<20} {:<8} turns",
                "session_id", "title", "workspace", "status", "mtime", "size"
            );
        } else {
            println!(
                "{:<20} {:<30} {:<15} {:<12} {:<8} turns",
                "session_id", "title", "workspace", "status", "size"
            );
        }
    } else if args.verbose {
        println!(
            "{:<20} {:<30} {:<12} {:<20} {:<8} turns",
            "session_id", "title", "status", "mtime", "size"
        );
    } else {
        println!(
            "{:<20} {:<30} {:<12} {:<8} turns",
            "session_id", "title", "status", "size"
        );
    }

    for s in &sessions {
        let title = truncate(&s.title, 30);
        let status = match s.status {
            SessionStatus::Active => "active",
            SessionStatus::Done => "done",
            SessionStatus::Interrupted => "intr",
        };
        let mtime = relative_time(&s.mtime);
        let size = human_size(s.size);

        if show_workspace_col {
            let ws = truncate(&s.workspace.display().to_string(), 15);
            if args.verbose {
                println!(
                    "{:<20} {:<30} {:<15} {:<12} {:<20} {:<8} {}",
                    s.session_id, title, ws, status, mtime, size, s.turn_count
                );
            } else {
                println!(
                    "{:<20} {:<30} {:<15} {:<12} {:<8} {}",
                    s.session_id, title, ws, status, size, s.turn_count
                );
            }
        } else if args.verbose {
            println!(
                "{:<20} {:<30} {:<12} {:<20} {:<8} {}",
                s.session_id, title, status, mtime, size, s.turn_count
            );
        } else {
            println!(
                "{:<20} {:<30} {:<12} {:<8} {}",
                s.session_id, title, status, size, s.turn_count
            );
        }
    }
    Ok(())
}

fn render_agents(agents: Vec<AgentSummary>) -> CliResult {
    if agents.is_empty() {
        println!("no agents found");
        return Ok(());
    }
    println!("{:<24} {:<16} pending", "agent", "status");
    for agent in agents {
        println!(
            "{:<24} {:<16} {}",
            agent.address,
            serde_json::to_value(agent.status)?
                .as_str()
                .unwrap_or_default(),
            agent.pending_inputs
        );
    }
    Ok(())
}

fn relative_time(rfc3339: &str) -> String {
    if rfc3339.is_empty() {
        return "-".to_string();
    }
    let Ok(ts) =
        time::OffsetDateTime::parse(rfc3339, &time::format_description::well_known::Rfc3339)
    else {
        return "-".to_string();
    };
    let duration = time::OffsetDateTime::now_utc() - ts;
    let secs = duration.whole_seconds();
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{}KB", bytes / 1024)
    } else {
        format!("{}MB", bytes / (1024 * 1024))
    }
}
