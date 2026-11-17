use std::cmp::Ordering;
use std::ffi::{OsStr, OsString};
use std::io;
use std::iter;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::ExitStatusExt;
use std::path::{Component, Path, PathBuf, Prefix};
use std::process::ExitStatus;
use std::ptr;

use windows_sys::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::Globalization::{
    CompareStringOrdinal, CSTR_EQUAL, CSTR_GREATER_THAN, CSTR_LESS_THAN,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetExitCodeProcess, WaitForSingleObject, CREATE_BREAKAWAY_FROM_JOB,
    CREATE_NEW_PROCESS_GROUP, CREATE_UNICODE_ENVIRONMENT, DETACHED_PROCESS, PROCESS_INFORMATION,
    STARTUPINFOW,
};

const NUL: u16 = 0;
const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;

pub(super) struct DetachedChild(OwnedHandle);

impl DetachedChild {
    pub(super) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let process = self.0.as_raw_handle();
        // NOTE: The owned process handle remains live for this non-blocking wait.
        match unsafe { WaitForSingleObject(process, 0) } {
            WAIT_OBJECT_0 => {
                let mut code = 0;
                // NOTE: The process has exited and the writable code slot is valid.
                if unsafe { GetExitCodeProcess(process, &mut code) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Some(ExitStatus::from_raw(code)))
            }
            WAIT_TIMEOUT => Ok(None),
            WAIT_FAILED => Err(io::Error::last_os_error()),
            result => Err(io::Error::other(format!(
                "unexpected process wait result {result}"
            ))),
        }
    }
}

pub(super) fn spawn_detached(
    executable: &Path,
    args: &[&OsStr],
    data_dir: &Path,
) -> io::Result<DetachedChild> {
    let command = command_line(executable.as_os_str(), args)?;
    let application: Vec<_> = executable.as_os_str().encode_wide().chain([NUL]).collect();
    let directory = process_directory(data_dir);
    let environment = environment_block(data_dir.as_os_str());
    let detached = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_UNICODE_ENVIRONMENT;
    match create_process(
        &application,
        command.clone(),
        &directory,
        &environment,
        detached | CREATE_BREAKAWAY_FROM_JOB,
    ) {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            create_process(&application, command, &directory, &environment, detached)
        }
        result => result,
    }
}

fn create_process(
    application: &[u16],
    mut command: Vec<u16>,
    directory: &[u16],
    environment: &[u16],
    flags: u32,
) -> io::Result<DetachedChild> {
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    // NOTE: CreateProcessW reads NUL-terminated UTF-16 inputs and can write to the command line.
    // Stable Command does not expose the handle inheritance flag.
    if unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            flags,
            environment.as_ptr().cast(),
            directory.as_ptr(),
            &startup,
            &mut process,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // NOTE: A successful CreateProcessW returns owned process and initial thread handles.
    unsafe {
        drop(OwnedHandle::from_raw_handle(process.hThread));
        Ok(DetachedChild(OwnedHandle::from_raw_handle(
            process.hProcess,
        )))
    }
}

fn command_line(executable: &OsStr, args: &[&OsStr]) -> io::Result<Vec<u16>> {
    let mut line = Vec::new();
    push_quoted_arg(&mut line, executable)?;
    for arg in args {
        line.push(u16::from(b' '));
        push_quoted_arg(&mut line, arg)?;
    }
    line.push(NUL);
    Ok(line)
}

fn push_quoted_arg(line: &mut Vec<u16>, arg: &OsStr) -> io::Result<()> {
    line.push(QUOTE);
    let mut slashes = 0;
    for unit in arg.encode_wide() {
        match unit {
            NUL => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "argument contains NUL",
                ))
            }
            BACKSLASH => slashes += 1,
            unit => {
                let escaped = if unit == QUOTE {
                    slashes * 2 + 1
                } else {
                    slashes
                };
                line.extend(iter::repeat_n(BACKSLASH, escaped));
                line.push(unit);
                slashes = 0;
            }
        }
    }
    line.extend(iter::repeat_n(BACKSLASH, slashes * 2));
    line.push(QUOTE);
    Ok(())
}

fn process_directory(path: &Path) -> Vec<u16> {
    let mut components = path.components();
    let ordinary = if let Some(Component::Prefix(prefix)) = components.next() {
        match prefix.kind() {
            Prefix::VerbatimDisk(drive) => Some(OsString::from(format!("{}:", char::from(drive)))),
            Prefix::VerbatimUNC(server, share) => {
                let mut unc = OsString::from(r"\\");
                unc.push(server);
                unc.push(r"\");
                unc.push(share);
                Some(unc)
            }
            _ => None,
        }
    } else {
        None
    };
    if let Some(ordinary) = ordinary {
        let mut candidate = PathBuf::from(ordinary);
        candidate.extend(components);
        // NOTE: CreateProcessW's working directory uses Win32 path syntax. Use it only
        // when removing the verbatim prefix resolves back to the same directory.
        if candidate
            .canonicalize()
            .is_ok_and(|canonical| canonical == path)
        {
            return candidate.as_os_str().encode_wide().chain([NUL]).collect();
        }
    }
    path.as_os_str().encode_wide().chain([NUL]).collect()
}

fn environment_block(data_dir: &OsStr) -> Vec<u16> {
    let home_key: Vec<_> = "KUKU_HOME".encode_utf16().collect();
    let mut variables: Vec<_> = std::env::vars_os()
        .filter_map(|(name, value)| {
            let name: Vec<_> = name.encode_wide().collect();
            (compare_names(&name, &home_key) != Ordering::Equal).then_some((name, value))
        })
        .collect();
    variables.push((home_key, data_dir.to_owned()));
    variables.sort_unstable_by(|(left, _), (right, _)| compare_names(left, right));
    let mut block = Vec::new();
    for (name, value) in variables {
        block.extend(name);
        block.push(u16::from(b'='));
        block.extend(value.encode_wide());
        block.push(NUL);
    }
    block.push(NUL);
    block
}

fn compare_names(left: &[u16], right: &[u16]) -> Ordering {
    // NOTE: CompareStringOrdinal reads the explicit lengths of these UTF-16 buffers.
    // Windows bounds environment variable names well below i32::MAX.
    match unsafe {
        CompareStringOrdinal(
            left.as_ptr(),
            left.len() as i32,
            right.as_ptr(),
            right.len() as i32,
            1,
        )
    } {
        CSTR_LESS_THAN => Ordering::Less,
        CSTR_EQUAL => Ordering::Equal,
        CSTR_GREATER_THAN => Ordering::Greater,
        _ => left.cmp(right),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::command_line;

    #[test]
    fn native_command_line_quotes_spaces_trailing_slashes_and_literal_quotes() {
        let command = command_line(
            OsStr::new(r"C:\Program Files\kuku.exe"),
            &[
                OsStr::new("__managed-server"),
                OsStr::new(r"C:\路径 空格\"),
                OsStr::new(r#"say \"hello""#),
                OsStr::new(""),
            ],
        )
        .unwrap();
        let expected = r#""C:\Program Files\kuku.exe" "__managed-server" "C:\路径 空格\\" "say \\\"hello\"" """#;
        assert_eq!(
            expected.encode_utf16().chain([0]).collect::<Vec<_>>(),
            command,
        );
    }
}
