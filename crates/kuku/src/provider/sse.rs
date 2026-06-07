use tokio_stream::wrappers::ReceiverStream;

use super::chunk::ProviderChunk;
use super::error::transport_error;
use super::types::ProviderFailure;
use super::ProviderChunkStream;

pub(crate) fn stream_sse_events(
    response: wreq::Response,
    mut on_frame: impl FnMut(&str) -> Result<Vec<ProviderChunk>, ProviderFailure> + Send + 'static,
    mut on_eof: impl FnMut() -> Result<Vec<ProviderChunk>, ProviderFailure> + Send + 'static,
) -> ProviderChunkStream {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<ProviderChunk, ProviderFailure>>(16);

    tokio::spawn(async move {
        run_sse_loop(response, &mut on_frame, &mut on_eof, &tx).await;
    });

    Box::pin(ReceiverStream::new(rx))
}

async fn run_sse_loop(
    response: wreq::Response,
    on_frame: &mut (impl FnMut(&str) -> Result<Vec<ProviderChunk>, ProviderFailure> + Send),
    on_eof: &mut (impl FnMut() -> Result<Vec<ProviderChunk>, ProviderFailure> + Send),
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
                    let chunks = match on_frame(&trimmed) {
                        Ok(chunks) => chunks,
                        Err(failure) => {
                            let _ = tx.send(Err(failure)).await;
                            return;
                        }
                    };
                    for chunk in chunks {
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
                    let chunks = match on_frame(&remaining) {
                        Ok(chunks) => chunks,
                        Err(failure) => {
                            let _ = tx.send(Err(failure)).await;
                            return;
                        }
                    };
                    for chunk in chunks {
                        let _ = tx.send(Ok(chunk)).await;
                    }
                }
                let chunks = match on_eof() {
                    Ok(chunks) => chunks,
                    Err(failure) => {
                        let _ = tx.send(Err(failure)).await;
                        return;
                    }
                };
                for chunk in chunks {
                    let _ = tx.send(Ok(chunk)).await;
                }
                return;
            }
        }
    }
}
