use tokio_stream::wrappers::ReceiverStream;

use super::ProviderChunkStream;
use super::chunk::ProviderChunk;
use super::error::{
    IMAGE_REQUEST_ERROR_DETAILS_OMITTED, omit_image_request_error_details, transport_error,
};
use super::trace::ProviderTrace;
use super::transport::TransportResponse;
use super::types::{ProviderFailure, ProviderFailureKind};

const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;

pub(crate) struct SseFrame {
    pub(crate) event: Option<String>,
    pub(crate) data: String,
}

pub(crate) fn parse_sse_frame(frame: &str) -> Option<SseFrame> {
    let mut event = None;
    let mut data = Vec::new();
    for line in frame.split(['\r', '\n']) {
        if let Some(value) = line.strip_prefix("event:") {
            event = Some(value.trim_start_matches(' ').to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.strip_prefix(' ').unwrap_or(value));
        } else if line == "data" {
            data.push("");
        }
    }
    if data.is_empty() {
        None
    } else {
        Some(SseFrame {
            event,
            data: data.join("\n"),
        })
    }
}

pub(crate) fn stream_sse_events(
    transport: TransportResponse,
    mut on_frame: impl FnMut(&str) -> Result<Vec<ProviderChunk>, ProviderFailure> + Send + 'static,
    mut on_eof: impl FnMut() -> Result<Vec<ProviderChunk>, ProviderFailure> + Send + 'static,
) -> ProviderChunkStream {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<ProviderChunk, ProviderFailure>>(16);
    tokio::spawn(async move {
        let TransportResponse {
            response,
            trace,
            contains_images,
        } = transport;
        let outcome = run_sse_loop(
            response,
            trace.as_ref(),
            contains_images,
            &mut on_frame,
            &mut on_eof,
            &tx,
        )
        .await;
        if let Some(trace) = &trace {
            if let Err(failure) = &outcome {
                trace.failure(failure.kind, None);
            } else {
                trace.flush();
            }
        }
        if let Err(failure) = outcome {
            let _ = tx.send(Err(failure)).await;
        }
    });
    Box::pin(ReceiverStream::new(rx))
}

async fn run_sse_loop(
    response: wreq::Response,
    trace: Option<&ProviderTrace>,
    contains_images: bool,
    on_frame: &mut (impl FnMut(&str) -> Result<Vec<ProviderChunk>, ProviderFailure> + Send),
    on_eof: &mut (impl FnMut() -> Result<Vec<ProviderChunk>, ProviderFailure> + Send),
    tx: &tokio::sync::mpsc::Sender<Result<ProviderChunk, ProviderFailure>>,
) -> Result<(), ProviderFailure> {
    use tokio_stream::StreamExt;
    let mut byte_stream = response.bytes_stream();
    let mut decoder = SseDecoder::default();
    loop {
        let next = tokio::select! {
            _ = tx.closed() => return Ok(()),
            next = byte_stream.next() => next,
        };
        let Some(bytes) = next else { break };
        for frame in decoder.feed(&bytes.map_err(|error| transport_error(&error))?)? {
            if !emit_frame(&frame, trace, contains_images, on_frame, tx).await? {
                return Ok(());
            }
        }
    }
    if let Some(frame) = decoder.finish()? {
        if !emit_frame(&frame, trace, contains_images, on_frame, tx).await? {
            return Ok(());
        }
    }
    let mut chunks = on_eof();
    if contains_images {
        redact_stream_errors(&mut chunks);
    }
    for chunk in chunks? {
        if tx.send(Ok(chunk)).await.is_err() {
            break;
        }
    }
    Ok(())
}

