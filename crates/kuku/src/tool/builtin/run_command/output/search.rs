use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufReader, Read};

use serde::Serialize;
use serde_json::json;

use crate::tool::ToolDefinition;

use super::{
    decode_header, invalid_output, read_frame_header, CaptureMetadata, OutputStream,
    FRAME_HEADER_BYTES, HEADER_BYTES, MAX_FRAME_BYTES, MAX_SEARCH_BYTES,
};

pub(crate) const DEFAULT_MATCHES: usize = 32;
pub(crate) const MAX_MATCHES: usize = 128;
pub(crate) const MAX_LITERAL_BYTES: usize = MAX_SEARCH_BYTES;
pub(crate) const MAX_SCAN_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) fn definition(read: &ToolDefinition) -> ToolDefinition {
    let properties = &read.input_schema["properties"];
    ToolDefinition {
        name: "search_output".into(),
        description: "Find literal text in retained output by ID or name. Returns byte ranges you can read separately. Continue at next_offset when has_more is true, even on an empty page. A running command can add more output later.".into(),
        input_schema: json!({
            "type":"object",
            "properties":{
                "id":properties["id"],
                "name":properties["name"],
                "text":{"type":"string","minLength":1,"maxLength":MAX_LITERAL_BYTES},
                "stream":{"type":"string","enum":["stdout","stderr"]},
                "offset":properties["offset"],
                "limit":{"type":"integer","minimum":1,"maximum":MAX_MATCHES,"default":DEFAULT_MATCHES}
            },
            "required":["text"],
            "oneOf":read.input_schema["oneOf"],
            "additionalProperties":false
        }),
        ..read.clone()
    }
}

pub(crate) struct OutputSearch<'a> {
    pub(crate) text: &'a str,
    pub(crate) stream: Option<OutputStream>,
    pub(crate) offset: u64,
    pub(crate) limit: usize,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct OutputMatch {
    stream: OutputStream,
    offset: u64,
    end_offset: u64,
}

#[derive(Serialize)]
pub(crate) struct SearchPage {
    pub(crate) output_id: String,
    matches: Vec<OutputMatch>,
    offset: u64,
    next_offset: u64,
    total_bytes: Option<u64>,
    has_more: bool,
    capture_complete: bool,
    capture_truncated: bool,
    cleanup_incomplete: bool,
}

pub(crate) fn search_open_output(
    file: File,
    output_id: String,
    request: OutputSearch<'_>,
    cancelled: impl Fn() -> bool,
) -> io::Result<SearchPage> {
    let needle = request.text.as_bytes();
    if !(1..=MAX_LITERAL_BYTES).contains(&needle.len())
        || !(1..=MAX_MATCHES).contains(&request.limit)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid output search bounds",
        ));
    }
    check_cancelled(&cancelled)?;
    let file_len = file.metadata()?.len();
    let mut file = BufReader::with_capacity(64 * 1024, file);
    let metadata = decode_header(&mut file)?;
    let mut search = Search::new(needle, &request);
    let mut file_offset = HEADER_BYTES as u64;
    let mut offset = 0;
    let scan_end = request.offset.saturating_add(MAX_SCAN_BYTES);
    let mut buffer = [0; MAX_FRAME_BYTES];
    while file_offset < file_len {
        check_cancelled(&cancelled)?;
        let Some((stream, bytes)) = read_frame_header(&mut file, &metadata)? else {
            break;
        };
        file_offset += FRAME_HEADER_BYTES as u64 + bytes as u64;
        if file_offset > file_len {
            if metadata.complete {
                return Err(invalid_output("output frame exceeds file length"));
            }
            break;
        }
        let frame_end = offset + bytes as u64;
        let scanned = (scan_end.min(frame_end) - offset) as usize;
        if request.stream.is_none_or(|selected| selected == stream) {
            let before_cursor = (request.offset.saturating_sub(offset).min(bytes as u64)) as usize;
            let skip = before_cursor.saturating_sub(needle.len() - 1);
            if skip > 0 {
                search.reset(stream);
                file.seek_relative(skip as i64)?;
            }
            let data = &mut buffer[..scanned - skip];
            file.read_exact(data)?;
            if let Some(end) = search.scan(stream, offset + skip as u64, data) {
                if end < frame_end || file_offset < file_len {
                    return Ok(search.page(output_id, end, true, &metadata));
                }
            }
        } else {
            file.seek_relative(scanned as i64)?;
        }
        if scanned < bytes || (frame_end == scan_end && file_offset < file_len) {
            return Ok(search.page(output_id, scan_end, true, &metadata));
        }
        offset = frame_end;
    }
    if metadata.complete && offset != metadata.stored_bytes {
        return Err(invalid_output(
            "output length disagrees with committed metadata",
        ));
    }
    Ok(search.page(output_id, offset.max(request.offset), false, &metadata))
}

