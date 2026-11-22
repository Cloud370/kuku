use std::borrow::Cow;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

pub use crate::execution::OutputStream;
use crate::execution::ReportedTests;

use super::encoding::display_text;
use super::reports::ReportCollector;
use super::summary::{Collector, Summary};

pub(crate) mod search;

const HEADER_BYTES: usize = 4096;
const MAGIC: &[u8; 8] = b"KUKUOUT1";
const FRAME_HEADER_BYTES: usize = 5;
pub(crate) const MAX_FRAME_BYTES: usize = 8192;
const MAX_READ_BYTES: usize = super::RUN_COMMAND_MAX_CHARS;
const DEFAULT_READ_BYTES: usize = 16 * 1024;
const MAX_READ_SEGMENTS: usize = 4096;
const MAX_SEARCH_BYTES: usize = 4096;
pub(crate) const MAX_OUTPUT_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_SUMMARY_LINES: usize = 6;
const MAX_SUMMARY_LINE_BYTES: usize = 192;
const MAX_PENDING_LINE_BYTES: usize = 1024;
pub(crate) const OUTPUT_DRAFT_SUFFIX: &str = ".output-draft";

#[derive(Debug, Clone, Copy)]
pub(crate) struct OutputLimits {
    pub max_output_bytes: u64,
    pub max_preview_bytes: usize,
}

impl Default for OutputLimits {
    fn default() -> Self {
        Self {
            max_output_bytes: 16 * 1024 * 1024,
            max_preview_bytes: MAX_READ_BYTES,
        }
    }
}

/// Selects a bounded byte window from retained command output.
#[derive(Debug, Clone)]
pub struct OutputRead {
    /// The starting byte offset across both captured streams.
    pub offset: u64,
    /// The maximum raw bytes to scan, between 1 and 80000.
    pub limit: usize,
    /// An optional literal filter of up to 4096 bytes that fits within the scan limit.
    pub search: Option<String>,
}

impl Default for OutputRead {
    fn default() -> Self {
        Self {
            offset: 0,
            limit: DEFAULT_READ_BYTES,
            search: None,
        }
    }
}

impl OutputStream {
    fn tag(self) -> u8 {
        match self {
            Self::Stdout => 1,
            Self::Stderr => 2,
        }
    }

    fn from_tag(tag: u8) -> io::Result<Self> {
        match tag {
            1 => Ok(Self::Stdout),
            2 => Ok(Self::Stderr),
            _ => Err(invalid_output("invalid output stream tag")),
        }
    }
}

/// Contains one retained stream segment in capture order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputSegment {
    /// The pipe that produced this segment.
    pub stream: OutputStream,
    /// The segment's starting byte offset across both captured streams.
    pub offset: u64,
    /// The number of retained raw bytes represented by this segment.
    pub bytes: usize,
    /// The readable command output with terminal color codes removed.
    pub text: String,
    /// Original bytes when the display text differs from the captured bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_bytes: Option<Vec<u8>>,
}

impl OutputSegment {
    /// Return the original bytes represented by this segment.
    pub fn as_bytes(&self) -> &[u8] {
        self.raw_bytes.as_deref().unwrap_or(self.text.as_bytes())
    }
}

/// Returns a bounded output view with continuation and capture status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputChunk {
    /// The Session-owned output reference selected by the read.
    pub output_id: String,
    /// The requested starting byte offset.
    pub offset: u64,
    /// The next byte offset to read, including any overlap required by literal search.
    pub next_offset: u64,
    /// The total retained payload bytes across both captured streams.
    pub total_bytes: u64,
    /// The selected stream segments in capture order.
    pub segments: Vec<OutputSegment>,
    /// A bounded set of selected build or test result lines.
    pub summary: Vec<String>,
    /// Whether the writer finalized this output record.
    pub complete: bool,
    /// Whether the storage quota caused output bytes to be discarded.
    pub capture_truncated: bool,
    /// Whether pipe cleanup reached its deadline before capture ended.
    pub cleanup_incomplete: bool,
    /// Whether literal search filtered the returned stream segments.
    pub filtered: bool,
    /// Whether decoding the returned segments replaced invalid or split UTF-8 bytes.
    pub lossy_utf8: bool,
    /// Whether more retained bytes remain after the continuation offset.
    pub has_more: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct CaptureMetadata {
    pub complete: bool,
    pub stored_bytes: u64,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub stdout_lines: u64,
    pub stderr_lines: u64,
    pub quota_truncated: bool,
    pub cleanup_incomplete: bool,
    pub summary: Vec<String>,
}

