use std::ffi::OsStr;
use std::io;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tokio::process::Command;

use crate::execution::{CommandInvocation, CommandShell};
use crate::tool::{ToolDefinition, ToolResultEnvelope};

use super::shell::shell_command;

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
}

impl CommandRequest {
    pub(super) fn command(&self, memory_bytes: Option<NonZeroU64>) -> io::Result<PreparedCommand> {
        #[cfg(not(unix))]
        let _ = memory_bytes;
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
                super::native::configure_memory(&mut command, memory_bytes)?;
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
                #[cfg(unix)]
                let command = {
                    let mut command = command;
                    super::native::configure_memory(&mut command, memory_bytes)?;
                    command
                };
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

pub(crate) fn changed_command_directory(args: &Value) -> Option<ToolResultEnvelope> {
    let cwd = Path::new(args["cwd"].as_str()?);
    (!cwd
        .canonicalize()
        .is_ok_and(|current| current == cwd && current.is_dir()))
    .then(|| {
        ToolResultEnvelope::blocked(
            "command directory changed after preparation",
            "choose the working directory again before requesting another command",
        )
    })
}

fn apply_command_defaults(
    args: &mut Value,
    definition: &ToolDefinition,
) -> Result<(), ToolResultEnvelope> {
    let fields = args
        .as_object_mut()
        .ok_or_else(|| invalid("run_command expects an object"))?;
    let properties = &definition.input_schema["properties"];
    if let Some(field) = fields
        .keys()
        .find(|field| properties.get(field.as_str()).is_none())
    {
        if field == "background" {
            return Err(invalid(
                "Set background on the outer run_js call or a direct Session run_command call.",
            ));
        }
        if field == "title" {
            return Err(invalid(
                "Set title on the outer run_js({title, code}) call or on a direct Session run_command call.",
            ));
        }
        if field == "timeout" {
            return Err(invalid(
                "run_command does not accept \"timeout\". Omit it. The host controls process deadlines.",
            ));
        }
        return Err(invalid(format!("run_command does not accept {field:?}")));
    }
    let shell_text = fields.contains_key("command");
    let mode_field = if shell_text { "shell" } else { "args" };
    for key in ["cwd", mode_field] {
        if let Some(default) = properties[key].get("default") {
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
                | "title"
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
        return Err(invalid(
            "No default shell is advertised. Supply a listed shell with command, or use program and literal args.",
        ));
    }
    let program = args
        .get("program")
        .and_then(Value::as_str)
        .filter(|program| !program.trim().is_empty())
        .ok_or_else(|| invalid("run_command requires program and literal args"))?;
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
    if cfg!(windows) && is_batch_file(program) {
        return Err(invalid(
            "Windows batch files require an explicit cmd.exe invocation",
        ));
    }
    Ok(CommandInvocation::Program {
        program: program.to_string(),
        args,
    })
}

fn is_batch_file(program: &str) -> bool {
    let Some(name) = Path::new(program).file_name().and_then(OsStr::to_str) else {
        return false;
    };
    let name = name.trim_end_matches([' ', '.']);
    Path::new(name).extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("bat") || extension.eq_ignore_ascii_case("cmd")
    })
}

fn invalid(message: impl Into<String>) -> ToolResultEnvelope {
    let message = message.into();
    ToolResultEnvelope::error(format!("failed: {message}"), message)
}

pub(crate) fn prepared_command_guard(args: &Value) -> Option<&'static str> {
    super::guard::blocked_command_reason(&invocation(args).ok()?)
}
