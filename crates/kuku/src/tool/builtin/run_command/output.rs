use std::borrow::Cow;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::execution::ReportedTests;

use super::encoding::display_text;
use super::reports::ReportCollector;

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

/// Identifies the command pipe that produced an output segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    /// The command's standard output pipe.
    Stdout,
    /// The command's standard error pipe.
    Stderr,
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
}

impl OutputWriter {
    pub(crate) async fn create(
        output_dir: &Path,
        output_id: &str,
        limits: OutputLimits,
    ) -> io::Result<Self> {
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
        let mut file = tokio::fs::File::from_std(
            crate::util::fs_sync::write_options()
                .create_new(true)
                .open(&path)?,
        );
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
        })
    }

    pub(super) async fn create_command(
        output_dir: &Path,
        output_id: &str,
        limits: OutputLimits,
    ) -> io::Result<Self> {
        let mut writer = Self::create(output_dir, output_id, limits).await?;
        writer.reports = Some(ReportCollector::default());
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
        self.file_bytes += (FRAME_HEADER_BYTES + kept) as u64;
        self.metadata.stored_bytes += kept as u64;
        Ok(())
    }

    pub(crate) async fn finish(self, cleanup_incomplete: bool) -> io::Result<CaptureMetadata> {
        self.finish_command(cleanup_incomplete)
            .await
            .map(|(metadata, _)| metadata)
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
    ) -> io::Result<(CaptureMetadata, Vec<ReportedTests>)> {
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
        Ok((self.metadata, reports))
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
    let mut logical_offset = 0;
    let file_bytes = file.metadata()?.len();
    while file.stream_position()? < file_bytes {
        let Some((stream, frame_bytes)) = read_frame_header(&mut file, &metadata)? else {
            break;
        };
        let payload = file.stream_position()?;
        if payload + frame_bytes as u64 > file_bytes {
            if metadata.complete {
                return Err(invalid_output("output frame exceeds file length"));
            }
            break;
        }
        window.read_frame(
            &mut file,
            stream,
            logical_offset,
            frame_bytes,
            payload,
            &request,
        )?;
        logical_offset += frame_bytes as u64;
    }
    if metadata.complete && logical_offset != metadata.stored_bytes {
        return Err(invalid_output(
            "output length disagrees with committed metadata",
        ));
    }
    render_chunk(
        output_id,
        request,
        metadata,
        window,
        logical_offset,
        &mut file,
    )
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

fn decode_header(file: &mut fs::File) -> io::Result<CaptureMetadata> {
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
    file: &mut fs::File,
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
    let ranges = match &request.search {
        Some(search) => {
            if next_offset < total_bytes && window.bytes.len() >= search.len() {
                next_offset -= (search.len() - 1) as u64;
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
        has_more: next_offset < total_bytes,
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
mod tests {
    use std::io::Write;

    use super::*;

    fn request(offset: u64, limit: usize) -> OutputRead {
        OutputRead {
            offset,
            limit,
            search: None,
        }
    }

    fn text(chunk: &OutputChunk) -> String {
        chunk
            .segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect()
    }

    #[tokio::test]
    async fn output_round_trip_preserves_stream_order_and_byte_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = OutputWriter::create(dir.path(), "call-1", OutputLimits::default())
            .await
            .unwrap();
        writer.write(OutputStream::Stdout, b"hello ").await.unwrap();
        writer.write(OutputStream::Stderr, b"world").await.unwrap();
        writer.write(OutputStream::Stdout, b"!\n").await.unwrap();
        writer.finish(false).await.unwrap();
        let chunk = read_output(dir.path(), "call-1", request(0, 100)).unwrap();
        assert_eq!("hello world!\n", text(&chunk));
        assert_eq!(
            vec![
                OutputStream::Stdout,
                OutputStream::Stderr,
                OutputStream::Stdout
            ],
            chunk
                .segments
                .iter()
                .map(|segment| segment.stream)
                .collect::<Vec<_>>()
        );
        assert_eq!(13, chunk.total_bytes);
        assert!(chunk.complete);
        assert!(!chunk.capture_truncated);
        let chunk = read_output(dir.path(), "call-1", request(6, 5)).unwrap();
        assert_eq!("world", text(&chunk));
        assert_eq!(11, chunk.next_offset);
        assert!(chunk.has_more);
        let last = read_output(dir.path(), "call-1", request(chunk.next_offset, 5)).unwrap();
        assert_eq!("!\n", text(&last));
        assert!(!last.has_more);
    }

    #[tokio::test]
    async fn output_quota_includes_headers_and_retains_observed_summary() {
        let dir = tempfile::tempdir().unwrap();
        let limits = OutputLimits {
            max_output_bytes: (HEADER_BYTES + FRAME_HEADER_BYTES + 7) as u64,
            max_preview_bytes: 100,
        };
        let mut writer = OutputWriter::create(dir.path(), "quota", limits)
            .await
            .unwrap();
        writer
            .write(OutputStream::Stdout, b"123456789\n")
            .await
            .unwrap();
        writer
            .write(
                OutputStream::Stderr,
                b"test result: ok. 3 passed; 0 failed\n",
            )
            .await
            .unwrap();
        let metadata = writer.finish(false).await.unwrap();
        assert_eq!(7, metadata.stored_bytes);
        assert_eq!(10, metadata.stdout_bytes);
        assert!(metadata.quota_truncated);
        assert_eq!(
            limits.max_output_bytes,
            fs::metadata(dir.path().join("quota.output")).unwrap().len()
        );
        let chunk = read_output(dir.path(), "quota", request(0, 100)).unwrap();
        assert_eq!("1234567", text(&chunk));
        assert!(chunk.capture_truncated);
        assert!(chunk.summary[0].contains("3 passed"));
    }

    #[tokio::test]
    async fn output_search_handles_frame_and_incremental_window_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = OutputWriter::create(dir.path(), "search", OutputLimits::default())
            .await
            .unwrap();
        writer
            .write(OutputStream::Stdout, b"hello wor")
            .await
            .unwrap();
        writer
            .write(OutputStream::Stdout, b"ld and goodbye")
            .await
            .unwrap();
        writer.finish(false).await.unwrap();
        let first = read_output(
            dir.path(),
            "search",
            OutputRead {
                offset: 0,
                limit: 8,
                search: Some("world".to_string()),
            },
        )
        .unwrap();
        assert!(first.segments.is_empty());
        assert!(first.filtered);
        assert_eq!(4, first.next_offset);
        let next = read_output(
            dir.path(),
            "search",
            OutputRead {
                offset: first.next_offset,
                limit: 8,
                search: Some("world".to_string()),
            },
        )
        .unwrap();
        assert!(text(&next).contains("world"));
        assert!(next.next_offset > first.next_offset);
        let absent = read_output(
            dir.path(),
            "search",
            OutputRead {
                offset: 0,
                limit: 100,
                search: Some("absent".to_string()),
            },
        )
        .unwrap();
        assert!(absent.segments.is_empty());
        assert!(!absent.has_more);
    }

    #[tokio::test]
    async fn output_preserves_raw_bytes_and_reports_lossy_slices() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = OutputWriter::create(dir.path(), "unicode", OutputLimits::default())
            .await
            .unwrap();
        let bytes = "你好".as_bytes();
        writer
            .write(OutputStream::Stdout, &bytes[..2])
            .await
            .unwrap();
        writer
            .write(OutputStream::Stdout, &bytes[2..])
            .await
            .unwrap();
        let invalid = b"\xff\0\r";
        writer.write(OutputStream::Stderr, invalid).await.unwrap();
        writer.finish(false).await.unwrap();
        let chunk = read_output(dir.path(), "unicode", request(0, bytes.len())).unwrap();
        assert_eq!("你好", text(&chunk));
        assert!(!chunk.lossy_utf8);
        assert!(chunk.segments[0].raw_bytes.is_none());
        let partial = read_output(dir.path(), "unicode", request(1, 2)).unwrap();
        assert!(partial.lossy_utf8);
        assert_eq!(3, partial.next_offset);
        assert_eq!(&bytes[1..3], partial.segments[0].as_bytes());
        let rest = read_output(dir.path(), "unicode", request(partial.next_offset, 100)).unwrap();
        assert_eq!(&bytes[3..], rest.segments[0].as_bytes());
        assert_eq!(invalid.as_slice(), rest.segments[1].as_bytes());
        let wire = serde_json::to_value(&rest).unwrap();
        assert!(wire["segments"][0].get("raw_bytes").is_none());
        assert_eq!(
            serde_json::json!([255, 0, 13]),
            wire["segments"][1]["raw_bytes"]
        );
    }

    #[tokio::test]
    async fn output_rejects_escape_ids_and_never_overwrites_existing_output() {
        let dir = tempfile::tempdir().unwrap();
        for id in ["", "../escape", "a/b", "a\\b", ".", "..", "💥"] {
            assert!(
                OutputWriter::create(dir.path(), id, OutputLimits::default())
                    .await
                    .is_err(),
                "unsafe id {id:?} was accepted"
            );
            assert!(read_output(dir.path(), id, request(0, 10)).is_err());
        }
        let writer = OutputWriter::create(dir.path(), "same", OutputLimits::default())
            .await
            .unwrap();
        writer.finish(false).await.unwrap();
        assert_eq!(
            io::ErrorKind::AlreadyExists,
            OutputWriter::create(dir.path(), "same", OutputLimits::default())
                .await
                .err()
                .unwrap()
                .kind()
        );
        assert!(
            read_output(dir.path(), "same", request(0, 100))
                .unwrap()
                .complete
        );
    }

    #[tokio::test]
    async fn output_detects_committed_corruption_and_bounds_read_requests() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = OutputWriter::create(dir.path(), "corrupt", OutputLimits::default())
            .await
            .unwrap();
        writer.write(OutputStream::Stdout, b"hello").await.unwrap();
        writer.finish(false).await.unwrap();
        assert!(read_output(dir.path(), "corrupt", request(0, 0)).is_err());
        assert!(read_output(dir.path(), "corrupt", request(0, MAX_READ_BYTES + 1)).is_err());
        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(dir.path().join("corrupt.output"))
            .unwrap();
        file.seek(SeekFrom::Start(HEADER_BYTES as u64 + 1)).unwrap();
        file.write_all(&u32::MAX.to_le_bytes()).unwrap();
        assert!(read_output(dir.path(), "corrupt", request(0, 10)).is_err());
    }

    #[tokio::test]
    async fn unfinished_capture_is_readable_and_marked_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = OutputWriter::create(dir.path(), "partial", OutputLimits::default())
            .await
            .unwrap();
        writer
            .write(OutputStream::Stdout, b"partial\n")
            .await
            .unwrap();
        writer.file.flush().await.unwrap();
        let chunk = read_output(dir.path(), "partial", request(0, 100)).unwrap();
        assert!(!chunk.complete);
        assert_eq!("partial\n", text(&chunk));
        drop(writer);
        fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join("partial.output"))
            .unwrap()
            .write_all(&[1, 10])
            .unwrap();
        assert_eq!(
            "partial\n",
            text(&read_output(dir.path(), "partial", request(0, 100)).unwrap())
        );
    }
}