pub(crate) struct OutputWriter {
    file: tokio::fs::File,
    path: PathBuf,
    file_bytes: u64,
    quota: u64,
    metadata: CaptureMetadata,
    stdout: StreamSummary,
    stderr: StreamSummary,
    reports: Option<ReportCollector>,
    collector: Option<Collector>,
}

pub(super) struct FinishedCapture {
    pub(super) metadata: CaptureMetadata,
    pub(super) reports: Vec<ReportedTests>,
    pub(super) summary: Option<Summary>,
}

impl OutputWriter {
    pub(crate) async fn create(
        output_dir: &Path,
        output_id: &str,
        limits: OutputLimits,
    ) -> io::Result<Self> {
        let path = Self::prepare(output_dir, output_id, limits)?;
        let file = crate::util::fs_sync::write_options()
            .create_new(true)
            .open(&path)?;
        Self::initialize(file, path, limits).await
    }

    pub(crate) async fn create_staged(
        output_dir: &Path,
        output_id: &str,
        limits: OutputLimits,
    ) -> io::Result<(Self, OutputDraft)> {
        let destination = Self::prepare(output_dir, output_id, limits)?;
        let (file, path) = tempfile::Builder::new()
            .prefix(output_id)
            .rand_bytes(0)
            .suffix(OUTPUT_DRAFT_SUFFIX)
            .make_in(output_dir, |path| {
                crate::util::fs_sync::write_options()
                    .create_new(true)
                    .open(path)
            })?
            .into_parts();
        let writer = Self::initialize(file, path.to_path_buf(), limits).await?;
        Ok((writer, OutputDraft { path, destination }))
    }

    pub(super) async fn create_command(
        output_dir: &Path,
        output_id: &str,
        limits: OutputLimits,
        collector: Option<Collector>,
    ) -> io::Result<Self> {
        let mut writer = Self::create(output_dir, output_id, limits).await?;
        writer.reports = Some(ReportCollector::default());
        writer.collector = collector;
        Ok(writer)
    }

    pub(crate) async fn write(&mut self, stream: OutputStream, bytes: &[u8]) -> io::Result<()> {
        let summary = match stream {
            OutputStream::Stdout => &mut self.stdout,
            OutputStream::Stderr => &mut self.stderr,
        };
        summary.observe(bytes, &mut self.metadata.summary, &mut self.reports);
        let remaining = self.quota.saturating_sub(self.file_bytes);
        let kept = bytes
            .len()
            .min(remaining.saturating_sub(FRAME_HEADER_BYTES as u64) as usize);
        self.metadata.quota_truncated |= kept < bytes.len();
        if kept == 0 {
            return Ok(());
        }
        let mut header = [0; FRAME_HEADER_BYTES];
        header[0] = stream.tag();
        header[1..].copy_from_slice(&(kept as u32).to_le_bytes());
        self.file.write_all(&header).await?;
        self.file.write_all(&bytes[..kept]).await?;
        if let Some(collector) = &mut self.collector {
            collector.observe(stream, self.metadata.stored_bytes, &bytes[..kept]);
        }
        self.file_bytes += (FRAME_HEADER_BYTES + kept) as u64;
        self.metadata.stored_bytes += kept as u64;
        Ok(())
    }

