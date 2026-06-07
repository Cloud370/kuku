#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
/// Whether a session is still running, finished cleanly, or was interrupted.
pub enum SessionStatus {
    Active,
    Done,
    Interrupted,
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionStatus::Active => write!(f, "active"),
            SessionStatus::Done => write!(f, "done"),
            SessionStatus::Interrupted => write!(f, "interrupted"),
        }
    }
}

pub mod id;
pub mod list;
pub mod paths;

pub use id::{new_session_id, validate_session_id};
pub use list::{list_sessions, SessionSummary};
pub use paths::{
    current_workspace, global_memory_path, host_log_path, kuku_home, project_home,
    project_memory_path, project_policy_path, runtime_log_path, session_events_path,
    session_log_path, HostLogKind,
};

pub(crate) use paths::session_lock_path;

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use crate::error::{Error, Result};

pub(crate) fn acquire_lock(lock_path: &Path) -> Result<()> {
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = format!(
        "{}\n{}\n",
        std::process::id(),
        time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
    );
    loop {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(lock_path)
        {
            Ok(mut file) => {
                use std::io::Write;

                file.write_all(content.as_bytes())?;
                file.flush()?;
                return Ok(());
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                let existing = fs::read_to_string(lock_path)?;
                let pid_str = existing.lines().next().unwrap_or("");
                let Ok(pid) = pid_str.parse::<i32>() else {
                    return Err(Error::SessionLocked {
                        session: lock_path.parent().unwrap_or(lock_path).to_path_buf(),
                        holder_pid: 0,
                    });
                };
                if process_alive(pid) {
                    return Err(Error::SessionLocked {
                        session: lock_path.parent().unwrap_or(lock_path).to_path_buf(),
                        holder_pid: pid,
                    });
                }
                match fs::remove_file(lock_path) {
                    Ok(()) => continue,
                    Err(remove_error) if remove_error.kind() == ErrorKind::NotFound => continue,
                    Err(remove_error) => return Err(remove_error.into()),
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
}

pub(crate) fn release_lock(lock_path: &Path) {
    let _ = fs::remove_file(lock_path);
}

/// Delete a session directory.
/// Returns Error::SessionLocked if the session is actively locked by another process.
pub fn delete_session(kuku_home: &Path, workspace: Option<&Path>, session_id: &str) -> Result<()> {
    let workspace = match workspace {
        Some(ws) => ws.to_path_buf(),
        None => paths::current_workspace()?,
    };
    validate_session_id(session_id)?;
    let lock_path = paths::session_lock_path(kuku_home, &workspace, session_id);
    let events_path = paths::session_events_path(kuku_home, &workspace, session_id)?;

    if let Ok(existing) = fs::read_to_string(&lock_path) {
        let pid_str = existing.lines().next().unwrap_or("");
        if let Ok(pid) = pid_str.parse::<i32>() {
            if process_alive(pid) {
                return Err(Error::SessionLocked {
                    session: events_path.parent().unwrap_or(&events_path).to_path_buf(),
                    holder_pid: pid,
                });
            }
        }
    }

    if let Some(session_dir) = events_path.parent() {
        fs::remove_dir_all(session_dir)?;
    }

    Ok(())
}

pub(crate) fn process_alive(pid: i32) -> bool {
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier};

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

    #[test]
    fn concurrent_acquire_lock_never_allows_two_winners() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("lock");
        let race_observed = Arc::new(AtomicBool::new(false));

        for _ in 0..2000 {
            release_lock(&lock_path);
            let start = Arc::new(Barrier::new(3));
            let finish = Arc::new(Barrier::new(3));
            let winners = Arc::new(std::sync::Mutex::new(Vec::new()));

            let left_start = start.clone();
            let left_finish = finish.clone();
            let left_winners = winners.clone();
            let left_path = lock_path.clone();
            let left = std::thread::spawn(move || {
                left_start.wait();
                if acquire_lock(&left_path).is_ok() {
                    left_winners.lock().unwrap().push(());
                }
                left_finish.wait();
            });

            let right_start = start.clone();
            let right_finish = finish.clone();
            let right_winners = winners.clone();
            let right_path = lock_path.clone();
            let right = std::thread::spawn(move || {
                right_start.wait();
                if acquire_lock(&right_path).is_ok() {
                    right_winners.lock().unwrap().push(());
                }
                right_finish.wait();
            });

            start.wait();
            finish.wait();
            left.join().unwrap();
            right.join().unwrap();

            if winners.lock().unwrap().len() > 1 {
                race_observed.store(true, Ordering::Relaxed);
                break;
            }
        }

        assert!(
            !race_observed.load(Ordering::Relaxed),
            "concurrent acquire_lock calls both succeeded"
        );
    }
}
