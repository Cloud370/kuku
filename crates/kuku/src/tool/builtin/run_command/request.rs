use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tokio::process::Command;

use crate::execution::{CommandInvocation, CommandShell};
use crate::tool::{ToolDefinition, ToolResultEnvelope};

use super::shell::shell_command;
use super::MAX_REQUESTED_PREVIEW_BYTES;

pub(super) struct PreparedCommand {
    pub(super) command: Command,
    executable_paths: Vec<PathBuf>,
}

impl PreparedCommand {
    pub(super) fn executable_exists(&self, cwd: &Path) -> bool {
        self.executable_paths
            .iter()
            .any(|path| cwd.join(path).exists())
    }
}

pub(super) struct CommandRequest {
    pub(super) invocation: CommandInvocation,
    pub(super) cwd: PathBuf,
    pub(super) timeout_seconds: Option<u32>,
    pub(super) preview_bytes: Option<usize>,
}

impl CommandRequest {
    pub(super) fn command(&self) -> io::Result<PreparedCommand> {
        match &self.invocation {
            CommandInvocation::Program { program, args } => {
                let program = if Path::new(program).file_name() == Some(OsStr::new(program)) {
                    PathBuf::from(program)
                } else {
                    self.cwd.join(program)
                };
                let mut command = Command::new(program);
                command.args(args);
                #[cfg(unix)]
                let executable_paths = super::native::configure(&mut command)?;
                #[cfg(not(unix))]
                let executable_paths = vec![PathBuf::from(command.as_std().get_program())];
                Ok(PreparedCommand {
                    command,
                    executable_paths,
                })
            }
            CommandInvocation::Shell { shell, command } => {
                let command = shell_command(*shell, command).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("selected interpreter {shell:?} was not found"),
                    )
                })?;
                let executable_paths = vec![PathBuf::from(command.as_std().get_program())];
                Ok(PreparedCommand {
                    command,
                    executable_paths,
                })
            }
        }
    }
}

pub(crate) fn prepare_command_args(
    args: &mut Value,
    workspace: &Path,
    definition: &ToolDefinition,
) -> Result<PathBuf, ToolResultEnvelope> {
    apply_command_defaults(args, definition)?;
    let request = run_command_request(args, workspace)?;
    let cwd = request
        .cwd
        .to_str()
        .ok_or_else(|| invalid("command cwd is not valid Unicode"))?;
    args["cwd"] = cwd.into();
    Ok(request.cwd)
}

fn apply_command_defaults(
    args: &mut Value,
    definition: &ToolDefinition,
) -> Result<(), ToolResultEnvelope> {
    if let Some(fields) = args.as_object_mut() {
        for field in ["background", "preview_bytes"] {
            if fields.contains_key(field)
                && definition.input_schema["properties"].get(field).is_none()
            {
                return Err(invalid(format!(
                    "{field} requires a top-level Session tool call"
                )));
            }
        }
        let shell_text = fields.contains_key("command");
        let mode_field = if shell_text { "shell" } else { "args" };
        for key in ["cwd", mode_field] {
            if let Some(default) = definition.input_schema["properties"][key].get("default") {
                fields.entry(key).or_insert_with(|| default.clone());
            }
        }
        if shell_text
            && fields
                .get("args")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
        {
            fields.remove("args");
        }
    }
    Ok(())
}

pub(crate) fn command_permission_candidate(args: &Value) -> String {
    if let Some(candidate) = resolved_candidate(args) {
        return candidate;
    }
    let mut args = args.clone();
    let definition =
        crate::tool::registry::run_command_definition(&super::available_shells(), false);
    apply_command_defaults(&mut args, &definition)
        .ok()
        .and_then(|()| resolved_candidate(&args))
        .unwrap_or_default()
}

fn resolved_candidate(args: &Value) -> Option<String> {
    let cwd = args.get("cwd")?.as_str()?;
    let invocation = invocation(args).ok()?;
    Some(serde_json::json!({"cwd": cwd, "invocation": invocation}).to_string())
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
            "program"
                | "args"
                | "shell"
                | "command"
                | "cwd"
                | "timeout"
                | "brief"
                | "background"
                | "preview_bytes"
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
        .map(|value| {
            value
                .as_u64()
                .and_then(|seconds| u32::try_from(seconds).ok())
                .filter(|seconds| *seconds > 0)
                .ok_or_else(|| invalid("timeout must be a positive 32-bit integer in seconds"))
        })
        .transpose()?;
    let preview_bytes = optional_preview_bytes(args)?;
    if args
        .get("background")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(invalid("background must be a boolean"));
    }
    Ok(CommandRequest {
        invocation,
        cwd,
        timeout_seconds,
        preview_bytes,
    })
}

fn optional_preview_bytes(args: &Value) -> Result<Option<usize>, ToolResultEnvelope> {
    args.get("preview_bytes")
        .map(|value| {
            value
                .as_u64()
                .filter(|n| (1..=MAX_REQUESTED_PREVIEW_BYTES as u64).contains(n))
                .map(|n| n as usize)
                .ok_or_else(|| {
                    invalid(format!(
                        "preview_bytes must be an integer from 1 to {MAX_REQUESTED_PREVIEW_BYTES}"
                    ))
                })
        })
        .transpose()
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
