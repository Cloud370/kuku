use std::borrow::Cow;
use std::path::Path;

use crate::execution::{CommandInvocation, OutputStream};

use super::super::reports::{pytest_report, rust_report};
use super::rules::{self, Rule};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Format {
    Cargo,
    Pytest,
    GoTest,
    TypeScript,
    GitStatus,
}

pub(super) enum Observation {
    Finished,
    Failed,
}

impl Format {
    pub(super) fn select(invocation: &CommandInvocation) -> Option<(Self, &'static Rule)> {
        let CommandInvocation::Program { program, args } = invocation else {
            return None;
        };
        let basename = Path::new(program).file_name()?.to_str()?;
        let basename = if cfg!(windows) {
            Cow::Owned(basename.to_ascii_lowercase())
        } else {
            Cow::Borrowed(basename)
        };
        let name = if cfg!(windows) {
            basename.strip_suffix(".exe").unwrap_or(&basename)
        } else {
            &basename
        };
        let format = match name {
            "cargo" if cargo_human_args(args) => Self::Cargo,
            "pytest" if pytest_human_args(args) => Self::Pytest,
            "go" if go_test_human_args(args) => Self::GoTest,
            "tsc" if typescript_args(args) => Self::TypeScript,
            "git" if git_status_args(args) => Self::GitStatus,
            name if python_launcher(name) => match args.as_slice() {
                [mode, module, flags @ ..]
                    if mode == "-m" && module == "pytest" && pytest_human_args(flags) =>
                {
                    Self::Pytest
                }
                _ => return None,
            },
            _ => return None,
        };
        let mut command = name.to_owned();
        for arg in args {
            command.push(' ');
            if arg.is_empty() || arg.chars().any(char::is_whitespace) {
                command.push_str(&serde_json::Value::String(arg.clone()).to_string());
            } else {
                command.push_str(arg);
            }
        }
        Some((format, rules::select(&command)?))
    }

    pub(super) fn observe(self, stream: OutputStream, line: &str) -> Option<Observation> {
        match self {
            Self::Cargo => cargo_observation(line),
            Self::Pytest if stream == OutputStream::Stdout => pytest_report(line).map(|report| {
                if report.failed != 0 || report.errors != 0 {
                    Observation::Failed
                } else {
                    Observation::Finished
                }
            }),
            Self::Pytest | Self::GoTest | Self::TypeScript | Self::GitStatus => None,
        }
    }
}

fn go_test_human_args(args: &[String]) -> bool {
    let Some((command, options)) = args.split_first() else {
        return false;
    };
    command == "test"
        && !options.iter().any(|arg| {
            matches!(
                arg.split('=')
                    .next()
                    .unwrap_or_default()
                    .trim_start_matches('-'),
                "" | "json"
                    | "test.json"
                    | "v"
                    | "test.v"
                    | "list"
                    | "test.list"
                    | "bench"
                    | "test.bench"
                    | "cover"
                    | "covermode"
                    | "coverpkg"
                    | "coverprofile"
                    | "test.coverprofile"
                    | "args"
                    | "c"
                    | "o"
                    | "h"
                    | "help"
            ) && arg.starts_with('-')
        })
}

fn typescript_args(args: &[String]) -> bool {
    !args.iter().any(|arg| {
        matches!(
            arg.split('=').next().unwrap_or_default(),
            "--watch" | "-w" | "--showConfig" | "--help" | "-h" | "--version" | "-v"
        )
    })
}

fn git_status_args(args: &[String]) -> bool {
    let Some((name, options)) = args.split_first() else {
        return false;
    };
    let options = options
        .iter()
        .map(String::as_str)
        .take_while(|arg| *arg != "--");
    name == "status"
        && options.clone().any(|arg| {
            matches!(
                arg,
                "--short"
                    | "-s"
                    | "-sb"
                    | "-bs"
                    | "--porcelain"
                    | "--porcelain=v1"
                    | "--porcelain=1"
            )
        })
        && !options.clone().any(|arg| {
            arg.starts_with("--porcelain=") && !matches!(arg, "--porcelain=v1" | "--porcelain=1")
        })
        && !options
            .clone()
            .any(|arg| matches!(arg, "-z" | "--null" | "--long" | "--verbose" | "-v" | "-vv"))
}

fn cargo_human_args(args: &[String]) -> bool {
    !args.iter().any(|arg| {
        matches!(
            arg.split('=').next().unwrap_or_default(),
            "--message-format"
                | "--format"
                | "--nocapture"
                | "--no-capture"
                | "--show-output"
                | "-v"
                | "-vv"
                | "--verbose"
                | "-Z"
                | "--config"
        )
    })
}

fn pytest_human_args(args: &[String]) -> bool {
    !args.iter().any(|arg| {
        matches!(
            arg.split('=').next().unwrap_or_default(),
            "--collect-only"
                | "--co"
                | "--fixtures"
                | "--fixtures-per-test"
                | "--capture"
                | "--debug"
                | "--pdb"
                | "--trace"
                | "--json-report"
                | "--junitxml"
                | "--junit-xml"
                | "--setup-plan"
                | "--setup-show"
                | "--setup-only"
                | "-s"
        )
    })
}

fn python_launcher(program: &str) -> bool {
    matches!(program, "python" | "python3" | "py")
        || program.strip_prefix("python3.").is_some_and(|version| {
            !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn cargo_observation(line: &str) -> Option<Observation> {
    if line.starts_with("error:")
        || line.starts_with("error[")
        || (line.starts_with("test ") && line.ends_with(" ... FAILED"))
        || line.starts_with("---- ")
        || (line.starts_with("thread ") && line.contains("panicked at "))
    {
        return Some(Observation::Failed);
    }
    if line.starts_with("Finished ") && line.contains(" in ") {
        return Some(Observation::Finished);
    }
    rust_report(line).map(|report| {
        if report.failed == 0 {
            Observation::Finished
        } else {
            Observation::Failed
        }
    })
}
