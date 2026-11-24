use std::io::{self, Read};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread;

use super::scan::{LINE_BYTES, SCAN_BYTES};
use super::{CollectedFile, ContextPreview, FileSearch, SearchPage, SearchView};
use crate::WorkspaceIoLimits;
use crate::tool::builtin::common::{
    FileCancellation, IoLimit, ScanDeadline, WorkspaceFiles, is_cancellation, is_io_limit,
};

const FILES_PER_WORKER: usize = 1024;
const MAX_EXTRA_THREADS: usize = 7;
const CONTEXT_SCAN_BYTES: u64 = 16 * 1024 * 1024;
const SCAN_CREDIT_BYTES: u64 = 1024 * 1024;

pub(super) struct WorkspaceScan<'a, 'w> {
    pub source: &'a WorkspaceFiles<'w>,
    pub deadline: ScanDeadline,
    pub cancelled: &'a FileCancellation<'a>,
}

#[derive(Default)]
pub(super) struct ScanStats {
    pub searched: usize,
    pub skipped: usize,
    pub unverified: usize,
    pub limit: Option<String>,
    pub unchecked: Option<UncheckedFile>,
}

#[derive(serde::Serialize)]
pub(super) struct UncheckedFile {
    pub path: String,
    pub reason: &'static str,
}

impl ScanStats {
    fn append(&mut self, stats: Self) {
        self.searched += stats.searched;
        self.skipped += stats.skipped;
        self.unverified += stats.unverified;
        self.limit = self.limit.take().or(stats.limit);
        self.unchecked = self.unchecked.take().or(stats.unchecked);
    }

    fn record_unchecked(&mut self, path: &str, reason: &'static str) {
        self.unchecked.get_or_insert_with(|| UncheckedFile {
            path: path.to_owned(),
            reason,
        });
    }
}

struct ContextScan<'a> {
    preview: &'a mut ContextPreview,
    reader: FileSearch,
    bytes_left: u64,
    lines: usize,
}

enum ScanBytes<'a> {
    Local(u64),
    Shared(ByteCredit<'a>),
}

impl ScanBytes<'_> {
    fn exhausted(&self) -> bool {
        match self {
            Self::Local(left) => *left == 0,
            Self::Shared(credit) => credit.left == 0 && credit.shared.exhausted(),
        }
    }
}

impl WorkspaceScan<'_, '_> {
    pub(super) fn run(
        &self,
        mut search: FileSearch,
        files: &[CollectedFile],
        page: &mut SearchPage,
        preview: &mut ContextPreview,
        context: usize,
        limits: WorkspaceIoLimits,
    ) -> io::Result<ScanStats> {
        let context = (context > 0 && page.view == SearchView::Lines).then_some(context);
        if page.offset == 0 && context.is_none() {
            if let Some(workers) = ExtraWorkers::acquire(files.len()) {
                return self.parallel(&search, files, page, limits.scan_bytes, workers.count + 1);
            }
        }

        let context = context.map(|lines| ContextScan {
            preview,
            reader: FileSearch::new("^", limits.search_line_bytes, self.deadline)
                .expect("valid context expression"),
            bytes_left: CONTEXT_SCAN_BYTES,
            lines,
        });
        self.scan_files(
            &mut search,
            files,
            page,
            ScanBytes::Local(limits.scan_bytes),
            context,
        )
    }

    fn parallel(
        &self,
        search: &FileSearch,
        files: &[CollectedFile],
        page: &mut SearchPage,
        scan_bytes: u64,
        workers: usize,
    ) -> io::Result<ScanStats> {
        let mut chunks = Vec::with_capacity(workers);
        let width = files.len().div_ceil(workers);
        let mut start = 0;
        while start < files.len() {
            let mut end = (start + width).min(files.len());
            while end < files.len() && files[end - 1].relative == files[end].relative {
                end += 1;
            }
            chunks.push(&files[start..end]);
            start = end;
        }

        let shared = SharedBytes::new(scan_bytes);
        let limit = page.limit;
        let view = page.view;
        let body_budget = page.body_budget;
        let scan_chunk = |chunk: &[CollectedFile]| {
            let mut page = SearchPage::new(0, limit, view, body_budget);
            let stats = self.scan_files(
                &mut search.clone(),
                chunk,
                &mut page,
                ScanBytes::Shared(shared.credit()),
                None,
            )?;
            Ok::<_, io::Error>((stats, page))
        };

        thread::scope(|scope| {
            let handles: Vec<_> = chunks[1..]
                .iter()
                .map(|&chunk| {
                    let handle = thread::Builder::new().spawn_scoped(scope, || scan_chunk(chunk));
                    (chunk, handle)
                })
                .collect();
            let (mut stats, first) = scan_chunk(chunks[0])?;
            page.append_page(first);
            for (chunk, handle) in handles {
                let output = match handle {
                    Ok(handle) => handle
                        .join()
                        .map_err(|_| io::Error::other("search worker failed"))?,
                    Err(_) => scan_chunk(chunk),
                };
                let (other, rows) = output?;
                stats.append(other);
                page.append_page(rows);
            }
            Ok(stats)
        })
    }

