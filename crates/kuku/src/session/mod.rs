pub mod id;
pub mod list;
pub mod paths;

pub use id::{new_session_id, validate_session_id};
pub use list::{list_sessions, SessionSummary};
pub use paths::{
    current_workspace, global_memory_path, kuku_home, project_home, project_memory_path,
    project_policy_path, session_events_path,
};

pub(crate) use paths::session_lock_path;

use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

pub(crate) fn acquire_lock(lock_path: &Path) -> Result<()> {
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Ok(existing) = fs::read_to_string(lock_path) {
        let pid_str = existing.lines().next().unwrap_or("");
        if let Ok(pid) = pid_str.parse::<i32>() {
            if process_alive(pid) {
                return Err(Error::SessionLocked {
                    session: lock_path.parent().unwrap_or(lock_path).to_path_buf(),
                    holder_pid: pid,
                });
            }
        }
    }
    let content = format!(
        "{}\n{}\n",
        std::process::id(),
        time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
    );
    fs::write(lock_path, content)?;
    Ok(())
}

pub(crate) fn release_lock(lock_path: &Path) {
    let _ = fs::remove_file(lock_path);
}

fn process_alive(pid: i32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::PathBuf::from(format!("/proc/{pid}")).exists()
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        unsafe { kill(pid, 0) == 0 }
    }
    #[cfg(target_os = "windows")]
    {
        extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
            fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
        }
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as u32);
            if handle.is_null() {
                false
            } else {
                CloseHandle(handle);
                true
            }
        }
    }
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    #[test]
    fn stale_lock_is_taken_over() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("lock");
        fs::write(&lock_path, "99999\n2020-01-01T00:00:00Z\n").unwrap();
        assert!(acquire_lock(&lock_path).is_ok());
        let content = fs::read_to_string(&lock_path).unwrap();
        assert!(content.contains(&std::process::id().to_string()));
    }

    #[test]
    fn live_lock_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("lock");
        let content = format!("{}\n2020-01-01T00:00:00Z\n", std::process::id());
        fs::write(&lock_path, &content).unwrap();
        assert!(acquire_lock(&lock_path).is_err());
    }
}