    pub(crate) async fn finish(self, cleanup_incomplete: bool) -> io::Result<CaptureMetadata> {
        self.finish_command(cleanup_incomplete, None)
            .await
            .map(|capture| capture.metadata)
    }

    pub(crate) fn quota_truncated(&self) -> bool {
        self.metadata.quota_truncated
    }

    pub(crate) async fn close(self) {
        drop(self.file.into_std().await);
    }

    pub(super) async fn finish_command(
        mut self,
        cleanup_incomplete: bool,
        exit_code: Option<i32>,
    ) -> io::Result<FinishedCapture> {
        if !cleanup_incomplete {
            self.stdout
                .finish(&mut self.metadata.summary, &mut self.reports);
            self.stderr
                .finish(&mut self.metadata.summary, &mut self.reports);
        }
        self.metadata.stdout_bytes = self.stdout.bytes;
        self.metadata.stderr_bytes = self.stderr.bytes;
        self.metadata.stdout_lines = self.stdout.line_count();
        self.metadata.stderr_lines = self.stderr.line_count();
        self.metadata.complete = true;
        self.metadata.cleanup_incomplete = cleanup_incomplete;
        self.file.flush().await?;
        self.file.seek(SeekFrom::Start(0)).await?;
        self.file.write_all(&encode_header(&self.metadata)?).await?;
        self.file.flush().await?;
        let file = self.file.into_std().await;
        crate::util::fs_sync::sync_file(&file)?;
        crate::util::fs_sync::sync_parent(&self.path)?;
        let reports = self
            .reports
            .map(ReportCollector::finish)
            .unwrap_or_default();
        let summary = if cleanup_incomplete || self.metadata.quota_truncated {
            None
        } else {
            self.collector
                .zip(exit_code)
                .and_then(|(collector, code)| collector.summarize(code))
        };
        Ok(FinishedCapture {
            metadata: self.metadata,
            reports,
            summary,
        })
    }

    fn prepare(output_dir: &Path, output_id: &str, limits: OutputLimits) -> io::Result<PathBuf> {
        if !(HEADER_BYTES as u64..=MAX_OUTPUT_BYTES).contains(&limits.max_output_bytes) {
            return Err(invalid_output(
                "output quota must be between 4096 bytes and 1 GiB",
            ));
        }
        if limits.max_preview_bytes > MAX_READ_BYTES {
            return Err(invalid_output("output preview must be at most 80000 bytes"));
        }
        let path = output_path(output_dir, output_id)?;
        crate::util::fs_sync::create_dir_all(output_dir)?;
        Ok(path)
    }

    async fn initialize(file: fs::File, path: PathBuf, limits: OutputLimits) -> io::Result<Self> {
        let mut file = tokio::fs::File::from_std(file);
        let metadata = CaptureMetadata::default();
        file.write_all(&encode_header(&metadata)?).await?;
        file.flush().await?;
        Ok(Self {
            file,
            path,
            file_bytes: HEADER_BYTES as u64,
            quota: limits.max_output_bytes,
            metadata,
            stdout: StreamSummary::default(),
            stderr: StreamSummary::default(),
            reports: None,
            collector: None,
        })
    }
}

pub(crate) struct OutputDraft {
    path: tempfile::TempPath,
    destination: PathBuf,
}

impl OutputDraft {
    pub(crate) fn publish(self) -> io::Result<()> {
        crate::util::fs_sync::rename(&self.path, &self.destination)
    }

    pub(crate) fn discard(self) -> io::Result<()> {
        self.path.close()
    }
}

#[derive(Default)]
struct StreamSummary {
    bytes: u64,
    newlines: u64,
    last_newline: bool,
    pending: Vec<u8>,
    long_line: bool,
}