async fn emit_frame(
    frame: &str,
    trace: Option<&ProviderTrace>,
    contains_images: bool,
    on_frame: &mut (impl FnMut(&str) -> Result<Vec<ProviderChunk>, ProviderFailure> + Send),
    tx: &tokio::sync::mpsc::Sender<Result<ProviderChunk, ProviderFailure>>,
) -> Result<bool, ProviderFailure> {
    if frame.trim().is_empty() {
        return Ok(true);
    }
    // Keep the wire frame in the independent debug log, not in Session errors.
    if let Some(trace) = trace {
        trace.frame(frame);
    }
    let mut chunks = on_frame(frame);
    if contains_images {
        redact_stream_errors(&mut chunks);
    }
    for chunk in chunks? {
        if tx.send(Ok(chunk)).await.is_err() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn redact_stream_errors(chunks: &mut Result<Vec<ProviderChunk>, ProviderFailure>) {
    match chunks {
        Ok(chunks) => {
            for chunk in chunks {
                if let ProviderChunk::ServerError { message, .. } = chunk {
                    *message = IMAGE_REQUEST_ERROR_DETAILS_OMITTED.into();
                }
            }
        }
        Err(failure) => omit_image_request_error_details(failure),
    }
}

#[derive(Default)]
pub(super) struct SseDecoder {
    buffer: Vec<u8>,
    scan: usize,
    line_start: usize,
}

impl SseDecoder {
    pub(super) fn feed(&mut self, bytes: &[u8]) -> Result<Vec<String>, ProviderFailure> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();
        let mut consumed = 0;
        while let Some((end, next)) = self.next_boundary() {
            if end - consumed > MAX_FRAME_BYTES {
                return Err(invalid_frame("provider SSE frame exceeds the byte limit"));
            }
            let frame = std::str::from_utf8(&self.buffer[consumed..end])
                .map_err(|_| invalid_frame("provider SSE frame contains invalid UTF-8"))?
                .trim_end()
                .to_string();
            frames.push(frame);
            consumed = next;
        }
        if self.buffer.len() - consumed > MAX_FRAME_BYTES {
            return Err(invalid_frame("provider SSE frame exceeds the byte limit"));
        }
        if consumed > 0 {
            self.buffer.drain(..consumed);
            self.scan -= consumed;
            self.line_start -= consumed;
        }
        Ok(frames)
    }

    fn next_boundary(&mut self) -> Option<(usize, usize)> {
        while self.scan < self.buffer.len() {
            let width = match self.buffer[self.scan] {
                b'\n' => 1,
                b'\r' if self.scan + 1 == self.buffer.len() => return None,
                b'\r' if self.buffer[self.scan + 1] == b'\n' => 2,
                b'\r' => 1,
                _ => {
                    self.scan += 1;
                    continue;
                }
            };
            let end = self.scan;
            let empty = self.line_start == end;
            self.scan += width;
            self.line_start = self.scan;
            if empty {
                return Some((end, self.scan));
            }
        }
        None
    }

    pub(super) fn finish(&mut self) -> Result<Option<String>, ProviderFailure> {
        if self.buffer.is_empty() {
            return Ok(None);
        }
        let frame = std::str::from_utf8(&self.buffer)
            .map_err(|_| invalid_frame("provider SSE stream ends with invalid UTF-8"))?
            .trim()
            .to_string();
        self.buffer.clear();
        Ok((!frame.is_empty()).then_some(frame))
    }
}

fn invalid_frame(message: &str) -> ProviderFailure {
    ProviderFailure {
        kind: ProviderFailureKind::InvalidResponse,
        message: message.into(),
        status: None,
        provider_request_id: None,
        retry_after: None,
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_boundaries_preserve_utf8_and_crlf_frames() {
        let source = "event: text\r\ndata: {\"text\":\"你好😀\"}\r\n\r\ndata: [DONE]\n\n";
        for width in 1..=source.len() {
            let mut decoder = SseDecoder::default();
            let mut frames = Vec::new();
            for bytes in source.as_bytes().chunks(width) {
                frames.extend(decoder.feed(bytes).unwrap());
            }
            if let Some(frame) = decoder.finish().unwrap() {
                frames.push(frame);
            }
            assert_eq!(2, frames.len(), "network chunk width {width}");
            assert_eq!(
                "{\"text\":\"你好😀\"}",
                parse_sse_frame(&frames[0]).unwrap().data
            );
            assert_eq!("[DONE]", parse_sse_frame(&frames[1]).unwrap().data);
        }
    }

    #[test]
    fn comments_and_multiline_data_follow_sse_field_rules() {
        let frame =
            parse_sse_frame(": keepalive\nevent: update\ndata: {\ndata: \"ok\":true}\nid: 1")
                .unwrap();
        assert_eq!(Some("update"), frame.event.as_deref());
        assert_eq!("{\n\"ok\":true}", frame.data);
        assert!(parse_sse_frame(": ping\nretry: 1000").is_none());
    }

    #[test]
    fn oversized_and_invalid_utf8_frames_fail_with_bounded_diagnostics() {
        let mut decoder = SseDecoder::default();
        let failure = decoder.feed(&vec![b'x'; MAX_FRAME_BYTES + 1]).unwrap_err();
        assert!(!failure.retryable);
        let mut decoder = SseDecoder::default();
        assert!(decoder.feed(b"data: \xff\n\n").is_err());
    }

    #[tokio::test]
    async fn dropping_a_stream_closes_a_stalled_http_response() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut headers = Vec::new();
            let mut buffer = [0; 1024];
            while !headers.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                headers.extend_from_slice(&buffer[..count]);
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
            assert_eq!(0, socket.read(&mut buffer).await.unwrap());
        });
        let response =
            super::super::http_client::api_client(&crate::config::ProviderProxy::Environment)
                .unwrap()
                .get(format!("http://{address}/stall"))
                .send()
                .await
                .unwrap();
        let stream = stream_sse_events(
            super::TransportResponse {
                response,
                trace: None,
                contains_images: false,
            },
            |_| Ok(Vec::new()),
            || Ok(Vec::new()),
        );
        drop(stream);
        tokio::time::timeout(std::time::Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    }
}
