use std::collections::VecDeque;

use tokio::sync::broadcast;

use crate::event::StoredEvent;
use crate::query::UiEvent;
use crate::Result;

use super::Session;

/// A committed fact or a bounded transient update from the same runtime.
#[derive(Debug, Clone)]
pub enum Observation {
    /// A durable event whose identity is a reconnectable cursor.
    Committed(Box<StoredEvent>),
    /// A transient streaming update that does not advance the durable cursor.
    Live { agent: String, event: Box<UiEvent> },
    /// Slow consumption dropped transient updates while durable facts remain available.
    Lagged { skipped: u64 },
}

/// Cursor-based observer that never owns advancement or an unbounded event queue.
pub struct SessionObserver {
    session: Session,
    cursor: u64,
    page: VecDeque<StoredEvent>,
    live: broadcast::Receiver<(String, UiEvent)>,
}

impl SessionObserver {
    /// Return the last durable event delivered by this observer.
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Return one already available observation without waiting or advancing execution.
    pub fn try_next(&mut self) -> Result<Option<Observation>> {
        if self.page.is_empty() {
            self.page = self.session.events(self.cursor, 32)?.into();
        }
        if let Some(event) = self.page.pop_front() {
            self.cursor = event.id;
            return Ok(Some(Observation::Committed(Box::new(event))));
        }
        match self.live.try_recv() {
            Ok((agent, event)) => Ok(Some(Observation::Live {
                agent,
                event: Box::new(event),
            })),
            Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                Ok(Some(Observation::Lagged { skipped }))
            }
            Err(broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed) => {
                Ok(None)
            }
        }
    }

    /// Wait for committed facts or transient progress without driving execution.
    pub async fn next(&mut self) -> Result<Option<Observation>> {
        loop {
            if let Some(event) = self.page.pop_front() {
                self.cursor = event.id;
                return Ok(Some(Observation::Committed(Box::new(event))));
            }
            let changed = self
                .session
                .inner
                .writer
                .lock()
                .unwrap()
                .as_ref()
                .map(|writer| writer.changed());
            let changed =
                changed.unwrap_or_else(|| std::sync::Arc::new(tokio::sync::Notify::new()));
            let notified = changed.notified_owned();
            tokio::pin!(notified);
            notified.as_mut().enable();
            self.page = self.session.events(self.cursor, 32)?.into();
            if !self.page.is_empty() {
                continue;
            }
            if self.session.inner.state.lock().unwrap().closed {
                return Ok(None);
            }
            tokio::select! {
                _ = &mut notified => {}
                result = self.live.recv() => match result {
                    Ok((agent, event)) => return Ok(Some(Observation::Live { agent, event: Box::new(event) })),
                    Err(broadcast::error::RecvError::Lagged(skipped)) => return Ok(Some(Observation::Lagged { skipped })),
                    Err(broadcast::error::RecvError::Closed) => return Ok(None),
                }
            }
        }
    }

    pub(super) fn new(session: Session, cursor: u64) -> Self {
        let live = session.inner.live.subscribe();
        Self {
            session,
            cursor,
            page: VecDeque::new(),
            live,
        }
    }
}