    fn scan_files(
        &self,
        search: &mut FileSearch,
        files: &[CollectedFile],
        page: &mut SearchPage,
        mut bytes: ScanBytes<'_>,
        mut context: Option<ContextScan<'_>>,
    ) -> io::Result<ScanStats> {
        let mut stats = ScanStats::default();
        let regex = search.regex().clone();
        for file in files {
            if let Err(error) = self.deadline.check() {
                stats.limit = Some(error.to_string());
                break;
            }
            let path = file.path();
            let mut input = match self.source.open(path, self.cancelled) {
                Ok(input) => input,
                Err(error) if is_cancellation(&error) => return Err(error),
                Err(_) => {
                    stats.skipped += 1;
                    stats.unverified += 1;
                    stats.record_unchecked(&file.relative, "could not be opened");
                    continue;
                }
            };
            let previous_count = page.matches.len();
            let previous_progress = page.progress;
            let matched = |number, line: &str| page.matched(&file.relative, number, line, &regex);
            let scan = match &mut bytes {
                ScanBytes::Local(left) => search.scan(&mut input, left, matched),
                ScanBytes::Shared(credit) => search.scan_reader(
                    SharedReader {
                        input: &mut input,
                        credit,
                        deadline: self.deadline,
                    },
                    matched,
                ),
            };
            if let Err(error) = scan.and_then(|()| self.source.verify(path, &input)) {
                if is_cancellation(&error) {
                    return Err(error);
                }
                page.progress = previous_progress;
                page.matches.truncate(previous_count);
                stats.skipped += 1;
                if is_io_limit(&error) {
                    stats.limit.get_or_insert_with(|| error.to_string());
                    if error
                        .get_ref()
                        .and_then(|cause| cause.downcast_ref::<IoLimit>())
                        .is_some_and(|limit| limit.0 == LINE_BYTES.0)
                    {
                        stats.record_unchecked(&file.relative, LINE_BYTES.0);
                    }
                    if bytes.exhausted() {
                        break;
                    }
                } else if error.kind() != io::ErrorKind::InvalidData {
                    stats.unverified += 1;
                    stats.record_unchecked(&file.relative, "could not be read or verified");
                }
                continue;
            }
            stats.searched += 1;
            if let (Some(context), ScanBytes::Local(bytes_left)) = (&mut context, &mut bytes) {
                let mut allowance = context.bytes_left.min(*bytes_left);
                let available = allowance;
                context.preview.extend(
                    &file.relative,
                    path,
                    self.source,
                    &mut input,
                    &mut context.reader,
                    &mut allowance,
                    &page.matches[previous_count..],
                    previous_count,
                    context.lines,
                )?;
                let consumed = available - allowance;
                context.bytes_left -= consumed;
                *bytes_left -= consumed;
            }
        }
        Ok(stats)
    }
}

struct ExtraWorkers {
    pool: &'static AtomicUsize,
    count: usize,
}

impl ExtraWorkers {
    fn acquire(files: usize) -> Option<Self> {
        let wanted = (files / FILES_PER_WORKER).saturating_sub(1);
        if wanted == 0 {
            return None;
        }
        static POOL: OnceLock<AtomicUsize> = OnceLock::new();
        let pool = POOL.get_or_init(|| {
            let cpus = thread::available_parallelism().map_or(1, usize::from);
            AtomicUsize::new(cpus.saturating_sub(2).min(MAX_EXTRA_THREADS))
        });
        let available = pool
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |available| {
                (available > 0).then_some(available - available.min(wanted))
            })
            .ok()?;
        Some(Self {
            pool,
            count: available.min(wanted),
        })
    }
}

impl Drop for ExtraWorkers {
    fn drop(&mut self) {
        self.pool.fetch_add(self.count, Ordering::Relaxed);
    }
}

struct SharedBytes {
    state: Mutex<ByteState>,
    returned: Condvar,
}

struct ByteState {
    available: u64,
    reserved: u64,
}

impl SharedBytes {
    fn new(available: u64) -> Self {
        Self {
            state: Mutex::new(ByteState {
                available,
                reserved: 0,
            }),
            returned: Condvar::new(),
        }
    }