impl StreamSummary {
    fn observe(
        &mut self,
        bytes: &[u8],
        summary: &mut Vec<String>,
        reports: &mut Option<ReportCollector>,
    ) {
        self.bytes = self.bytes.saturating_add(bytes.len() as u64);
        for &byte in bytes {
            self.last_newline = byte == b'\n';
            if self.last_newline {
                self.newlines = self.newlines.saturating_add(1);
                self.finish(summary, reports);
            } else if self.pending.len() < MAX_PENDING_LINE_BYTES {
                self.pending.push(byte);
            } else {
                self.long_line = true;
            }
        }
    }

    fn finish(&mut self, summary: &mut Vec<String>, reports: &mut Option<ReportCollector>) {
        let (text, lossy) = display_text(&self.pending, true, !self.long_line);
        let text = text.trim();
        if !self.long_line && !lossy {
            if let Some(reports) = reports {
                reports.observe(text);
            }
        }
        if !self.long_line && is_summary_line(text) {
            let mut line = text
                .chars()
                .filter(|ch| !ch.is_control())
                .collect::<String>();
            truncate_utf8(&mut line, MAX_SUMMARY_LINE_BYTES);
            if summary.last() != Some(&line) {
                if summary.len() == MAX_SUMMARY_LINES {
                    summary.remove(0);
                }
                summary.push(line);
            }
        }
        self.pending.clear();
        self.long_line = false;
    }

    fn line_count(&self) -> u64 {
        self.newlines + u64::from(self.bytes > 0 && !self.last_newline)
    }
}

pub(crate) fn read_output(
    output_dir: &Path,
    output_id: &str,
    request: OutputRead,
) -> io::Result<OutputChunk> {
    validate_read(&request)?;
    read_open_output(open_output(output_dir, output_id)?, output_id, request)
}

pub(crate) fn open_output(output_dir: &Path, output_id: &str) -> io::Result<fs::File> {
    fs::File::open(output_path(output_dir, output_id)?)
}

pub(crate) fn capture_metadata(output_dir: &Path, output_id: &str) -> io::Result<CaptureMetadata> {
    decode_header(&mut open_output(output_dir, output_id)?)
}

pub(crate) fn framed_bytes(payload: u64) -> u64 {
    (HEADER_BYTES as u64)
        .saturating_add(payload)
        .saturating_add(payload.div_ceil(MAX_FRAME_BYTES as u64) * FRAME_HEADER_BYTES as u64)
}

pub(crate) fn read_open_output(
    mut file: fs::File,
    output_id: &str,
    request: OutputRead,
) -> io::Result<OutputChunk> {
    validate_read(&request)?;
    let metadata = decode_header(&mut file)?;
    let mut window = OutputWindow::default();
    let captured_bytes = scan_output_frames(
        &mut file,
        &metadata,
        |file, stream, offset, frame_bytes, payload| {
            window.read_frame(file, stream, offset, frame_bytes, payload, &request)
        },
    )?;
    render_chunk(
        output_id,
        request,
        metadata,
        window,
        captured_bytes,
        &mut file,
    )
}

pub(crate) fn read_complete_stdout_bytes(
    file: fs::File,
    expected_bytes: usize,
) -> io::Result<Vec<u8>> {
    let mut file = io::BufReader::new(file);
    let metadata = decode_header(&mut file)?;
    if !metadata.complete
        || metadata.quota_truncated
        || metadata.stored_bytes != expected_bytes as u64
    {
        return Err(invalid_output(
            "retained output is incomplete or has an unexpected size",
        ));
    }
    let mut bytes = vec![0; expected_bytes];
    let mut offset = 0;
    while offset < bytes.len() {
        let Some((OutputStream::Stdout, count)) = read_frame_header(&mut file, &metadata)? else {
            return Err(invalid_output("expected a retained stdout frame"));
        };
        let end = offset + count;
        let target = bytes
            .get_mut(offset..end)
            .ok_or_else(|| invalid_output("unexpected output frame extent"))?;
        file.read_exact(target)?;
        offset = end;
    }
    if file.read(&mut [0])? != 0 {
        return Err(invalid_output("unexpected data after retained output"));
    }
    Ok(bytes)
}

