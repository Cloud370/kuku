//! Builds Git commands with inherited and repository-configured helpers disabled.

use crate::platform::RootCommand;

const GIT_PREFIX: [&str; 4] = [
    "--no-pager",
    "--literal-pathspecs",
    "-c",
    "core.fsmonitor=false",
];

pub(super) fn git_command(args: &[&str], filters: &[String]) -> RootCommand {
    let mut command = RootCommand::new("git").args(GIT_PREFIX);
    command = command
        .arg("-c")
        .arg("core.attributesfile=/dev/null")
        .arg("-c")
        .arg("core.autocrlf=false")
        .arg("-c")
        .arg("core.safecrlf=false");
    for filter in filters {
        for key in ["clean", "smudge", "process"] {
            command = command.arg("-c").arg(format!("filter.{filter}.{key}="));
        }
        command = command
            .arg("-c")
            .arg(format!("filter.{filter}.required=false"));
    }
    command = command.args(args.iter().copied());
    let null_config = if cfg!(windows) { "NUL" } else { "/dev/null" };
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null_config)
        .env("LC_ALL", "C")
        .env("LANG", "C")
}
