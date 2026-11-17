use std::fmt::Debug;

use tokio_stream::StreamExt;

pub async fn next_json_line<S, B, E>(
    stream: &mut S,
    buf: &mut Vec<u8>,
    context: &str,
) -> serde_json::Value
where
    S: tokio_stream::Stream<Item = Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
    E: Debug,
{
    loop {
        if let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line = String::from_utf8(buf.drain(..=pos).collect()).unwrap();
            return serde_json::from_str(line.trim()).unwrap_or_else(|error| {
                panic!("invalid JSON line {context}: {error}; line={line:?}")
            });
        }

        match stream.next().await {
            Some(Ok(chunk)) => buf.extend_from_slice(chunk.as_ref()),
            Some(Err(error)) => panic!("stream error {context}: {error:?}"),
            None => {
                let buffered = String::from_utf8_lossy(buf).into_owned();
                if buffered.is_empty() {
                    panic!("stream ended before receiving next JSON line {context}");
                }
                panic!("stream ended with partial buffered data {context}: {buffered:?}");
            }
        }
    }
}