pub(super) struct OutputTail {
    pub(super) text: String,
    pub(super) lossy_utf8: bool,
}

pub(super) fn read_output_tails(
    output_dir: &Path,
    output_id: &str,
    stdout_limit: usize,
    stderr_limit: usize,
) -> io::Result<[OutputTail; 2]> {
    let mut file = open_output(output_dir, output_id)?;
    let metadata = decode_header(&mut file)?;
    if !metadata.complete || metadata.quota_truncated || metadata.cleanup_incomplete {
        return Err(invalid_output(
            "cannot read the end of an incomplete command capture",
        ));
    }
    let stream_bytes = [metadata.stdout_bytes, metadata.stderr_bytes];
    let limits = [stdout_limit, stderr_limit];
    let requests: [OutputRead; 2] = std::array::from_fn(|slot| {
        let limit = limits[slot];
        OutputRead {
            offset: stream_bytes[slot].saturating_sub(limit as u64),
            limit,
            search: None,
        }
    });
    let mut windows = [OutputWindow::default(), OutputWindow::default()];
    let mut offsets = [0, 0];
    scan_output_frames(
        &mut file,
        &metadata,
        |file, stream, _, frame_bytes, payload| {
            let slot = stream as usize;
            windows[slot].read_frame(
                file,
                stream,
                offsets[slot],
                frame_bytes,
                payload,
                &requests[slot],
            )?;
            offsets[slot] += frame_bytes as u64;
            Ok(())
        },
    )?;
    if offsets != stream_bytes {
        return Err(invalid_output(
            "output streams disagree with committed metadata",
        ));
    }
    let [stdout, stderr] = windows;
    Ok([
        stdout.render_tail(&mut file)?,
        stderr.render_tail(&mut file)?,
    ])
}

fn scan_output_frames(
    file: &mut fs::File,
    metadata: &CaptureMetadata,
    mut visit: impl FnMut(&mut fs::File, OutputStream, u64, usize, u64) -> io::Result<()>,
) -> io::Result<u64> {
    let mut captured_bytes = 0;
    let file_bytes = file.metadata()?.len();
    while file.stream_position()? < file_bytes {
        let Some((stream, frame_bytes)) = read_frame_header(file, metadata)? else {
            break;
        };
        let payload = file.stream_position()?;
        if payload + frame_bytes as u64 > file_bytes {
            if metadata.complete {
                return Err(invalid_output("output frame exceeds file length"));
            }
            break;
        }
        visit(file, stream, captured_bytes, frame_bytes, payload)?;
        captured_bytes += frame_bytes as u64;
    }
    if metadata.complete && captured_bytes != metadata.stored_bytes {
        return Err(invalid_output(
            "output length disagrees with committed metadata",
        ));
    }
    Ok(captured_bytes)
}

#[derive(Default)]
struct OutputWindow {
    bytes: Vec<u8>,
    spans: Vec<OutputSpan>,
    last_file_byte: [Option<u64>; 2],
    stream_ends: [u64; 2],
}

struct OutputSpan {
    stream: OutputStream,
    offset: u64,
    range: Range<usize>,
    preceding_file_byte: Option<u64>,
}

impl OutputSpan {
    fn starts_line(&self, file: &mut fs::File, bytes: &[u8]) -> io::Result<bool> {
        let Some(position) = self.preceding_file_byte else {
            return Ok(true);
        };
        if !cfg!(windows) || std::str::from_utf8(bytes).is_ok() {
            return Ok(false);
        }
        let mut previous = [0];
        file.seek(SeekFrom::Start(position))?;
        file.read_exact(&mut previous)?;
        Ok(previous[0] == b'\n')
    }
}

