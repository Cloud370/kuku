use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

static LOG_WRITE_LOCKS: OnceLock<Mutex<std::collections::HashMap<PathBuf, Arc<Mutex<()>>>>> =
    OnceLock::new();

use crate::error::Result;

use super::{LogLevel, LogRecord};

pub struct BufferedLogWriter {
    path: PathBuf,
    buffer: Vec<LogRecord>,
    flush_every: usize,
    post_flush_every: Option<usize>,
    successful_flushes: usize,
    post_flush: Option<Box<dyn FnMut() -> Result<()> + Send>>,
    #[cfg(test)]
    fail_after_bytes: Option<usize>,
}

impl std::fmt::Debug for BufferedLogWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BufferedLogWriter")
            .field("path", &self.path)
            .field("buffer", &self.buffer)
            .field("flush_every", &self.flush_every)
            .field("post_flush_every", &self.post_flush_every)
            .field("successful_flushes", &self.successful_flushes)
            .finish_non_exhaustive()
    }
}

impl BufferedLogWriter {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self::with_flush_every(path, 64)
    }

    pub fn with_flush_every(path: impl AsRef<Path>, flush_every: usize) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            buffer: Vec::new(),
            flush_every: flush_every.max(1),
            post_flush_every: None,
            successful_flushes: 0,
            post_flush: None,
            #[cfg(test)]
            fail_after_bytes: None,
        }
    }

    pub fn with_post_flush_every(
        mut self,
        post_flush_every: usize,
        post_flush: Box<dyn FnMut() -> Result<()> + Send>,
    ) -> Self {
        self.post_flush_every = Some(post_flush_every.max(1));
        self.post_flush = Some(post_flush);
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    pub(crate) fn set_fail_after_bytes(&mut self, fail_after_bytes: Option<usize>) {
        self.fail_after_bytes = fail_after_bytes;
    }

    pub fn push(&mut self, record: LogRecord) -> Result<()> {
        let flush_immediately = matches!(record.level, LogLevel::Warn | LogLevel::Error);
        self.buffer.push(record);
        if flush_immediately || self.buffer.len() >= self.flush_every {
            self.flush()?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let lock = log_write_lock(&self.path);
        let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut file_lock = log_file_lock(&self.path)?;
        file_lock.lock()?;

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&self.path)?;
        #[cfg(test)]
        run_before_append_hook(&self.path)?;

        let completed = match append_records(
            &mut file,
            &self.buffer,
            #[cfg(test)]
            self.fail_after_bytes,
        ) {
            Ok(completed) => completed,
            Err(error) => {
                self.buffer.drain(0..error.completed);
                return Err(error.error);
            }
        };
        self.buffer.drain(0..completed);
        self.run_post_flush_if_due()?;
        Ok(())
    }

    fn run_post_flush_if_due(&mut self) -> Result<()> {
        let Some(post_flush_every) = self.post_flush_every else {
            return Ok(());
        };
        self.successful_flushes += 1;
        if !self.successful_flushes.is_multiple_of(post_flush_every) {
            return Ok(());
        }
        if let Some(post_flush) = self.post_flush.as_mut() {
            post_flush()?;
        }
        Ok(())
    }
}

fn log_file_lock(path: &Path) -> Result<fslock::LockFile> {
    let lock_path = path.with_extension(format!(
        "{}lock",
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!("{extension}."))
            .unwrap_or_default()
    ));
    Ok(fslock::LockFile::open(&lock_path)?)
}