    fn exhausted(&self) -> bool {
        let state = self.state.lock().expect("scan byte budget lock");
        state.available == 0 && state.reserved == 0
    }

    fn credit(&self) -> ByteCredit<'_> {
        ByteCredit {
            shared: self,
            reserved: 0,
            left: 0,
        }
    }

    fn reserve(&self) -> u64 {
        let state = self.state.lock().expect("scan byte budget lock");
        let mut state = self
            .returned
            .wait_while(state, |state| state.available == 0 && state.reserved != 0)
            .expect("scan byte budget lock");
        let reserved = state.available.min(SCAN_CREDIT_BYTES);
        state.available -= reserved;
        state.reserved += reserved;
        reserved
    }

    fn release(&self, reserved: u64, left: u64) {
        if reserved == 0 {
            return;
        }
        let mut state = self.state.lock().expect("scan byte budget lock");
        let may_have_waiters = state.available == 0;
        state.available += left;
        state.reserved -= reserved;
        if may_have_waiters && (state.available != 0 || state.reserved == 0) {
            self.returned.notify_all();
        }
    }
}

struct ByteCredit<'a> {
    shared: &'a SharedBytes,
    reserved: u64,
    left: u64,
}

impl ByteCredit<'_> {
    fn read(&mut self, input: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            self.shared.release(self.reserved, 0);
            self.reserved = self.shared.reserve();
            self.left = self.reserved;
        }
        if self.left == 0 {
            return match input.read(&mut buffer[..1])? {
                0 => Ok(0),
                _ => Err(io::Error::other(SCAN_BYTES)),
            };
        }
        let allowed = self.left.min(buffer.len() as u64) as usize;
        let count = input.read(&mut buffer[..allowed])?;
        self.left -= count as u64;
        Ok(count)
    }
}

impl Drop for ByteCredit<'_> {
    fn drop(&mut self) {
        self.shared.release(self.reserved, self.left);
    }
}

struct SharedReader<'a, 'b, R> {
    input: R,
    credit: &'a mut ByteCredit<'b>,
    deadline: ScanDeadline,
}

impl<R: Read> Read for SharedReader<'_, '_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.deadline.check()?;
        self.credit.read(&mut self.input, buffer)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        CollectedFile, FileSearch, ScanBytes, ScanDeadline, SearchPage, SearchView, WorkspaceFiles,
        WorkspaceScan,
    };

    #[test]
    fn parallel_scan_preserves_order_counts_and_preview() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap();
        let source = WorkspaceFiles::new(&path).unwrap();
        let deadline = ScanDeadline::default();
        let scan = WorkspaceScan {
            source: &source,
            deadline,
            cancelled: &|| false,
        };
        let mut files = Vec::new();
        for index in 0..12 {
            let relative = format!("{index:02}.txt");
            let content = if index == 1 {
                &b"hit\n\xff"[..]
            } else {
                &b"hit alpha\nother\nhit beta\n"[..]
            };
            if index != 3 {
                fs::write(path.join(&relative), content).unwrap();
            }
            files.push(CollectedFile {
                relative,
                original: None,
                modified: None,
            });
        }
        let search = FileSearch::new("hit", 1024, deadline).unwrap();
        for view in [SearchView::Lines, SearchView::Files, SearchView::Count] {
            let mut serial = SearchPage::new(0, 10, view, 70);
            let mut parallel = SearchPage::new(0, 10, view, 70);
            let expected = scan
                .scan_files(
                    &mut search.clone(),
                    &files,
                    &mut serial,
                    ScanBytes::Local(1024),
                    None,
                )
                .unwrap();
            let actual = scan
                .parallel(&search, &files, &mut parallel, 1024, 3)
                .unwrap();
            assert_eq!(
                (expected.searched, expected.skipped, expected.limit),
                (actual.searched, actual.skipped, actual.limit)
            );
            assert_eq!((expected.unverified, actual.unverified), (1, 1));
            assert_eq!(serial.progress.total, parallel.progress.total);
            assert_eq!(serial.progress.full, parallel.progress.full);
            assert_eq!(
                serde_json::to_value(serial.matches).unwrap(),
                serde_json::to_value(parallel.matches).unwrap()
            );
        }

        let mut limited = SearchPage::new(0, 10, SearchView::Lines, 80_000);
        let stats = scan.parallel(&search, &files, &mut limited, 30, 3).unwrap();
        assert_eq!(
            stats.limit.as_deref(),
            Some("search input byte budget exhausted")
        );
        assert!(stats.searched <= 1);
        assert!(limited.progress.total <= 2);
    }
}
