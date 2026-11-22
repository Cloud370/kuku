use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::config::LogsConfig;

use super::prune::collect_candidates;
use super::store::{log_file_lock, log_write_lock};

const MAX_BYTES: u64 = 64 * 1024 * 1024;
const SEGMENT_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) fn trace_root(home: &Path) -> PathBuf {
    super::logs_root(home).join("provider-trace")
}

pub(crate) fn segment_limit(logs: &LogsConfig) -> u64 {
    budget(logs).min(SEGMENT_BYTES)
}

fn budget(logs: &LogsConfig) -> u64 {
    MAX_BYTES.min(u64::from(logs.max_total_size_mb) * 1024 * 1024)
}

pub(crate) fn with_trace_lock<T>(
    root: &Path,
    action: impl FnOnce() -> crate::Result<T>,
) -> crate::Result<T> {
    let path = root.join("segments");
    let lock = log_write_lock(&path);
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file_lock = log_file_lock(&path)?;
    file_lock.lock()?;
    action()
}

pub(crate) fn append(
    root: &Path,
    limits: &LogsConfig,
    day: time::Date,
    bytes: &[u8],
) -> crate::Result<()> {
    let segment_limit = segment_limit(limits);
    if bytes.is_empty() || bytes.len() as u64 > segment_limit {
        return Ok(());
    }
    fs::create_dir_all(root)?;
    with_trace_lock(root, || {
        let name = format!("current-{day}-{}.jsonl", budget(limits));
        let active = root.join(&name);
        let marker = root.join("active-segment");
        let changed = !fs::read_to_string(&marker).is_ok_and(|previous| previous == name);
        let length = match fs::metadata(&active) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error.into()),
        };
        let rotates = length + bytes.len() as u64 > segment_limit;
        if rotates {
            let name = tempfile::Builder::new()
                .prefix("segment-")
                .suffix(".jsonl")
                .tempfile_in(root)?;
            let path = name.into_temp_path();
            fs::rename(&active, &path)?;
            path.keep().map_err(|error| error.error)?;
        }
        if length == 0 || rotates || changed {
            // Reserve capacity for the whole segment so ordinary appends need no directory scan.
            let max_closed = budget(limits) - segment_limit;
            let mut closed = collect_candidates(root)?;
            closed.retain(|segment| segment.path != active);
            closed.sort_by(|a, b| a.modified_at.cmp(&b.modified_at).then(a.path.cmp(&b.path)));
            let mut closed_bytes = closed.iter().map(|segment| segment.size_bytes).sum::<u64>();
            let now = SystemTime::now();
            let max_age = std::time::Duration::from_secs(u64::from(limits.max_age_days) * 86400);
            for segment in closed {
                if closed_bytes > max_closed
                    || now
                        .duration_since(segment.modified_at)
                        .is_ok_and(|age| age > max_age)
                {
                    fs::remove_file(&segment.path)?;
                    closed_bytes -= segment.size_bytes;
                }
            }
        }
        if changed {
            fs::write(marker, name)?;
        }
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(active)?.write_all(bytes)?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File, FileTimes};
    use std::time::{Duration, SystemTime};

    use crate::config::LogsConfig;

    use super::{append, budget, collect_candidates, trace_root};

    #[test]
    fn concurrent_trace_batches_rotate_within_budget_and_preserve_sessions() {
        let home = tempfile::tempdir().unwrap();
        let root = trace_root(home.path());
        fs::create_dir_all(&root).unwrap();
        let old = root.join("expired.jsonl");
        let file = File::create(&old).unwrap();
        file.set_times(
            FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(172800)),
        )
        .unwrap();
        drop(file);
        let journal = home.path().join("events.jsonl");
        fs::write(&journal, "journal").unwrap();
        let limits = LogsConfig {
            max_age_days: 1,
            max_total_size_mb: 1,
        };
        let day = time::OffsetDateTime::now_utc().date();
        std::thread::scope(|threads| {
            for worker in 0..4 {
                let root = &root;
                let limits = &limits;
                threads.spawn(move || {
                    for frame in 0..12 {
                        let record = serde_json::json!({"session":worker,"frame":frame,"padding":"x".repeat(128 * 1024)});
                        let line = format!("{record}\n");
                        append(root, limits, day, line.as_bytes()).unwrap();
                        super::with_trace_lock(root, || {
                            let segments = collect_candidates(root)?;
                            assert!(segments.iter().map(|segment| segment.size_bytes).sum::<u64>() <= budget(limits));
                            Ok(())
                        }).unwrap();
                    }
                });
            }
        });
        assert!(!old.exists());
        assert_eq!(fs::read_to_string(journal).unwrap(), "journal");
        let segments = collect_candidates(&root).unwrap();
        assert_eq!(segments.len(), 1);
        let retained = fs::read_to_string(&segments[0].path).unwrap();
        assert!(!retained.is_empty());
        for line in retained.lines() {
            serde_json::from_str::<serde_json::Value>(line).unwrap();
        }
    }
}