fn log_write_lock(path: &Path) -> Arc<Mutex<()>> {
    let locks = LOG_WRITE_LOCKS.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mut locks = locks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks
        .entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

#[cfg(test)]
type BeforeAppendHook = Box<dyn Fn(&Path) -> std::io::Result<()> + Send + Sync>;

#[cfg(test)]
struct ScopedBeforeAppendHook {
    path: PathBuf,
    hook: BeforeAppendHook,
}

#[cfg(test)]
fn before_append_hook() -> &'static Mutex<Option<ScopedBeforeAppendHook>> {
    static HOOK: OnceLock<Mutex<Option<ScopedBeforeAppendHook>>> = OnceLock::new();
    HOOK.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
fn set_before_append_hook(path: PathBuf, hook: BeforeAppendHook) {
    *before_append_hook().lock().unwrap() = Some(ScopedBeforeAppendHook { path, hook });
}

#[cfg(test)]
fn run_before_append_hook(path: &Path) -> std::io::Result<()> {
    let hook = {
        let mut hook = before_append_hook().lock().unwrap();
        if hook.as_ref().is_some_and(|hook| hook.path == path) {
            hook.take()
        } else {
            None
        }
    };
    if let Some(hook) = hook {
        (hook.hook)(path)?;
    }
    Ok(())
}

struct AppendError {
    completed: usize,
    error: crate::error::Error,
}

fn append_records(
    writer: &mut std::fs::File,
    records: &[LogRecord],
    #[cfg(test)] fail_after_bytes: Option<usize>,
) -> std::result::Result<usize, AppendError> {
    #[cfg(test)]
    {
        let mut remaining = fail_after_bytes;
        let mut completed = 0;
        for record in records {
            let line = serialized_line(record).map_err(|error| AppendError { completed, error })?;
            let start = writer
                .metadata()
                .map_err(|error| AppendError {
                    completed,
                    error: error.into(),
                })?
                .len();
            let result = {
                let mut writer = FailAfterWriter::new(writer, &mut remaining);
                write_record_line(&mut writer, &line)
            };
            match result {
                Ok(()) => completed += 1,
                Err(error) => {
                    writer
                        .set_len(start)
                        .map_err(|rollback_error| AppendError {
                            completed,
                            error: rollback_error.into(),
                        })?;
                    writer
                        .seek(SeekFrom::Start(start))
                        .map_err(|rollback_error| AppendError {
                            completed,
                            error: rollback_error.into(),
                        })?;
                    return Err(AppendError {
                        completed,
                        error: error.into(),
                    });
                }
            }
        }
        return Ok(completed);
    }

    #[cfg(not(test))]
    {
        let mut completed = 0;
        for record in records {
            let line = serialized_line(record).map_err(|error| AppendError { completed, error })?;
            let start = writer
                .metadata()
                .map_err(|error| AppendError {
                    completed,
                    error: error.into(),
                })?
                .len();
            match write_record_line(&mut *writer, &line) {
                Ok(()) => completed += 1,
                Err(error) => {
                    writer
                        .set_len(start)
                        .map_err(|rollback_error| AppendError {
                            completed,
                            error: rollback_error.into(),
                        })?;
                    writer
                        .seek(SeekFrom::Start(start))
                        .map_err(|rollback_error| AppendError {
                            completed,
                            error: rollback_error.into(),
                        })?;
                    return Err(AppendError {
                        completed,
                        error: error.into(),
                    });
                }
            }
        }
        Ok(completed)
    }
}

fn serialized_line(record: &LogRecord) -> Result<Vec<u8>> {
    let mut line = serde_json::to_vec(record)?;
    line.push(b'\n');
    Ok(line)
}

fn write_record_line<W: Write>(writer: &mut W, line: &[u8]) -> std::io::Result<()> {
    writer.write_all(line)?;
    writer.flush()
}

#[cfg(test)]
struct FailAfterWriter<'a, W> {
    inner: &'a mut W,
    remaining: &'a mut Option<usize>,
}

#[cfg(test)]
impl<'a, W> FailAfterWriter<'a, W> {
    fn new(inner: &'a mut W, remaining: &'a mut Option<usize>) -> Self {
        Self { inner, remaining }
    }
}