impl OutputWindow {
    fn render_tail(self, file: &mut fs::File) -> io::Result<OutputTail> {
        let starts_line = match self.spans.first() {
            Some(span) => span.starts_line(file, &self.bytes)?,
            None => true,
        };
        let (text, lossy_utf8) = display_text(&self.bytes, starts_line, true);
        Ok(OutputTail {
            text: text.into_owned(),
            lossy_utf8,
        })
    }

    fn read_frame(
        &mut self,
        file: &mut fs::File,
        stream: OutputStream,
        frame_offset: u64,
        frame_bytes: usize,
        payload: u64,
        request: &OutputRead,
    ) -> io::Result<()> {
        let skip = request
            .offset
            .saturating_sub(frame_offset)
            .min(frame_bytes as u64) as usize;
        let room = request.limit.saturating_sub(self.bytes.len());
        let keep = if self.spans.len() < MAX_READ_SEGMENTS {
            (frame_bytes - skip).min(room)
        } else {
            0
        };
        let slot = stream as usize;
        let preceding = self.last_file_byte[slot];
        self.last_file_byte[slot] = Some(payload + frame_bytes as u64 - 1);
        self.stream_ends[slot] = frame_offset + frame_bytes as u64;
        file.seek(SeekFrom::Current(skip as i64))?;
        if keep > 0 {
            let start = self.bytes.len();
            self.bytes.resize(start + keep, 0);
            file.read_exact(&mut self.bytes[start..])?;
            if let Some(previous) = self.spans.last_mut().filter(|span| span.stream == stream) {
                previous.range.end = start + keep;
            } else {
                self.spans.push(OutputSpan {
                    stream,
                    offset: frame_offset + skip as u64,
                    range: start..start + keep,
                    preceding_file_byte: if skip == 0 {
                        preceding
                    } else {
                        Some(payload + skip as u64 - 1)
                    },
                });
            }
        }
        file.seek(SeekFrom::Current((frame_bytes - skip - keep) as i64))?;
        Ok(())
    }
}

pub(crate) fn output_path(output_dir: &Path, output_id: &str) -> io::Result<PathBuf> {
    validate_output_id(output_id)?;
    Ok(output_dir.join(format!("{output_id}.output")))
}

pub(crate) fn validate_output_id(output_id: &str) -> io::Result<()> {
    if output_id.is_empty()
        || output_id.len() > 128
        || !output_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(invalid_output(
            "output id must contain 1-128 ASCII letters, digits, '-' or '_'",
        ));
    }
    Ok(())
}

fn encode_header(metadata: &CaptureMetadata) -> io::Result<[u8; HEADER_BYTES]> {
    let json = serde_json::to_vec(metadata).map_err(io::Error::other)?;
    if json.len() > HEADER_BYTES - 12 {
        return Err(invalid_output("output metadata exceeds fixed header"));
    }
    let mut header = [0; HEADER_BYTES];
    header[..8].copy_from_slice(MAGIC);
    header[8..12].copy_from_slice(&(json.len() as u32).to_le_bytes());
    header[12..12 + json.len()].copy_from_slice(&json);
    Ok(header)
}

fn decode_header(file: &mut impl Read) -> io::Result<CaptureMetadata> {
    let mut header = [0; HEADER_BYTES];
    file.read_exact(&mut header)?;
    if &header[..8] != MAGIC {
        return Err(invalid_output("unsupported output file header"));
    }
    let length = u32::from_le_bytes(header[8..12].try_into().unwrap()) as usize;
    if length > HEADER_BYTES - 12 {
        return Err(invalid_output("invalid output metadata length"));
    }
    serde_json::from_slice(&header[12..12 + length]).map_err(invalid_output)
}

