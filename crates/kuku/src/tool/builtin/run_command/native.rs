use std::env;
use std::ffi::{CString, OsStr, OsString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;

use tokio::process::Command;

struct CStringArray {
    _values: Vec<CString>,
    pointers: Vec<*const libc::c_char>,
}

impl CStringArray {
    fn new(values: impl IntoIterator<Item = OsString>) -> io::Result<Self> {
        let values = values
            .into_iter()
            .map(|value| c_string(&value))
            .collect::<io::Result<Vec<_>>>()?;
        let mut pointers: Vec<_> = values.iter().map(|value| value.as_ptr()).collect();
        pointers.push(ptr::null());
        Ok(Self {
            _values: values,
            pointers,
        })
    }
}

// NOTE: These private pointers only borrow immutable CString allocations owned by this value; moving the owner cannot move those allocations.
unsafe impl Send for CStringArray {}
// NOTE: No pointer or backing allocation is exposed for mutation, so sharing this owner only permits immutable reads.
unsafe impl Sync for CStringArray {}

struct NativeExec {
    paths: Vec<CString>,
    search_path: bool,
    argv: CStringArray,
    environment: CStringArray,
}

impl NativeExec {
    fn execute(&self) -> io::Result<()> {
        let mut permission_denied = false;
        for path in &self.paths {
            // NOTE: All NUL-terminated paths, strings and null-terminated pointer arrays were allocated before fork and remain owned here. execve is async-signal-safe and never performs shell fallback.
            unsafe {
                libc::execve(
                    path.as_ptr(),
                    self.argv.pointers.as_ptr(),
                    self.environment.pointers.as_ptr(),
                );
            }
            let error = io::Error::last_os_error();
            if !self.search_path {
                return Err(error);
            }
            match error.raw_os_error() {
                Some(libc::EACCES) => permission_denied = true,
                Some(libc::ENOENT | libc::ENOTDIR) => {}
                _ => return Err(error),
            }
        }
        Err(io::Error::from_raw_os_error(if permission_denied {
            libc::EACCES
        } else {
            libc::ENOENT
        }))
    }
}

pub(super) fn configure(command: &mut Command) -> io::Result<Vec<PathBuf>> {
    let program = Path::new(command.as_std().get_program());
    let environment: Vec<_> = env::vars_os().collect();
    let search_path = environment
        .iter()
        .find(|(name, _)| name == "PATH")
        .map(|(_, value)| value.as_os_str());
    let paths = program_paths(program, search_path);
    let exec = NativeExec {
        paths: paths
            .iter()
            .map(|path| c_string(path.as_os_str()))
            .collect::<io::Result<_>>()?,
        search_path: program.file_name() == Some(program.as_os_str()),
        argv: CStringArray::new(
            std::iter::once(command.as_std().get_program())
                .chain(command.as_std().get_args())
                .map(OsStr::to_os_string),
        )?,
        environment: CStringArray::new(environment.into_iter().map(|(mut name, value)| {
            name.push("=");
            name.push(value);
            name
        }))?,
    };
    // NOTE: Tokio applies cwd, stdio and the process group before this callback. It only reads prepared buffers, calls execve and returns raw errno; no allocation, lock or resource destruction occurs after fork. Its error handshake still reports failures before a Child is returned. The safe Command API can fall back to a shell.
    unsafe {
        command.pre_exec(move || exec.execute());
    }
    Ok(paths)
}

fn program_paths(program: &Path, search_path: Option<&OsStr>) -> Vec<PathBuf> {
    if program.file_name() != Some(program.as_os_str()) {
        return vec![program.to_path_buf()];
    }
    let search_path = search_path
        .map(OsStr::to_os_string)
        .unwrap_or_else(super::shell::default_path);
    env::split_paths(&search_path)
        .map(|directory| directory.join(program))
        .collect()
}

fn c_string(value: &OsStr) -> io::Result<CString> {
    CString::new(value.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "native command contains a NUL byte",
        )
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::io;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;

    use tokio::process::Command;

    use crate::execution::{CommandExecutionFacts, CommandTermination};

    use super::super::run_command;
    use super::configure;

    const FIXTURE: &str = "tool::builtin::run_command::native::tests::native_bytes_fixture";
    const LITERALS: [&str; 3] = ["", "literal ; > interpreted.txt", "中文 🌱"];

    #[tokio::test]
    async fn native_path_search_preserves_cwd_arguments_environment_and_start_errors() {
        let dir = tempfile::tempdir().unwrap();
        let search_path = std::env::join_paths([
            OsString::from("not-directory"),
            OsString::from("missing"),
            OsString::from("blocked"),
            OsString::from("bin-目录"),
            OsString::new(),
            OsString::from("tail"),
        ])
        .unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .env("PATH", search_path)
            .env("KUKU_NATIVE_EXEC_WORKSPACE", dir.path())
            .env(
                "KUKU_NATIVE_EXEC_BYTES",
                OsString::from_vec(b"inherited-\xff".to_vec()),
            )
            .current_dir(dir.path())
            .args([
                "--ignored",
                "--exact",
                "tool::builtin::run_command::native::tests::native_path_fixture",
                "--nocapture",
            ])
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed"));
    }

    #[tokio::test]
    #[ignore = "subprocess fixture for native execution with a controlled PATH and environment"]
    async fn native_path_fixture() {
        let workspace = PathBuf::from(std::env::var_os("KUKU_NATIVE_EXEC_WORKSPACE").unwrap());
        let working = workspace.join("working directory");
        let bin = working.join("bin-目录");
        let blocked = working.join("blocked");
        let tail = working.join("tail");
        for directory in [&bin, &blocked, &tail] {
            fs::create_dir_all(directory).unwrap();
        }
        fs::write(working.join("not-directory"), b"not a directory").unwrap();
        install_binary(&bin.join("probe"));
        install_binary(&working.join("at-cwd"));
        write_program(&blocked.join("probe"), b"not executable", 0o644);
        let args: Vec<_> = ["--ignored", "--exact", FIXTURE, "--nocapture", "--"]
            .into_iter()
            .chain(LITERALS)
            .collect();
        let expected: Vec<_> = LITERALS.iter().map(|arg| arg.as_bytes().to_vec()).collect();
        for program in ["probe", "at-cwd"] {
            let result = run_command(
                &serde_json::json!({"program":program,"args":args,"cwd":"working directory","timeout":10}),
                &workspace,
                None,
                None,
            )
            .await;
            assert_eq!("ok", result.status, "{result:?}");
            let facts = CommandExecutionFacts::from_structured(result.structured.as_ref().unwrap())
                .unwrap();
            assert!(
                facts
                    .output_preview
                    .contains(&serde_json::to_string(&expected).unwrap()),
                "{result:?}"
            );
            assert!(facts.stderr_bytes > 0);
            assert!(facts.started && facts.capture_complete && !facts.cleanup_incomplete);
        }

        let mut raw = Command::new("/bin/sh");
        raw.args([
            "-c",
            "printf '%s' \"$1\"; printf '%s' \"$KUKU_NATIVE_EXEC_BYTES\"",
            "native-fixture",
        ])
        .arg(OsStr::from_bytes(b"arg-\xfd"));
        configure(&mut raw).unwrap();
        let output = raw
            .current_dir(&working)
            .stdin(Stdio::null())
            .process_group(0)
            .output()
            .await
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(b"arg-\xfdinherited-\xff", output.stdout.as_slice());

        write_program(&blocked.join("denied"), b"not executable", 0o644);
        write_program(
            &bin.join("invalid"),
            b"echo unexpected > interpreted.txt\n",
            0o755,
        );
        write_program(
            &tail.join("invalid"),
            b"#!/bin/sh\necho fallback > interpreted.txt\n",
            0o755,
        );
        write_program(
            &bin.join("bad-interpreter"),
            b"#!/missing/kuku-interpreter\n",
            0o755,
        );
        for (program, termination, errno) in [
            ("absent", CommandTermination::ProgramMissing, libc::ENOENT),
            ("denied", CommandTermination::StartFailed, libc::EACCES),
            (
                "./not-directory/child",
                CommandTermination::StartFailed,
                libc::ENOTDIR,
            ),
            ("invalid", CommandTermination::StartFailed, libc::ENOEXEC),
            (
                "bad-interpreter",
                CommandTermination::StartFailed,
                libc::ENOENT,
            ),
        ] {
            assert_start_failure(&workspace, program, &[], termination, Some(errno)).await;
        }

        write_program(
            &bin.join("script"),
            b"#!/bin/sh\nprintf 'shebang:%s' \"$1\"\nprintf script-stderr >&2\n",
            0o755,
        );
        let result = run_command(
            &serde_json::json!({"program":"script","args":[LITERALS[1]],"cwd":"working directory","timeout":5}),
            &workspace, None, None,
        ).await;
        assert_eq!("ok", result.status, "{result:?}");
        assert!(result
            .model_content
            .contains("shebang:literal ; > interpreted.txt"));
        assert!(result.model_content.contains("script-stderr"));
        assert!(!working.join("interpreted.txt").exists());

        assert_start_failure(
            &workspace,
            "absent\0program",
            &[],
            CommandTermination::StartFailed,
            None,
        )
        .await;
        assert_start_failure(
            &workspace,
            "probe",
            &["bad\0argument"],
            CommandTermination::StartFailed,
            None,
        )
        .await;
    }

    #[test]
    #[ignore = "subprocess fixture for native argument, environment and process group bytes"]
    fn native_bytes_fixture() {
        assert_eq!(
            b"inherited-\xff",
            std::env::var_os("KUKU_NATIVE_EXEC_BYTES")
                .unwrap()
                .as_bytes()
        );
        let workspace = PathBuf::from(std::env::var_os("KUKU_NATIVE_EXEC_WORKSPACE").unwrap());
        assert_eq!(
            workspace.join("working directory").canonicalize().unwrap(),
            std::env::current_dir().unwrap()
        );
        // NOTE: getpgrp only reads the current process's group; it has no safe standard-library equivalent.
        assert_eq!(std::process::id() as libc::pid_t, unsafe {
            libc::getpgrp()
        });
        let mut stdin = String::new();
        io::Read::read_to_string(&mut io::stdin(), &mut stdin).unwrap();
        assert!(stdin.is_empty());
        let args: Vec<_> = std::env::args_os()
            .skip_while(|arg| arg != "--")
            .skip(1)
            .map(|arg| arg.as_bytes().to_vec())
            .collect();
        println!("{}", serde_json::to_string(&args).unwrap());
        eprintln!("native-stderr");
    }

    #[tokio::test]
    async fn native_program_without_path_finds_a_system_program() {
        let output = Command::new(std::env::current_exe().unwrap())
            .env_remove("PATH")
            .args([
                "--ignored",
                "--exact",
                "tool::builtin::run_command::native::tests::native_default_path_fixture",
                "--nocapture",
            ])
            .output()
            .await
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed"));
    }

    #[tokio::test]
    #[ignore = "subprocess fixture for native execution without PATH"]
    async fn native_default_path_fixture() {
        assert!(std::env::var_os("PATH").is_none());
        let dir = tempfile::tempdir().unwrap();
        let result = run_command(
            &serde_json::json!({"program":"sh","args":["-c","printf default-path"],"cwd":".","timeout":5}),
            dir.path(), None, None,
        ).await;
        assert_eq!("ok", result.status, "{result:?}");
        assert!(result.model_content.contains("default-path"));
    }

    fn install_binary(path: &Path) {
        let source = std::env::current_exe().unwrap();
        if fs::hard_link(&source, path).is_err() {
            fs::copy(source, path).unwrap();
        }
    }

    fn write_program(path: &Path, contents: &[u8], mode: u32) {
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    async fn assert_start_failure(
        workspace: &Path,
        program: &str,
        args: &[&str],
        termination: CommandTermination,
        errno: Option<i32>,
    ) {
        let result = run_command(
            &serde_json::json!({"program":program,"args":args,"cwd":"working directory","timeout":5}),
            workspace, None, None,
        ).await;
        assert_eq!("error", result.status, "{result:?}");
        let facts =
            CommandExecutionFacts::from_structured(result.structured.as_ref().unwrap()).unwrap();
        assert_eq!(termination, facts.termination, "{result:?}");
        assert_eq!(None, facts.exit_code);
        assert!(!facts.started);
        assert_eq!(0, facts.output_ref.bytes);
        if let Some(errno) = errno {
            assert_eq!(
                Some(io::Error::from_raw_os_error(errno).to_string()),
                facts.start_error
            );
        } else {
            assert!(facts.start_error.is_some());
        }
    }
}
