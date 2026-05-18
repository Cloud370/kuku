use kuku::builtin_prompt_catalog;

use crate::cli_args::{PromptsArgs, PromptsSubcommand};

pub fn run(args: PromptsArgs) -> Result<(), Box<dyn std::error::Error>> {
    let catalog = builtin_prompt_catalog();

    match args.cmd {
        None | Some(PromptsSubcommand::Show { name: None }) => {
            print_prompt("system", &catalog.system.text);
            print_prompt("project-context", &catalog.project_context.text);
            print_prompt("tool-guidance", &catalog.tool_guidance.text);
            print_prompt("runtime-context", &catalog.runtime_context.text);
        }
        Some(PromptsSubcommand::Show { name: Some(ref n) }) => match n.as_str() {
            "system" => print_prompt("system", &catalog.system.text),
            "project-context" => {
                print_prompt("project-context", &catalog.project_context.text)
            }
            "tool-guidance" => print_prompt("tool-guidance", &catalog.tool_guidance.text),
            "runtime-context" => {
                print_prompt("runtime-context", &catalog.runtime_context.text)
            }
            other => {
                eprintln!("unknown prompt: {other}");
                eprintln!(
                    "available: system, project-context, tool-guidance, runtime-context"
                );
                std::process::exit(1);
            }
        },
        Some(PromptsSubcommand::Export { dir }) => {
            let path = std::path::PathBuf::from(&dir);
            std::fs::create_dir_all(&path)?;
            std::fs::write(path.join("system.md"), &catalog.system.text)?;
            std::fs::write(
                path.join("project-context.md"),
                &catalog.project_context.text,
            )?;
            std::fs::write(path.join("tool-guidance.md"), &catalog.tool_guidance.text)?;
            std::fs::write(
                path.join("runtime-context.md"),
                &catalog.runtime_context.text,
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
