use std::path::PathBuf;

use kuku::session::{kuku_home, list_sessions, SessionStatus};

use crate::cli_args::ListArgs;

/// List sessions: `kuku list [-w <workspace>] [-v]`
pub async fn run(args: ListArgs) -> Result<(), Box<dyn std::error::Error>> {
    let home = kuku_home()?;
    let workspace = args.workspace.map(PathBuf::from);
    let sessions = list_sessions(&home, workspace.as_deref())?;

    if sessions.is_empty() {
        println!("no sessions found");
        return Ok(());
    }

    if args.verbose {
        println!(
            "{:<20} {:<30} {:<15} {:<12} {:<20} {:<8} turns",
            "session_id", "title", "workspace", "status", "mtime", "size"
        );
    } else {
        println!(
            "{:<20} {:<30} {:<15} {:<12} turns",
            "session_id", "title", "workspace", "status"
        );
    }

    for s in &sessions {
        let title = truncate(&s.title, 30);
        let ws = truncate(&s.workspace.display().to_string(), 15);
        let status = match s.status {
            SessionStatus::Active => "active",
            SessionStatus::Done => "done",
            SessionStatus::Interrupted => "intr",
        };
        let mtime = relative_time(&s.mtime);
        let size = human_size(s.size);

        if args.verbose {
            println!(
                "{:<20} {:<30} {:<15} {:<12} {:<20} {:<8} {}",
                s.session_id, title, ws, status, mtime, size, s.turn_count
            );
        } else {
            println!(
                "{:<20} {:<30} {:<15} {:<12} {}",
                s.session_id, title, ws, status, s.turn_count
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
