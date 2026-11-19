use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tokio::process::Command;

use crate::execution::{CommandInvocation, CommandShell};
use crate::tool::{ToolDefinition, ToolResultEnvelope};

use super::shell::shell_command;

const TIMEOUT_CAP_SECONDS: u64 = 600;

pub(super) struct CommandRequest {
    pub(super) invocation: CommandInvocation,
    pub(super) cwd: PathBuf,
    pub(super) timeout_seconds: u64,
}

impl CommandRequest {
    pub(super) fn command(&self) -> Result<Command, String> {
        match &self.invocation {
            CommandInvocation::Program { program, args } => {
                let program = if Path::new(program).file_name() == Some(OsStr::new(program)) {
                    PathBuf::from(program)
                } else {
                    self.cwd.join(program)
                };
                let mut command = Command::new(program);
                command.args(args);
                Ok(command)
            }
            CommandInvocation::Shell { shell, command } => shell_command(*shell, command)
                .ok_or_else(|| format!("selected interpreter {shell:?} was not found")),
        }
    }
}

pub(crate) fn prepare_command_args(
    args: &mut Value,
    workspace: &Path,
    definition: &ToolDefinition,
) -> Result<PathBuf, ToolResultEnvelope> {
    if let Some(fields) = args
        .as_object_mut()
        .filter(|fields| fields.contains_key("command"))
    {
        if !fields.contains_key("shell") {
            if let Some(default) = definition.input_schema["properties"]["shell"].get("default") {
                fields.insert("shell".into(), default.clone());
            }
        }
        if fields
            .get("args")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
        {
            fields.remove("args");
        }
    }
    let request = run_command_request(args, workspace)?;
    let cwd = request
        .cwd
        .to_str()
        .ok_or_else(|| invalid("command cwd is not valid Unicode"))?;
    args["cwd"] = cwd.into();
    Ok(request.cwd)
}

pub(crate) fn command_permission_candidate(args: &Value) -> String {
    let Some(cwd) = args.get("cwd").and_then(Value::as_str) else {
        return String::new();
    };
    invocation(args)
        .map(|invocation| serde_json::json!({"cwd": cwd, "invocation": invocation}).to_string())
        .unwrap_or_default()
}

pub(super) fn run_command_request(
    args: &Value,
    workspace: &Path,
) -> Result<CommandRequest, ToolResultEnvelope> {
    let fields = args
        .as_object()
        .ok_or_else(|| invalid("run_command expects an object"))?;
    if let Some(field) = fields.keys().find(|field| {
        !matches!(
            field.as_str(),
            "program" | "args" | "shell" | "command" | "cwd" | "timeout" | "brief"
        )
    }) {
        return Err(invalid(format!("run_command does not accept {field:?}")));
    }
    let invocation = invocation(args)?;
    let cwd = args
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|cwd| !cwd.is_empty())
        .ok_or_else(|| {
            invalid("run_command requires an explicit cwd (use '.' for the workspace)")
        })?;
    let cwd = workspace.join(cwd).canonicalize().map_err(|error| {
        invalid(format!(
            "command cwd {cwd:?} could not be resolved: {error}"
        ))
    })?;
    if !cwd.is_dir() {
        return Err(invalid(format!(
            "command cwd is not a directory: {}",
            cwd.display()
        )));
    }
    let timeout_seconds = args
        .get("timeout")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("run_command requires timeout in seconds"))?;
    if !(1..=TIMEOUT_CAP_SECONDS).contains(&timeout_seconds) {
        return Err(invalid(format!(
            "timeout must be between 1 and {TIMEOUT_CAP_SECONDS} seconds"
        )));
    }
    Ok(CommandRequest {
        invocation,
        cwd,
        timeout_seconds,
    })
}

fn invocation(args: &Value) -> Result<CommandInvocation, ToolResultEnvelope> {
    if let Some(shell) = args.get("shell") {
        if args.get("program").is_some() || args.get("args").is_some() {
            return Err(invalid(
                "select either program/args or shell/command, not both",
            ));
        }
        let shell = serde_json::from_value::<CommandShell>(shell.clone())
            .map_err(|error| invalid(format!("invalid command shell: {error}")))?;
        let command = args
            .get("command")
            .and_then(Value::as_str)
            .filter(|command| !command.trim().is_empty())
            .ok_or_else(|| invalid("explicit shell execution requires a nonempty command"))?;
        return Ok(CommandInvocation::Shell {
            shell,
            command: command.to_string(),
        });
    }
    if args.get("command").is_some() {
        return Err(invalid("No default shell is advertised. Supply a listed shell with command, or use program and literal args."));
    }
    let program = args
        .get("program")
        .and_then(Value::as_str)
        .filter(|program| !program.trim().is_empty())
        .ok_or_else(|| invalid("run_command requires program and literal args, or command text"))?;
    let args = args
        .get("args")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            invalid("run_command requires a literal args array (use [] for no arguments)")
        })?
        .iter()
        .map(|arg| {
            arg.as_str()
                .map(ToString::to_string)
                .ok_or_else(|| invalid("every command argument must be a string"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if cfg!(windows) && requires_cmd_shell(program) {
        return Err(invalid(
            "Windows batch files and cmd.exe require shell='cmd' and command text",
        ));
    }
    Ok(CommandInvocation::Program {
        program: program.to_string(),
        args,
    })
}

fn requires_cmd_shell(program: &str) -> bool {
    let Some(name) = Path::new(program).file_name().and_then(OsStr::to_str) else {
        return false;
    };
    let name = name.trim_end_matches([' ', '.']);
    name.eq_ignore_ascii_case("cmd")
        || name.eq_ignore_ascii_case("cmd.exe")
        || Path::new(name).extension().is_some_and(|extension| {
            extension.eq_ignore_ascii_case("bat") || extension.eq_ignore_ascii_case("cmd")
        })
}

fn invalid(message: impl Into<String>) -> ToolResultEnvelope {
    let message = message.into();
    ToolResultEnvelope::error(format!("failed: {message}"), message)
}
