use tokio_stream::wrappers::ReceiverStream;

use super::chunk::ProviderChunk;
use super::error::transport_error;
use super::types::ProviderFailure;
use super::ProviderChunkStream;

pub(crate) fn stream_sse_events(
    response: reqwest::Response,
    mut on_frame: impl FnMut(&str) -> Vec<ProviderChunk> + Send + 'static,
) -> ProviderChunkStream {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<ProviderChunk, ProviderFailure>>(16);

    tokio::spawn(async move {
        run_sse_loop(response, &mut on_frame, &tx).await;
    });

    Box::pin(ReceiverStream::new(rx))
}

async fn run_sse_loop(
    response: reqwest::Response,
    on_frame: &mut (impl FnMut(&str) -> Vec<ProviderChunk> + Send),
    tx: &tokio::sync::mpsc::Sender<Result<ProviderChunk, ProviderFailure>>,
) {
    use tokio_stream::StreamExt;
    let mut byte_stream = response.bytes_stream();
    let mut buf = String::new();

    loop {
        match byte_stream.next().await {
            Some(Ok(bytes)) => {
                buf.push_str(&String::from_utf8_lossy(&bytes));
                while let Some(pos) = buf.find("\n\n") {
                    let frame = buf[..pos].to_string();
                    buf.drain(..pos + 2);
                    let trimmed = frame.trim().to_string();
                    if trimmed.is_empty() {
                        continue;
                    }
                    for chunk in on_frame(&trimmed) {
                        if tx.send(Ok(chunk)).await.is_err() {
                            return;
                        }
                    }
                }
            }
            Some(Err(e)) => {
                let _ = tx.send(Err(transport_error(&e))).await;
                return;
            }
            None => {
                let remaining = buf.trim().to_string();
                if !remaining.is_empty() {
                    for chunk in on_frame(&remaining) {
                        let _ = tx.send(Ok(chunk)).await;
                    }
                }
                for chunk in on_frame("") {
                    let _ = tx.send(Ok(chunk)).await;
                }
                return;
            }
        }
    }
}