#[cfg(test)]
impl<W: Write> Write for FailAfterWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match *self.remaining {
            None => self.inner.write(buf),
            Some(0) => Err(std::io::Error::other("simulated partial write failure")),
            Some(remaining) => {
                let allowed = remaining.min(buf.len());
                let written = self.inner.write(&buf[..allowed])?;
                *self.remaining = Some(remaining.saturating_sub(written));
                if written < buf.len() {
                    return Err(std::io::Error::other("simulated partial write failure"));
                }
                Ok(written)
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::{LogLevel, LogScope};

    #[test]
    fn buffered_writer_flushes_jsonl_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/runtime/2026-06-06.jsonl");
        let mut writer = BufferedLogWriter::with_flush_every(&path, 2);

        let mut first = LogRecord::new("2026-06-06T00:00:00Z", LogLevel::Info, LogScope::Runtime);
        first.kind = "runtime.start".to_string();
        first.message = "runtime started".to_string();
        writer.push(first).unwrap();
        assert!(!path.exists());

        let mut second = LogRecord::new("2026-06-06T00:00:01Z", LogLevel::Info, LogScope::Runtime);
        second.kind = "runtime.ready".to_string();
        second.message = "runtime ready".to_string();
        writer.push(second).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content.lines().count(), 2);
    }

    #[test]
    fn buffered_writer_runs_post_flush_hook_only_after_threshold_flushes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/runtime/2026-06-06.jsonl");
        let hook_count = std::sync::Arc::new(std::sync::Mutex::new(0));
        let observed = hook_count.clone();
        let mut writer = BufferedLogWriter::with_flush_every(&path, 1).with_post_flush_every(
            2,
            Box::new(move || {
                *observed.lock().unwrap() += 1;
                Ok(())
            }),
        );

        let mut first = LogRecord::new("2026-06-06T00:00:00Z", LogLevel::Info, LogScope::Runtime);
        first.kind = "runtime.first".to_string();
        first.message = "first".to_string();
        writer.push(first).unwrap();
        assert_eq!(*hook_count.lock().unwrap(), 0);

        let mut second = LogRecord::new("2026-06-06T00:00:01Z", LogLevel::Info, LogScope::Runtime);
        second.kind = "runtime.second".to_string();
        second.message = "second".to_string();
        writer.push(second).unwrap();
        assert_eq!(*hook_count.lock().unwrap(), 1);

        let mut third = LogRecord::new("2026-06-06T00:00:02Z", LogLevel::Info, LogScope::Runtime);
        third.kind = "runtime.third".to_string();
        third.message = "third".to_string();
        writer.push(third).unwrap();
        assert_eq!(*hook_count.lock().unwrap(), 1);
    }

    #[test]
    fn flush_appends_to_existing_file_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/runtime/2026-06-06.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();

        let mut existing =
            LogRecord::new("2026-06-06T00:00:00Z", LogLevel::Info, LogScope::Runtime);
        existing.kind = "runtime.existing".to_string();
        existing.message = "existing record".to_string();
        let existing_line = serialized_line(&existing);
        std::fs::write(&path, &existing_line).unwrap();

        let mut writer = BufferedLogWriter::with_flush_every(&path, 8);
        let mut appended =
            LogRecord::new("2026-06-06T00:00:01Z", LogLevel::Info, LogScope::Runtime);
        appended.kind = "runtime.appended".to_string();
        appended.message = "appended record".to_string();
        writer.push(appended).unwrap();

        writer.flush().unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(&existing_line));
        let records: Vec<LogRecord> = content
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].kind, "runtime.existing");
        assert_eq!(records[1].kind, "runtime.appended");
    }

    #[test]
    fn flush_uses_append_mode_when_file_grows_after_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/runtime/2026-06-06.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "before\n").unwrap();

        set_before_append_hook(
            path.clone(),
            Box::new(|path| {
                let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
                file.write_all(b"external\n")?;
                Ok(())
            }),
        );

        let mut writer = BufferedLogWriter::with_flush_every(&path, 8);
        let mut appended =
            LogRecord::new("2026-06-06T00:00:01Z", LogLevel::Info, LogScope::Runtime);
        appended.kind = "runtime.appended".to_string();
        appended.message = "appended record".to_string();
        writer.push(appended).unwrap();
        writer.flush().unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines[0], "before");
        assert_eq!(lines[1], "external");
        let appended: LogRecord = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(appended.kind, "runtime.appended");
    }

    #[test]
    fn multiple_writer_instances_append_complete_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/runtime/2026-06-06.jsonl");

        let mut first = BufferedLogWriter::with_flush_every(&path, 8);
        let mut second = BufferedLogWriter::with_flush_every(&path, 8);

        let mut first_record =
            LogRecord::new("2026-06-06T00:00:00Z", LogLevel::Info, LogScope::Runtime);
        first_record.kind = "runtime.first".to_string();
        first_record.message = "first record".to_string();
        first.push(first_record).unwrap();

        let mut second_record =
            LogRecord::new("2026-06-06T00:00:01Z", LogLevel::Info, LogScope::Runtime);
        second_record.kind = "runtime.second".to_string();
        second_record.message = "second record".to_string();
        second.push(second_record).unwrap();

        first.flush().unwrap();
        second.flush().unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let records: Vec<LogRecord> = content
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].kind, "runtime.first");
        assert_eq!(records[1].kind, "runtime.second");
    }

    #[test]
    fn warn_and_error_records_flush_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/runtime/2026-06-06.jsonl");
        let mut writer = BufferedLogWriter::with_flush_every(&path, 64);

        let mut info = LogRecord::new("2026-06-06T00:00:00Z", LogLevel::Info, LogScope::Runtime);
        info.kind = "runtime.info".to_string();
        info.message = "info record".to_string();
        writer.push(info).unwrap();
        assert!(!path.exists());

        let mut warn = LogRecord::new("2026-06-06T00:00:01Z", LogLevel::Warn, LogScope::Runtime);
        warn.kind = "runtime.warn".to_string();
        warn.message = "warn record".to_string();
        writer.push(warn).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content.lines().count(), 2);

        let mut error = LogRecord::new("2026-06-06T00:00:02Z", LogLevel::Error, LogScope::Runtime);
        error.kind = "runtime.error".to_string();
        error.message = "error record".to_string();
        writer.push(error).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content.lines().count(), 3);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn flush_error_preserves_buffer_for_retry() {
        let mut writer = BufferedLogWriter::with_flush_every("/dev/full", 8);

        let mut first = LogRecord::new("2026-06-06T00:00:00Z", LogLevel::Info, LogScope::Runtime);
        first.kind = "runtime.start".to_string();
        first.message = "runtime started".to_string();
        writer.push(first).unwrap();

        let mut second = LogRecord::new("2026-06-06T00:00:01Z", LogLevel::Info, LogScope::Runtime);
        second.kind = "runtime.ready".to_string();
        second.message = "runtime ready".to_string();
        writer.push(second).unwrap();

        let error = writer.flush().unwrap_err();
        assert!(!error.to_string().is_empty());
        assert_eq!(writer.buffer.len(), 2);

        let dir = tempfile::tempdir().unwrap();
        writer.path = dir.path().join("logs/runtime/2026-06-06.jsonl");
        writer.flush().unwrap();

        let content = std::fs::read_to_string(writer.path()).unwrap();
        assert_eq!(content.lines().count(), 2);
        assert!(writer.buffer.is_empty());
    }

    #[test]
    fn retry_after_partial_flush_does_not_duplicate_persisted_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/runtime/2026-06-06.jsonl");
        let mut writer = BufferedLogWriter::with_flush_every(&path, 8);

        let mut first = LogRecord::new("2026-06-06T00:00:00Z", LogLevel::Info, LogScope::Runtime);
        first.kind = "runtime.start".to_string();
        first.message = "runtime started".to_string();
        writer.push(first).unwrap();

        let mut second = LogRecord::new("2026-06-06T00:00:01Z", LogLevel::Info, LogScope::Runtime);
        second.kind = "runtime.ready".to_string();
        second.message = "runtime ready".to_string();
        writer.push(second).unwrap();

        let first_line = serialized_line(&writer.buffer[0]);
        writer.fail_after_bytes = Some(first_line.len() + 10);

        let error = writer.flush().unwrap_err();
        assert!(error
            .to_string()
            .contains("simulated partial write failure"));
        assert_eq!(writer.buffer.len(), 1);

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines, vec![first_line.trim_end()]);

        writer.fail_after_bytes = None;
        writer.flush().unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_ne!(lines[0], lines[1]);
    }

    #[test]
    fn partial_current_record_is_rolled_back_so_file_stays_valid_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/runtime/2026-06-06.jsonl");
        let mut writer = BufferedLogWriter::with_flush_every(&path, 8);

        let mut first = LogRecord::new("2026-06-06T00:00:00Z", LogLevel::Info, LogScope::Runtime);
        first.kind = "runtime.start".to_string();
        first.message = "runtime started".to_string();
        writer.push(first).unwrap();

        let mut second = LogRecord::new("2026-06-06T00:00:01Z", LogLevel::Info, LogScope::Runtime);
        second.kind = "runtime.ready".to_string();
        second.message = "runtime ready".to_string();
        writer.push(second).unwrap();

        let first_line = serialized_line(&writer.buffer[0]);
        writer.fail_after_bytes = Some(first_line.len() + 10);

        let error = writer.flush().unwrap_err();
        assert!(error
            .to_string()
            .contains("simulated partial write failure"));

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 1);
        serde_json::from_str::<LogRecord>(lines[0]).unwrap();
        assert!(content.ends_with('\n'));
    }

    #[test]
    fn failed_first_record_preserves_full_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/runtime/2026-06-06.jsonl");
        let mut writer = BufferedLogWriter::with_flush_every(&path, 8);

        let mut first = LogRecord::new("2026-06-06T00:00:00Z", LogLevel::Info, LogScope::Runtime);
        first.kind = "runtime.start".to_string();
        first.message = "runtime started".to_string();
        writer.push(first).unwrap();

        let mut second = LogRecord::new("2026-06-06T00:00:01Z", LogLevel::Info, LogScope::Runtime);
        second.kind = "runtime.ready".to_string();
        second.message = "runtime ready".to_string();
        writer.push(second).unwrap();

        writer.fail_after_bytes = Some(5);

        let error = writer.flush().unwrap_err();
        assert!(error
            .to_string()
            .contains("simulated partial write failure"));
        assert_eq!(writer.buffer.len(), 2);

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.is_empty());
    }

    fn serialized_line(record: &LogRecord) -> String {
        String::from_utf8(super::serialized_line(record).unwrap()).unwrap()
    }
}
