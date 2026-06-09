use std::io::{self, Write};
use std::path::Path;

use kuku::context::{
    compute_file_revert_plan, count_file_turns_after, find_active_rollback, list_user_turns,
};
use kuku::conversation::address::ConversationAddress;
use kuku::event::{EventStore, RollbackScope};

pub fn run_undo(workspace: &Path, home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let sessions = kuku::session::list_sessions(home, Some(workspace))?;
    let latest = sessions
        .iter()
        .max_by_key(|s| s.created_at.as_str())
        .ok_or("no sessions found")?;
    let session_id = &latest.session_id;
    let events_path = kuku::session::session_events_path(home, workspace, session_id)?;
    let session_dir = events_path
        .parent()
        .ok_or("cannot determine session directory")?
        .to_path_buf();

    let events = EventStore::replay(&events_path)?;
    if events.is_empty() {
        println!("No events in current session.");
        return Ok(());
    }

    if let Some(active) = find_active_rollback(&events) {
        handle_undo_rollback(&events, &events_path, &session_dir, workspace, &active)?;
    } else {
        handle_new_rollback(&events, &events_path, &session_dir, workspace)?;
    }

    Ok(())
}

fn handle_new_rollback(
    events: &[kuku::event::StoredEvent],
    events_path: &Path,
    session_dir: &Path,
    workspace: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let turns = list_user_turns(events);
    if turns.is_empty() {
        println!("No user turns to rollback.");
        return Ok(());
    }

    println!("\nRecent conversation turns:");
    let display_count = turns.len().min(10);
    for (i, entry) in turns.iter().take(display_count).enumerate() {
        let file_marker = if entry.has_file_changes { " *" } else { "" };
        println!(
            "  [{}] turn {} {}\"{}\"{file_marker}",
            i + 1,
            entry.turn,
            format_ts(&entry.ts),
            entry.text_preview,
        );
    }
    print!("Select turn to rollback to [1-{display_count}]: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let choice: usize = input.trim().parse().map_err(|_| "invalid selection")?;
    if choice == 0 || choice > display_count {
        return Err("selection out of range".into());
    }
    let target_turn = turns[choice - 1].turn;

    let file_turn_count = count_file_turns_after(events, target_turn);
    let scope = select_scope(file_turn_count)?;

    if scope.affects_files() {
        let plan = compute_file_revert_plan(events, target_turn, workspace);
        display_plan_preview(&plan, workspace);
        if !plan.sensitive_files.is_empty() {
            print!("Sensitive files will be affected. Continue? [y/N]: ");
            io::stdout().flush()?;
            let mut s = String::new();
            io::stdin().read_line(&mut s)?;
            if s.trim().to_lowercase() != "y" {
                println!("Aborted.");
                return Ok(());
            }
        }
    }

    print!("Confirm rollback? [y/N]: ");
    io::stdout().flush()?;
    let mut confirm = String::new();
    io::stdin().read_line(&mut confirm)?;
    if confirm.trim().to_lowercase() != "y" {
        println!("Aborted.");
        return Ok(());
    }

    let result = kuku::context::rollback_turn(
        events_path,
        workspace,
        session_dir,
        &ConversationAddress::MAIN,
        target_turn,
        scope,
    )?;
    for w in &result.warnings {
        eprintln!("warning: {w}");
    }

    println!(
        "Rollback complete. ({} file(s) restored, {} deleted)",
        result.files_restored, result.files_deleted
    );
    Ok(())
}

fn handle_undo_rollback(
    events: &[kuku::event::StoredEvent],
    events_path: &Path,
    session_dir: &Path,
    workspace: &Path,
    active: &kuku::context::ActiveRollback,
) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "\nActive rollback detected (target: turn {}, scope: {:?})",
        active.to_turn, active.scope
    );

    let file_turn_count = count_file_turns_after(events, active.to_turn);

    let can_restore_files = match active.scope {
        RollbackScope::ConversationOnly => true,
        RollbackScope::FilesOnly => file_turn_count == 0,
        RollbackScope::Both => {
            if file_turn_count > 0 {
                println!(
                    "Warning: {file_turn_count} turn(s) with file changes since rollback. \
                     File state cannot be safely restored."
                );
                false
            } else {
                true
            }
        }
    };

    if can_restore_files && active.scope.affects_files() {
        println!("  [1] Undo rollback (restore conversation + files)");
    } else if active.scope.affects_conversation() {
        println!("  [1] Undo rollback (restore conversation only, files stay as-is)");
    } else {
        println!("  [1] Undo rollback (files only, no conversation change)");
    }
    println!("  [2] Cancel");
    print!("Select [1-2]: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let choice: u32 = input.trim().parse().map_err(|_| "invalid selection")?;

    if choice != 1 {
        println!("Cancelled.");
        return Ok(());
    }

    let result = kuku::context::undo_rollback(events_path, workspace, session_dir)?;
    if result.files_restored {
        println!("Files restored from backup.");
    } else if active.scope.affects_files() {
        eprintln!("warning: files not restored (backup not found or unsafe)");
    }

    println!("Rollback undone.");
    Ok(())
}

fn select_scope(file_turn_count: usize) -> Result<RollbackScope, Box<dyn std::error::Error>> {
    println!("\nRollback scope:");
    if file_turn_count > 0 {
        println!("  [1] Conversation + files ({file_turn_count} file turn(s) will be reverted)");
        println!("  [2] Conversation only");
        println!("  [3] Files only");
    } else {
        println!("  [1] Conversation only");
        println!("  [2] Files only");
    }
    print!("Select: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let choice: u32 = input.trim().parse().map_err(|_| "invalid selection")?;

    if file_turn_count > 0 {
        match choice {
            1 => Ok(RollbackScope::Both),
            2 => Ok(RollbackScope::ConversationOnly),
            3 => Ok(RollbackScope::FilesOnly),
            _ => Err("invalid selection".into()),
        }
    } else {
        match choice {
            1 => Ok(RollbackScope::ConversationOnly),
            2 => Ok(RollbackScope::FilesOnly),
            _ => Err("invalid selection".into()),
        }
    }
}

fn display_plan_preview(plan: &kuku::context::RevertPlan, workspace: &Path) {
    if plan.restores.is_empty() && plan.deletes.is_empty() && plan.unrecoverable.is_empty() {
        println!("\nNo file changes to revert.");
        return;
    }
    println!("\nFile change preview:");
    for restore in &plan.restores {
        let relative = restore
            .path
            .strip_prefix(workspace)
            .unwrap_or(&restore.path);
        let diff = restore.new_content_on_plan.lines().count() as i64
            - restore.old_content.lines().count() as i64;
        println!("  restore: {} ({:+} lines)", relative.display(), diff);
    }
    for delete in &plan.deletes {
        let relative = delete.strip_prefix(workspace).unwrap_or(delete);
        println!("  delete: {} (did not exist before)", relative.display());
    }
    for unrec in &plan.unrecoverable {
        let relative = unrec.strip_prefix(workspace).unwrap_or(unrec);
        println!(
            "  unrecoverable: {} (no full snapshot available)",
            relative.display()
        );
    }
}

fn format_ts(ts: &str) -> String {
    if ts.is_empty() {
        return String::new();
    }
    format!("[{ts}] ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_ts_empty() {
        assert_eq!(format_ts(""), "");
    }

    #[test]
    fn format_ts_nonempty() {
        assert_eq!(format_ts("2026-05-28T00:00:00Z"), "[2026-05-28T00:00:00Z] ");
    }
}
