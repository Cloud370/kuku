use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(windows)]
use std::os::windows::ffi::OsStringExt;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use tokio::process::Command;
#[cfg(windows)]
use windows_sys::Win32::Foundation::MAX_PATH;
#[cfg(windows)]
use windows_sys::Win32::System::SystemInformation::{GetSystemDirectoryW, GetWindowsDirectoryW};

use crate::execution::CommandShell;

const SHELLS: [CommandShell; 6] = [
    CommandShell::Sh,
    CommandShell::Bash,
    CommandShell::Zsh,
    CommandShell::Cmd,
    CommandShell::Powershell,
    CommandShell::Pwsh,
];

pub(crate) fn available_shells() -> Vec<CommandShell> {
    let search = ShellSearch::new();
    SHELLS
        .into_iter()
        .filter(|shell| search.find(*shell).is_some())
        .collect()
}

pub(super) fn shell_command(shell: CommandShell, text: &str) -> Option<Command> {
    let program = ShellSearch::new().find(shell)?;
    let mut command = Command::new(program);
    match shell {
        CommandShell::Sh | CommandShell::Bash | CommandShell::Zsh => {
            command.args(["-c", text]);
        }
        CommandShell::Powershell | CommandShell::Pwsh => {
            command.args(["-NoLogo", "-NoProfile", "-NonInteractive"]);
            // NOTE: -Command keeps stderr in text form. A bare "-" selects stdin and NUL cannot travel in argv.
            if text == "-" || text.contains('\0') {
                let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
                command.arg("-EncodedCommand").arg(STANDARD.encode(utf16));
            } else {
                command.args(["-Command", text]);
            }
        }
        CommandShell::Cmd => apply_cmd_arguments(&mut command, text),
    }
    Some(command)
}

struct ShellSearch {
    directories: Vec<PathBuf>,
    #[cfg(windows)]
    system: Option<PathBuf>,
}

impl ShellSearch {
    fn new() -> Self {
        let host_dir = env::current_dir().ok();
        #[cfg(windows)]
        let system = get_directory(GetSystemDirectoryW);
        #[cfg(windows)]
        let mut directories: Vec<_> = [
            env::current_exe()
                .ok()
                .and_then(|path| path.parent().map(Path::to_path_buf)),
            system.clone(),
            get_directory(GetWindowsDirectoryW),
        ]
        .into_iter()
        .flatten()
        .collect();
        #[cfg(not(windows))]
        let mut directories = Vec::new();

        let path = env::var_os("PATH").unwrap_or_else(default_path);
        directories.extend(env::split_paths(&path).filter_map(|dir| {
            if cfg!(windows) && dir.as_os_str().is_empty() {
                return None;
            }
            if dir.is_absolute() {
                Some(dir)
            } else {
                host_dir.as_ref().map(|cwd| cwd.join(dir))
            }
        }));
        Self {
            directories,
            #[cfg(windows)]
            system,
        }
    }

    fn find(&self, shell: CommandShell) -> Option<PathBuf> {
        let program = shell_program(shell);
        for directory in &self.directories {
            let path = directory.join(&program);
            if is_executable(&path) {
                return Some(path);
            }
            #[cfg(windows)]
            if shell == CommandShell::Powershell && self.system.as_ref() == Some(directory) {
                let path = directory
                    .join("WindowsPowerShell")
                    .join("v1.0")
                    .join(&program);
                if is_executable(&path) {
                    return Some(path);
                }
            }
        }
        None
    }
}

fn shell_program(shell: CommandShell) -> String {
    let name = match shell {
        CommandShell::Sh => "sh",
        CommandShell::Bash => "bash",
        CommandShell::Zsh => "zsh",
        CommandShell::Cmd => return "cmd.exe".into(),
        CommandShell::Powershell => return "powershell.exe".into(),
        CommandShell::Pwsh => "pwsh",
    };
    format!("{name}{}", env::consts::EXE_SUFFIX)
}

#[cfg(windows)]
fn apply_cmd_arguments(command: &mut Command, text: &str) {
    // NOTE: /S removes the enclosing quotes before cmd parses the original command text.
    command
        .args(["/D", "/S", "/C"])
        .raw_arg(format!("\"{text}\""));
}

#[cfg(not(windows))]
fn apply_cmd_arguments(command: &mut Command, text: &str) {
    command.args(["/D", "/S", "/C", text]);
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // NOTE: The live NUL-terminated path is passed with the effective process credentials used for execution.
    (unsafe { libc::faccessat(libc::AT_FDCWD, path.as_ptr(), libc::X_OK, libc::AT_EACCESS) }) == 0
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(unix)]
pub(super) fn default_path() -> OsString {
    let mut path = [0u8; 256];
    // NOTE: confstr writes at most the supplied byte capacity and includes the terminating NUL in its length.
    let length = unsafe { libc::confstr(libc::_CS_PATH, path.as_mut_ptr().cast(), path.len()) };
    if length > 0 && length <= path.len() {
        OsString::from_vec(path[..length - 1].to_vec())
    } else {
        OsString::from("/usr/bin:/bin")
    }
}

#[cfg(not(unix))]
fn default_path() -> OsString {
    OsString::new()
}

#[cfg(windows)]
fn get_directory(query: unsafe extern "system" fn(*mut u16, u32) -> u32) -> Option<PathBuf> {
    let mut path = [0u16; MAX_PATH as usize];
    // NOTE: The Win32 directory query may write at most the supplied number of UTF-16 units to this live buffer.
    let length = unsafe { query(path.as_mut_ptr(), path.len() as u32) } as usize;
    (length > 0 && length < path.len()).then(|| OsString::from_wide(&path[..length]).into())
}
