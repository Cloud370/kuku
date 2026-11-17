use std::path::Path;
use std::pin::Pin;
use std::time::Duration;

use bytes::Bytes;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_stream::StreamExt;
use wreq::Method;

use crate::http::checked;
use crate::{Client, Error, HistoryEntry, Target};

const READ_TIMEOUT: Duration = Duration::from_secs(45);
const RETRY_DELAYS_MS: [u64; 4] = [100, 250, 500, 1000];
const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// A typed observation frame, carrying either a durable fact or transient preview.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Observation {
    /// A committed Session event advancing the observation cursor.
    Committed {
        /// Stable durable event cursor.
        id: u64,
        /// Agent address associated with the fact, when known.
        agent: Option<String>,
        /// Committed event kind.
        kind: String,
        /// Optional displayable history entry for the selected target.
        entry: Option<Box<HistoryEntry>>,
        /// Whether the fact affects a client's Session or Agent state.
        #[serde(default)]
        state_changed: bool,
        /// Turn of the committed fact, when applicable.
        turn: Option<u64>,
        /// Provider request associated with the fact, when applicable.
        request_id: Option<String>,
        /// Operation code for a runtime-state fact.
        operation: Option<String>,
    },
    /// Transient text, tool progress or another runtime signal.
    Live {
        /// Agent that produced the preview.
        agent: String,
        /// Execution identity that emitted the preview.
        execution_id: u64,
        /// Last durable Session cursor seen at the preview's source.
        #[serde(default)]
        cursor: u64,
        /// Turn that emitted the preview, when available.
        turn: Option<u64>,
        /// The existing SDK progress event in wire form.
        event: Value,
    },
    /// Transient frames were dropped and should be refreshed from durable history.
    Lagged {
        /// Number of missed transient frames, or zero after a reconnect with an unknown count.
        skipped: u64,
    },
    /// An idle connection is still healthy.
    Heartbeat,
}

/// A bounded observation reader that resumes from its last delivered committed event.
pub struct Subscription {
    client: Client,
    target: Target,
    agent: Option<String>,
    cursor: u64,
    reader: Option<Reader>,
    reconnect: bool,
    failures: usize,
}

impl Subscription {
    /// Return the last delivered committed Session event.
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Return the target this subscription was originally bound to.
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// Read the next nonduplicate observation, reconnecting with bounded backoff when disconnected.
    pub async fn next(&mut self) -> Result<Observation, Error> {
        loop {
            if self.reader.is_none() {
                match self.connect().await {
                    Ok(reader) => self.reader = Some(reader),
                    Err(error) => {
                        self.pause(error).await?;
                        continue;
                    }
                }
                if std::mem::take(&mut self.reconnect) {
                    return Ok(Observation::Lagged { skipped: 0 });
                }
            }
            match self
                .reader
                .as_mut()
                .expect("reader was initialized")
                .next()
                .await
            {
                Ok(Observation::Heartbeat) => {
                    self.failures = 0;
                }
                Ok(Observation::Committed { id, .. }) if id <= self.cursor => {}
                Ok(Observation::Live { ref agent, .. })
                    if self.agent.as_ref().is_some_and(|target| target != agent) => {}
                Ok(frame) => {
                    self.failures = 0;
                    if let Observation::Committed { id, .. } = &frame {
                        self.cursor = *id;
                    }
                    return Ok(frame);
                }
                Err(error) => {
                    self.reader = None;
                    self.reconnect = true;
                    self.pause(error).await?;
                }
            }
        }
    }

    pub(crate) fn new(client: Client, target: &Target, agent: Option<&str>, after: u64) -> Self {
        Self {
            client,
            target: target.clone(),
            agent: agent.map(str::to_owned),
            cursor: after,
            reader: None,
            reconnect: false,
            failures: 0,
        }
    }

    async fn pause(&mut self, error: Error) -> Result<(), Error> {
        let Some(delay) = RETRY_DELAYS_MS
            .get(self.failures)
            .filter(|_| retryable(&error))
        else {
            return Err(error);
        };
        self.failures += 1;
        tokio::time::sleep(Duration::from_millis(*delay)).await;
        Ok(())
    }

    async fn connect(&mut self) -> Result<Reader, Error> {
        #[derive(Serialize)]
        struct Query<'a> {
            workspace: &'a Path,
            agent: Option<&'a str>,
            after: u64,
        }
        let response = self
            .client
            .request(
                Method::GET,
                ["sessions", &self.target.session_id, "observe"],
            )
            .query(&Query {
                workspace: &self.target.workspace,
                agent: self.agent.as_deref(),
                after: self.cursor,
            })
            .read_timeout(READ_TIMEOUT)
            .send()
            .await
            .map_err(Error::transport)?;
        let response = checked(response).await?;
        let kind = response
            .headers()
            .get(wreq::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok());
        if !kind.is_some_and(|kind| kind.split(';').next() == Some("application/x-ndjson")) {
            return Err(Error::Protocol(
                "observation stream must use application/x-ndjson".into(),
            ));
        }
        Ok(Reader {
            stream: Box::pin(response.bytes_stream()),
            remaining: Bytes::new(),
            line: Vec::new(),
        })
    }
}

type BodyStream = Pin<Box<dyn Stream<Item = wreq::Result<Bytes>> + Send>>;

struct Reader {
    stream: BodyStream,
    remaining: Bytes,
    line: Vec<u8>,
}

impl Reader {
    async fn next(&mut self) -> Result<Observation, Error> {
        loop {
            if let Some(newline) = self.remaining.iter().position(|byte| *byte == b'\n') {
                let fragment = self.remaining.split_to(newline + 1);
                self.extend(&fragment[..newline])?;
                let parsed = if self.line.iter().all(u8::is_ascii_whitespace) {
                    None
                } else {
                    Some(serde_json::from_slice(&self.line))
                };
                self.line.clear();
                if let Some(parsed) = parsed {
                    return parsed.map_err(|error| Error::Protocol(error.to_string()));
                }
            } else {
                let fragment = std::mem::take(&mut self.remaining);
                self.extend(&fragment)?;
                self.remaining = self
                    .stream
                    .next()
                    .await
                    .transpose()
                    .map_err(Error::transport)?
                    .ok_or(Error::ObservationEnded)?;
            }
        }
    }

    fn extend(&mut self, fragment: &[u8]) -> Result<(), Error> {
        if self.line.len() + fragment.len() > MAX_LINE_BYTES {
            return Err(Error::Protocol(
                "observation frame exceeds the supported size".into(),
            ));
        }
        self.line.extend_from_slice(fragment);
        Ok(())
    }
}

fn retryable(error: &Error) -> bool {
    matches!(
        error,
        Error::Transport(_)
            | Error::ObservationEnded
            | Error::HttpStatus(502..=504)
            | Error::Service {
                status: 502..=504,
                ..
            }
    )
}
