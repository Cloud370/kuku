use kuku::session::{current_workspace, kuku_home, list_sessions};

use crate::cli_args::ListArgs;

pub async fn run(args: ListArgs) -> Result<(), Box<dyn std::error::Error>> {
    let home = kuku_home()?;
    let workspace = current_workspace()?;
    let sessions = list_sessions(&home, &workspace)?;
    if sessions.is_empty() {
        println!("no sessions in this workspace");
        return Ok(());
    }
    for s in &sessions {
        if args.verbose {
            println!(
                "{}  created:{}  turns:{}",
                s.session_id, s.created_at, s.turn_count
            );
        } else {
            println!("{}", s.session_id);
        }
    }
    Ok(())
}