fn check_cancelled(cancelled: impl Fn() -> bool) -> io::Result<()> {
    if cancelled() {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "output search cancelled",
        ))
    } else {
        Ok(())
    }
}

struct Search<'a> {
    needle: &'a [u8],
    fallback: Vec<usize>,
    prefixes: [VecDeque<u64>; 2],
    offset: u64,
    limit: usize,
    matches: Vec<OutputMatch>,
}

impl<'a> Search<'a> {
    fn new(needle: &'a [u8], request: &OutputSearch<'_>) -> Self {
        let mut fallback = vec![0; needle.len()];
        let mut length = 0;
        for index in 1..needle.len() {
            while length > 0 && needle[index] != needle[length] {
                length = fallback[length - 1];
            }
            if needle[index] == needle[length] {
                length += 1;
            }
            fallback[index] = length;
        }
        Self {
            needle,
            fallback,
            prefixes: std::array::from_fn(|_| VecDeque::with_capacity(needle.len())),
            offset: request.offset,
            limit: request.limit,
            matches: Vec::with_capacity(request.limit),
        }
    }

    fn reset(&mut self, stream: OutputStream) {
        self.prefixes[stream_index(stream)].clear();
    }

    fn scan(&mut self, stream: OutputStream, offset: u64, bytes: &[u8]) -> Option<u64> {
        let prefix = &mut self.prefixes[stream_index(stream)];
        for (index, byte) in bytes.iter().copied().enumerate() {
            let position = offset + index as u64;
            let mut length = prefix.len();
            while length > 0 && self.needle[length] != byte {
                length = self.fallback[length - 1];
            }
            if length < prefix.len() {
                prefix.drain(..prefix.len() - length);
            }
            if self.needle[length] != byte {
                continue;
            }
            prefix.push_back(position);
            if prefix.len() == self.needle.len() {
                let end_offset = position + 1;
                if end_offset > self.offset {
                    self.matches.push(OutputMatch {
                        stream,
                        offset: prefix[0],
                        end_offset,
                    });
                    if self.matches.len() == self.limit {
                        return Some(end_offset);
                    }
                }
                prefix.drain(..prefix.len() - self.fallback[prefix.len() - 1]);
            }
        }
        None
    }

    fn page(
        self,
        output_id: String,
        next_offset: u64,
        has_more: bool,
        metadata: &CaptureMetadata,
    ) -> SearchPage {
        SearchPage {
            output_id,
            matches: self.matches,
            offset: self.offset,
            next_offset,
            total_bytes: metadata.complete.then_some(metadata.stored_bytes),
            has_more,
            capture_complete: metadata.complete
                && !metadata.quota_truncated
                && !metadata.cleanup_incomplete,
            capture_truncated: metadata.quota_truncated,
            cleanup_incomplete: metadata.cleanup_incomplete,
        }
    }
}

