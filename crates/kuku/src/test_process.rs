use std::ffi::OsStr;
use std::process::Command;

const ISOLATED_TEST: &str = "_KUKU_ISOLATED_TEST";

/// Run one test with its environment configured at process creation.
pub fn isolate_test<T>(
    module: &str,
    name: &str,
    configure: impl FnOnce(&mut Command) -> T,
) -> bool {
    let name = match module.split_once("::") {
        Some((_, module)) => format!("{module}::{name}"),
        None => name.to_owned(),
    };
    if std::env::var_os(ISOLATED_TEST).as_deref() == Some(OsStr::new(&name)) {
        return true;
    }
    let executable = std::env::current_exe().expect("locate the Rust test executable");
    let mut command = Command::new(executable);
    command
        .args(["--exact", &name, "--include-ignored", "--nocapture"])
        .env(ISOLATED_TEST, &name)
        .env("RUST_TEST_COLOR", "never");
    let _fixture = configure(&mut command);
    let output = command.output().expect("start the isolated test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed;"),
        "isolated test {name} failed ({}):\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    false
}