fn read_frame_header(
    file: &mut impl Read,
    metadata: &CaptureMetadata,
) -> io::Result<Option<(OutputStream, usize)>> {
    let mut header = [0; FRAME_HEADER_BYTES];
    match file.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof && !metadata.complete => {
            return Ok(None)
        }
        Err(error) => return Err(error),
    }
    let stream = OutputStream::from_tag(header[0])?;
    let length = u32::from_le_bytes(header[1..].try_into().unwrap()) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(invalid_output("invalid output frame length"));
    }
    Ok(Some((stream, length)))
}

fn validate_read(request: &OutputRead) -> io::Result<()> {
    if request.limit == 0 || request.limit > MAX_READ_BYTES {
        return Err(invalid_output(
            "output read limit must be between 1 and 80000 bytes",
        ));
    }
    if let Some(search) = &request.search {
        if search.is_empty() || search.len() > MAX_SEARCH_BYTES || search.len() > request.limit {
            return Err(invalid_output(
                "literal search must contain 1-4096 bytes and fit the read limit",
            ));
        }
    }
    Ok(())
}

fn render_chunk(
    output_id: &str,
    request: OutputRead,
    metadata: CaptureMetadata,
    window: OutputWindow,
    total_bytes: u64,
    file: &mut fs::File,
) -> io::Result<OutputChunk> {
    let mut next_offset = request.offset.saturating_add(window.bytes.len() as u64);
    let has_more = next_offset < total_bytes;
    let ranges = match &request.search {
        Some(search) => {
            if (has_more && window.bytes.len() >= search.len()) || (!has_more && !metadata.complete)
            {
                next_offset -= window.bytes.len().min(search.len() - 1) as u64;
            }
            let matcher =
                regex::bytes::Regex::new(&regex::escape(search)).map_err(invalid_output)?;
            matcher
                .find_iter(&window.bytes)
                .map(|hit| hit.range())
                .collect::<Vec<_>>()
        }
        None => std::iter::once(0..window.bytes.len()).collect(),
    };
    let mut ranges = ranges.into_iter().peekable();
    let mut segments = Vec::new();
    let mut lossy_utf8 = false;
    let complete = metadata.complete && !metadata.quota_truncated && !metadata.cleanup_incomplete;
    for span in window.spans {
        while ranges
            .peek()
            .is_some_and(|range| range.end <= span.range.start)
        {
            ranges.next();
        }
        if ranges
            .peek()
            .is_some_and(|range| range.start < span.range.end)
        {
            let bytes = &window.bytes[span.range.clone()];
            let ends_stream = complete
                && span.offset + bytes.len() as u64 == window.stream_ends[span.stream as usize];
            let (text, lossy) = display_text(bytes, span.starts_line(file, bytes)?, ends_stream);
            let raw_bytes = matches!(text, Cow::Owned(_)).then(|| bytes.to_vec());
            lossy_utf8 |= lossy;
            segments.push(OutputSegment {
                stream: span.stream,
                offset: span.offset,
                bytes: bytes.len(),
                text: text.into_owned(),
                raw_bytes,
            });
        }
    }
    Ok(OutputChunk {
        output_id: output_id.to_string(),
        offset: request.offset,
        next_offset,
        total_bytes,
        segments,
        summary: metadata.summary,
        complete: metadata.complete,
        capture_truncated: metadata.quota_truncated,
        cleanup_incomplete: metadata.cleanup_incomplete,
        filtered: request.search.is_some(),
        lossy_utf8,
        has_more,
    })
}

fn is_summary_line(line: &str) -> bool {
    line.starts_with("test result:")
        || line.starts_with("Finished ")
        || line.starts_with("error:")
        || line.starts_with("error[")
        || line.starts_with("FAILED ")
        || line.starts_with("Tests:")
        || (line.contains(" passed") && (line.contains(" in ") || line.contains(" failed")))
}

fn truncate_utf8(text: &mut String, max_bytes: usize) {
    let mut keep = max_bytes.min(text.len());
    while !text.is_char_boundary(keep) {
        keep -= 1;
    }
    text.truncate(keep);
}

fn invalid_output(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests;
