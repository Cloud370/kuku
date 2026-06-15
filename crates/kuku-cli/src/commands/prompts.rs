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
            std::fs::write(path.join("system.md"), &catalog.system.text)?;
            std::fs::write(
                path.join("project-policy.md"),
                &catalog.blocks["project-policy"].text,
            )?;
            std::fs::write(
                path.join("tool-guidance.md"),
                &catalog.blocks["tool-guidance"].text,
            )?;
            std::fs::write(
                path.join("runtime-context.md"),
                &catalog.runtime["context"].text,
            )?;
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
