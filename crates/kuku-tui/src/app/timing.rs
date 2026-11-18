use super::Pane;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TurnTiming {
    pub(crate) turn: u64,
    pub(crate) started_at_ms: u64,
    pub(crate) elapsed_ms: Option<u64>,
}

impl Pane {
    pub(crate) fn turn_timing(&self) -> Option<TurnTiming> {
        let reported = self.snapshot.as_ref().and_then(|snapshot| {
            let agent = &snapshot.agent;
            Some((
                snapshot.cursor,
                TurnTiming {
                    turn: agent.turn?,
                    started_at_ms: agent.turn_started_at_ms?,
                    elapsed_ms: agent.turn_elapsed_ms,
                },
            ))
        });
        match (reported, self.observed_timing) {
            (Some((cursor, snapshot)), Some((event, live)))
                if snapshot.turn > live.turn || snapshot.turn == live.turn && cursor >= event =>
            {
                Some(snapshot)
            }
            (snapshot, live) => live.or(snapshot).map(|(_, timing)| timing),
        }
    }

    pub(super) fn observe_timing(
        &mut self,
        id: u64,
        turn: Option<u64>,
        started_at_ms: Option<u64>,
        elapsed_ms: Option<u64>,
    ) {
        let Some(turn) = turn.filter(|_| started_at_ms.is_some() || elapsed_ms.is_some()) else {
            return;
        };
        let current = self.turn_timing();
        if current.is_some_and(|current| current.turn > turn) {
            return;
        }
        let current = current.filter(|current| current.turn == turn);
        let Some(started_at_ms) = started_at_ms.or(current.map(|current| current.started_at_ms))
        else {
            return;
        };
        self.observed_timing = Some((
            id,
            TurnTiming {
                turn,
                started_at_ms,
                elapsed_ms: elapsed_ms.or(current.and_then(|current| current.elapsed_ms)),
            },
        ));
    }
}
