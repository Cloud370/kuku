use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::session::validate_session_id;

use super::HostKind;

pub fn logs_root(kuku_home: &Path) -> PathBuf {
    kuku_home.join("logs")
}

pub fn session_log_path(kuku_home: &Path, session_id: &str) -> Result<PathBuf> {
    validate_session_id(session_id)?;
    Ok(logs_root(kuku_home)
        .join("session")
        .join(format!("{session_id}.jsonl")))
}

pub fn runtime_log_path(kuku_home: &Path, day: &str) -> Result<PathBuf> {
    validate_log_day(day)?;
    Ok(logs_root(kuku_home)
        .join("runtime")
        .join(format!("{day}.jsonl")))
}

pub fn host_log_path(kuku_home: &Path, host: HostKind, day: &str) -> Result<PathBuf> {
    validate_log_day(day)?;
    Ok(logs_root(kuku_home)
        .join("host")
        .join(host.as_str())
        .join(format!("{day}.jsonl")))
}

fn validate_log_day(day: &str) -> Result<()> {
    let valid = day.len() == 10
        && day
            .bytes()
            .enumerate()
            .all(|(index, byte)| matches!((index, byte), (4 | 7, b'-') | (_, b'0'..=b'9')));
    if !valid {
        return Err(Error::InvalidArgument(format!(
            "invalid log day segment: {day}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_log_path_uses_host_kind_directory() {
        let path =
            host_log_path(Path::new("/tmp/kuku-home"), HostKind::Webui, "2026-06-06").unwrap();
        assert_eq!(
            path,
            PathBuf::from("/tmp/kuku-home/logs/host/webui/2026-06-06.jsonl")
        );
    }

    #[test]
    fn session_log_path_rejects_escape_like_session_ids() {
        let error = session_log_path(Path::new("/tmp/kuku-home"), "../outside").unwrap_err();
        assert!(matches!(error, Error::InvalidSessionId(value) if value == "../outside"));
    }

    #[test]
    fn runtime_log_path_rejects_invalid_day_segment() {
        let error = runtime_log_path(Path::new("/tmp/kuku-home"), "../outside").unwrap_err();
        assert!(error.to_string().contains("invalid log day segment"));
    }
}
