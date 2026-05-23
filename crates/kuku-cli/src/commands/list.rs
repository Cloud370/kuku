use std::path::PathBuf;

use kuku::session::{current_workspace, kuku_home, list_sessions, SessionStatus};

use crate::cli_args::ListArgs;

/// List sessions: `kuku list [-a] [-w <workspace>] [-v]`
pub async fn run(args: ListArgs) -> Result<(), Box<dyn std::error::Error>> {
    let home = kuku_home()?;

    let workspace_override = args.workspace.map(PathBuf::from);
    let default_ws;
    let (workspace, show_workspace_col) = if workspace_override.is_some() {
        (workspace_override.as_deref(), false)
    } else if args.all {
        (None, true)
    } else {
        default_ws = current_workspace()?;
        (Some(default_ws.as_path()), false)
    };

    let sessions = list_sessions(&home, workspace)?;

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

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max - 1])
    }
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
