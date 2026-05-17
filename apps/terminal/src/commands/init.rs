use std::fs;

use kuku::config::generate_default;
use kuku::session::kuku_home;

/// Initialize kuku: generate config.toml and create directory structure.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let home = kuku_home()?;
    let config_path = home.join("config.toml");

    if config_path.exists() {
        eprintln!("配置文件已存在: {}", config_path.display());
        eprintln!("如需重新生成，请先删除该文件。");
        std::process::exit(1);
    }

    fs::create_dir_all(&home)?;
    fs::create_dir_all(home.join("sessions"))?;

    fs::write(&config_path, generate_default())?;

    println!("已生成配置文件: {}", config_path.display());
    println!("已创建目录: {}", home.display());
    println!();
    println!("请设置 API key 环境变量后即可使用:");
    println!("  export ANTHROPIC_API_KEY=your-key");
    println!("  kuku run \"hello\"");
    Ok(())
}
