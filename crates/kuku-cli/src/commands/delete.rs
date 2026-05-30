use std::path::PathBuf;

use kuku::session::{delete_session, kuku_home, list_sessions};

use crate::cli_args::DeleteArgs;

/// Delete a session: `kuku delete <session_id> [-w <workspace>]`
pub async fn run(args: DeleteArgs) -> Result<(), Box<dyn std::error::Error>> {
    let home = kuku_home()?;
    let workspace = args.workspace.map(PathBuf::from);

    let sessions = list_sessions(&home, workspace.as_deref())?;
    let target = sessions.iter().find(|s| s.session_id == args.session_id);

    match target {
        Some(s) => {
            println!(
                "Delete session '{}'?\n  title: {}\n  workspace: {}\n  turns: {}\n  status: {}",
                s.session_id,
                s.title,
                s.workspace.display(),
                s.turn_count,
                s.status,
            );
            print!("Confirm? [y/N]: ");
            use std::io::Write;
            std::io::stdout().flush()?;

            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            if matches!(input.trim(), "y" | "") {
                delete_session(&home, workspace.as_deref(), &args.session_id)?;
                println!("deleted {}", args.session_id);
            } else {
                println!("cancelled");
            }
        }
        None => {
            eprintln!("session not found: {}", args.session_id);
        }
    }
    Ok(())
}
