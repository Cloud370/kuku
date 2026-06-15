use kuku::builtin_prompt_catalog;

use crate::cli_args::{PromptsArgs, PromptsSubcommand};

pub fn run(args: PromptsArgs) -> Result<(), Box<dyn std::error::Error>> {
    let catalog = builtin_prompt_catalog();

    match args.cmd {
        None | Some(PromptsSubcommand::Show { name: None }) => {
            print_prompt("system", &catalog.system.text);
            print_prompt("project-policy", &catalog.blocks["project-policy"].text);
            print_prompt("tool-guidance", &catalog.blocks["tool-guidance"].text);
            print_prompt("runtime-context", &catalog.runtime["context"].text);
        }
        Some(PromptsSubcommand::Show { name: Some(ref n) }) => match n.as_str() {
            "system" => print_prompt("system", &catalog.system.text),
            "project-context" | "project-policy" => {
                print_prompt("project-policy", &catalog.blocks["project-policy"].text)
            }
            "tool-guidance" => print_prompt("tool-guidance", &catalog.blocks["tool-guidance"].text),
            "runtime-context" => print_prompt("runtime-context", &catalog.runtime["context"].text),
            other => {
                eprintln!("unknown prompt: {other}");
                eprintln!("available: system, project-policy, tool-guidance, runtime-context");
                std::process::exit(1);
            }
        },
        Some(PromptsSubcommand::Export { dir }) => {
            let path = std::path::PathBuf::from(&dir);
            std::fs::create_dir_all(&path)?;

            // system
            std::fs::write(path.join("system.md"), &catalog.system.text)?;

            // blocks/
            let blocks_dir = path.join("blocks");
            std::fs::create_dir_all(&blocks_dir)?;
            for (name, asset) in &catalog.blocks {
                std::fs::write(blocks_dir.join(format!("{name}.md")), &asset.text)?;
            }

            // agents/
            let agents_dir = path.join("agents");
            std::fs::create_dir_all(&agents_dir)?;
            for (name, asset) in &catalog.agents {
                std::fs::write(agents_dir.join(format!("{name}.md")), &asset.text)?;
            }

            // memory/
            let memory_dir = path.join("memory");
            std::fs::create_dir_all(&memory_dir)?;
            for (name, asset) in &catalog.memory {
                std::fs::write(memory_dir.join(format!("{name}.md")), &asset.text)?;
            }

            // runtime/
            let runtime_dir = path.join("runtime");
            std::fs::create_dir_all(&runtime_dir)?;
            for (name, asset) in &catalog.runtime {
                std::fs::write(runtime_dir.join(format!("{name}.md")), &asset.text)?;
            }

            // tools/
            let tools_dir = path.join("tools");
            std::fs::create_dir_all(&tools_dir)?;
            for (name, asset) in &catalog.tools {
                std::fs::write(tools_dir.join(format!("{name}.md")), &asset.text)?;
            }

            println!("exported prompts to {}", path.display());
        }
    }
    Ok(())
}

fn print_prompt(name: &str, content: &str) {
    println!("-- {name} --");
    println!("{content}");
    println!();
}
