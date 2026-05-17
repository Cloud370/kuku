use std::fs;

use kuku::config::generate_default;
use kuku::session::kuku_home;

/// Initialize kuku: generate config.toml and create directory structure.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let home = kuku_home()?;
    let config_path = home.join("config.toml");

    if config_path.exists() {
        eprintln!("config file already exists: {}", config_path.display());
        eprintln!("delete it first to regenerate.");
        std::process::exit(1);
    }

    fs::create_dir_all(&home)?;
    fs::create_dir_all(home.join("sessions"))?;

    fs::write(&config_path, generate_default())?;

    println!("created config: {}", config_path.display());
    println!("created directory: {}", home.display());
    println!();
    println!("set your API key to get started:");
    println!("  export ANTHROPIC_API_KEY=your-key");
    println!("  kuku run \"hello\"");
    Ok(())
}
