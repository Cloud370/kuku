use std::path::Path;

use crate::execution::CommandInvocation;

const GUARDED_COMMANDS: &[(&str, &str)] = &[
    ("git reset --hard", "git reset --hard discards local work"),
    ("git clean -f", "git clean deletes untracked files"),
    ("rm -rf", "recursive force delete is destructive"),
    ("rm -fr", "recursive force delete is destructive"),
    ("rmdir /s /q", "recursive force delete is destructive"),
    ("del /s", "recursive delete is destructive"),
    (
        "remove-item -recurse -force",
        "recursive force delete is destructive",
    ),
    (
        "cargo publish",
        "publish affects external package registries",
    ),
    ("npm publish", "publish affects external package registries"),
    (
        "pnpm publish",
        "publish affects external package registries",
    ),
    (
        "yarn publish",
        "publish affects external package registries",
    ),
    ("bun publish", "publish affects external package registries"),
    ("npm run deploy", "deploy affects external systems"),
    ("pnpm deploy", "deploy affects external systems"),
    ("yarn deploy", "deploy affects external systems"),
    ("make deploy", "deploy affects external systems"),
    ("cargo release", "release affects external systems"),
];

pub(crate) fn blocked_command_reason(invocation: &CommandInvocation) -> Option<&'static str> {
    match invocation {
        CommandInvocation::Program { program, args } => native_guard_reason(program, args),
        CommandInvocation::Shell { command, .. } => shell_guard_reason(command),
    }
}

fn native_guard_reason(program: &str, args: &[String]) -> Option<&'static str> {
    let program = command_name(program)?;
    let (program, args) = if program == "sudo" {
        let (program, args) = args.split_first()?;
        (command_name(program)?, args)
    } else {
        (program, args)
    };
    for (rule, reason) in GUARDED_COMMANDS {
        let mut parts = rule.split_whitespace().peekable();
        let mut args = args.iter();
        if parts.next() != Some(program.as_str()) {
            continue;
        }
        while let Some(part) = parts.next() {
            let Some(arg) = args.next() else {
                break;
            };
            if parts.peek().is_none() && arg.to_ascii_lowercase().starts_with(part) {
                return Some(reason);
            }
            if !arg.eq_ignore_ascii_case(part) {
                break;
            }
        }
    }
    let flags: &[&str] = match program.as_str() {
        "sh" | "bash" | "zsh" => &["-c", "-lc", "-cl"],
        "cmd" => &["/c", "/k"],
        "powershell" | "pwsh" => &["-command", "-c"],
        _ => return None,
    };
    let command = args
        .iter()
        .position(|arg| flags.iter().any(|flag| arg.eq_ignore_ascii_case(flag)))?;
    shell_guard_reason(args.get(command + 1)?)
}

fn command_name(program: &str) -> Option<String> {
    let name = Path::new(program)
        .file_name()?
        .to_str()?
        .to_ascii_lowercase();
    Some(name.strip_suffix(".exe").unwrap_or(&name).to_string())
}

fn shell_guard_reason(command: &str) -> Option<&'static str> {
    normalized_command_segments(command)
        .into_iter()
        .find_map(|segment| {
            GUARDED_COMMANDS
                .iter()
                .find(|(prefix, _)| segment.starts_with(prefix))
                .map(|(_, reason)| *reason)
        })
}

#[allow(
    clippy::collapsible_str_replace,
    reason = "normalize compound separators before processing their individual characters"
)]
fn split_command_separators(input: &str) -> String {
    // Order matters: && must be replaced before & to avoid double-splitting
    input
        .replace("&&", "\x00")
        .replace("||", "\n")
        .replace('&', "\n")
        .replace('\x00', "\n")
        .replace(';', "\n")
}

fn normalized_command_segments(candidate: &str) -> Vec<String> {
    let normalized = split_command_separators(&candidate.to_ascii_lowercase());

    normalized
        .lines()
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.strip_prefix("sudo ").unwrap_or(segment).trim())
        .flat_map(unwrap_shell_wrapper)
        .collect()
}

fn unwrap_shell_wrapper(segment: &str) -> Vec<String> {
    for prefix in [
        "sh -c ",
        "bash -lc ",
        "zsh -lc ",
        "cmd /c ",
        "powershell -command ",
        "pwsh -command ",
    ] {
        if let Some(rest) = segment.strip_prefix(prefix) {
            let rest = rest.trim().trim_matches('"').trim_matches('\'');
            return split_command_separators(rest)
                .lines()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.strip_prefix("sudo ").unwrap_or(value).trim())
                .map(ToString::to_string)
                .collect();
        }
    }
    vec![segment.to_string()]
}