fn stream_index(stream: OutputStream) -> usize {
    match stream {
        OutputStream::Stdout => 0,
        OutputStream::Stderr => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::{search_open_output, OutputMatch, OutputSearch, OutputStream, MAX_SCAN_BYTES};
    use crate::tool::builtin::run_command::output::{open_output, OutputLimits, OutputWriter};

    #[tokio::test]
    async fn searches_one_stream_across_interleaving_and_resumes_in_capture_order() {
        use OutputStream::{Stderr, Stdout};

        let dir = tempfile::tempdir().unwrap();
        let mut writer = OutputWriter::create(dir.path(), "search", OutputLimits::default())
            .await
            .unwrap();
        for (stream, bytes) in [
            (Stdout, &b"ne"[..]),
            (Stderr, &b"needle\n"[..]),
            (Stdout, &b"edle nee"[..]),
            (Stderr, &b"dle\n"[..]),
        ] {
            writer.write(stream, bytes).await.unwrap();
        }
        writer.finish(false).await.unwrap();
        let search = |offset, stream| {
            search_open_output(
                open_output(dir.path(), "search").unwrap(),
                "search".into(),
                OutputSearch {
                    text: "needle",
                    offset,
                    stream,
                    limit: 1,
                },
                || false,
            )
            .unwrap()
        };
        let first = search(0, None);
        assert_eq!(
            first.matches,
            [OutputMatch {
                stream: Stderr,
                offset: 2,
                end_offset: 8
            }]
        );
        assert!(first.has_more && first.capture_complete);
        let second = search(first.next_offset, None);
        assert_eq!(
            second.matches,
            [OutputMatch {
                stream: Stdout,
                offset: 0,
                end_offset: 13
            }]
        );
        let third = search(second.next_offset, None);
        assert!(third.matches.is_empty() && !third.has_more);
        assert_eq!(third.total_bytes, Some(21));
        assert_eq!(search(0, Some(Stdout)).matches, second.matches);
    }

    #[tokio::test]
    async fn scan_budget_stops_an_empty_page_and_continues_across_that_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = OutputWriter::create(
            dir.path(),
            "pages",
            OutputLimits {
                max_output_bytes: MAX_SCAN_BYTES + 1024 * 1024,
                ..OutputLimits::default()
            },
        )
        .await
        .unwrap();
        let filler = [b'x'; 8192];
        for _ in 0..MAX_SCAN_BYTES / filler.len() as u64 - 1 {
            writer.write(OutputStream::Stdout, &filler).await.unwrap();
        }
        writer
            .write(OutputStream::Stdout, &filler[..filler.len() - 2])
            .await
            .unwrap();
        writer.write(OutputStream::Stdout, b"seek").await.unwrap();
        writer.finish(true).await.unwrap();
        let open = || open_output(dir.path(), "pages").unwrap();
        let request = |offset| OutputSearch {
            text: "seek",
            offset,
            stream: None,
            limit: 8,
        };
        let first = search_open_output(open(), "pages".into(), request(0), || false).unwrap();
        assert!(first.matches.is_empty() && first.has_more);
        assert_eq!(first.next_offset, MAX_SCAN_BYTES);
        let second =
            search_open_output(open(), "pages".into(), request(first.next_offset), || false)
                .unwrap();
        assert_eq!(
            second.matches,
            [OutputMatch {
                stream: OutputStream::Stdout,
                offset: MAX_SCAN_BYTES - 2,
                end_offset: MAX_SCAN_BYTES + 2
            }]
        );
        assert!(!second.has_more && !second.capture_complete && second.cleanup_incomplete);
        let checks = std::cell::Cell::new(0);
        let cancelled = search_open_output(open(), "pages".into(), request(0), || {
            checks.set(checks.get() + 1);
            checks.get() > 4
        });
        assert_eq!(
            cancelled.err().unwrap().kind(),
            std::io::ErrorKind::Interrupted
        );
    }
}
