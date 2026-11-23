use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::app::{Address, App, TurnTiming};

use super::{ActivityFrame, ObservedActivity};

pub(super) const FRAME_INTERVAL: Duration = Duration::from_millis(140);
const PHASE_CONFIRMATION: Duration = Duration::from_millis(96);

#[derive(Default)]
pub(crate) struct ActivityClock {
    target: Option<Address>,
    phase: PhaseClock,
    animation_started: Option<Instant>,
    elapsed: ElapsedClock,
    thinking: super::motion::ThinkingFlow,
}

impl ActivityClock {
    pub(crate) fn confirming_phase(&self) -> bool {
        self.phase.next.is_some()
    }

    pub(crate) fn observe(&mut self, app: &App, now: Instant) {
        self.update_phase(app, now);
    }

    pub(crate) fn sample(&mut self, app: &App, now: Instant, wall: SystemTime) -> ActivityFrame {
        let (observed, shown) = self.update_phase(app, now);
        let motion = observed.motion().is_some();
        if motion {
            self.animation_started.get_or_insert(now);
        } else {
            self.animation_started = None;
        }
        let step = self
            .animation_started
            .map(|started| now.duration_since(started).as_millis() / FRAME_INTERVAL.as_millis());
        let elapsed =
            self.elapsed
                .sample(app.pane().turn_timing(), observed.clock_runs(), now, wall);
        let mut frame = ActivityFrame::new(shown, step, elapsed);
        frame.phase_elapsed = self
            .phase
            .shown_since
            .filter(|_| shown.clock_runs())
            .map(|began| now.duration_since(began).as_secs());
        let source = if observed == ObservedActivity::Agent(crate::app::Activity::Thinking) {
            app.pane().transcript.live_thinking()
        } else {
            None
        };
        frame.thinking = self.thinking.sample(source, step).to_owned();
        frame
    }

    fn update_phase(&mut self, app: &App, now: Instant) -> (ObservedActivity, ObservedActivity) {
        if self.target.as_ref() != Some(&app.active) {
            *self = Self {
                target: Some(app.active.clone()),
                ..Self::default()
            };
        }
        let observed = ObservedActivity::of(app);
        let shown = self.phase.sample(observed, now);
        (observed, shown)
    }
}

#[derive(Default)]
struct PhaseClock {
    shown: Option<ObservedActivity>,
    shown_since: Option<Instant>,
    next: Option<(ObservedActivity, Instant)>,
}

impl PhaseClock {
    fn sample(&mut self, observed: ObservedActivity, now: Instant) -> ObservedActivity {
        if let Some(shown) = self
            .shown
            .filter(|shown| *shown != observed && shown.can_coalesce() && observed.can_coalesce())
        {
            let (candidate, began) = self.next.get_or_insert((observed, now));
            if *candidate != observed {
                (*candidate, *began) = (observed, now);
            }
            if now.duration_since(*began) < PHASE_CONFIRMATION {
                return shown;
            }
        }
        if self.shown != Some(observed) {
            self.shown_since = Some(
                self.next
                    .filter(|(candidate, _)| *candidate == observed)
                    .map_or(now, |(_, began)| began),
            );
        }
        self.shown = Some(observed);
        self.next = None;
        observed
    }
}

#[derive(Default)]
struct ElapsedClock {
    anchor: Option<Anchor>,
}

struct Anchor {
    turn: u64,
    started_at_ms: u64,
    origin: Option<(Instant, u64)>,
    seconds: Option<u64>,
}

impl ElapsedClock {
    fn sample(
        &mut self,
        timing: Option<TurnTiming>,
        running: bool,
        now: Instant,
        wall: SystemTime,
    ) -> Option<u64> {
        let Some(timing) = timing else {
            self.anchor = None;
            return None;
        };
        if self.anchor.as_ref().is_none_or(|anchor| {
            anchor.turn != timing.turn || anchor.started_at_ms != timing.started_at_ms
        }) {
            self.anchor = Some(Anchor {
                turn: timing.turn,
                started_at_ms: timing.started_at_ms,
                origin: None,
                seconds: None,
            });
        }
        let anchor = self.anchor.as_mut().expect("established Turn timing");
        if let Some(elapsed) = timing.elapsed_ms {
            anchor.seconds = Some(elapsed / 1000);
            anchor.origin = None;
        } else if running {
            let (at, age) = *anchor.origin.get_or_insert_with(|| {
                let epoch_ms = to_ms(wall.duration_since(UNIX_EPOCH).unwrap_or_default());
                let age = epoch_ms.saturating_sub(anchor.started_at_ms);
                (
                    now,
                    age.max(anchor.seconds.unwrap_or(0).saturating_mul(1000)),
                )
            });
            anchor.seconds = Some(age.saturating_add(to_ms(now.duration_since(at))) / 1000);
        } else {
            anchor.origin = None;
        }
        anchor.seconds
    }
}

fn to_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant, UNIX_EPOCH};

    use crate::app::{Activity, ServiceStatus};

    use super::{ElapsedClock, ObservedActivity, PhaseClock, TurnTiming, PHASE_CONFIRMATION};

    #[test]
    fn thinking_time_stays_anchored_during_updates_and_restarts_after_tools() {
        let mut clock = PhaseClock::default();
        let start = Instant::now();
        let thinking = ObservedActivity::Agent(Activity::Thinking);
        let tools = ObservedActivity::Agent(Activity::RunningTools);
        assert_eq!(thinking, clock.sample(thinking, start));
        assert_eq!(Some(start), clock.shown_since);
        assert_eq!(
            thinking,
            clock.sample(thinking, start + Duration::from_secs(3))
        );
        assert_eq!(Some(start), clock.shown_since);

        let tool_start = start + Duration::from_secs(4);
        clock.sample(tools, tool_start);
        assert_eq!(tools, clock.sample(tools, tool_start + PHASE_CONFIRMATION));
        let second_start = start + Duration::from_secs(5);
        clock.sample(thinking, second_start);
        assert_eq!(
            thinking,
            clock.sample(thinking, second_start + PHASE_CONFIRMATION)
        );
        assert_eq!(Some(second_start), clock.shown_since);
    }

    #[test]
    fn elapsed_is_monotonic_during_work_and_freezes_at_the_durable_end() {
        let mut clock = ElapsedClock::default();
        let start = Instant::now();
        let wall = UNIX_EPOCH + Duration::from_millis(65_900);
        let timing = TurnTiming {
            turn: 9,
            started_at_ms: 1_000,
            elapsed_ms: None,
        };
        assert_eq!(Some(64), clock.sample(Some(timing), true, start, wall));
        assert_eq!(
            Some(65),
            clock.sample(
                Some(timing),
                true,
                start + Duration::from_millis(150),
                UNIX_EPOCH
            )
        );
        assert_eq!(
            Some(65),
            clock.sample(Some(timing), false, start + Duration::from_secs(12), wall)
        );
        assert_eq!(
            Some(89),
            clock.sample(
                Some(timing),
                true,
                start + Duration::from_secs(25),
                wall + Duration::from_secs(25)
            )
        );
        let finished = TurnTiming {
            elapsed_ms: Some(88_500),
            ..timing
        };
        assert_eq!(
            Some(88),
            clock.sample(
                Some(finished),
                false,
                start + Duration::from_secs(40),
                wall + Duration::from_secs(40)
            )
        );
        assert_eq!(
            None,
            clock.sample(None, false, start + Duration::from_secs(44), wall)
        );
    }

    #[test]
    fn ordinary_phase_changes_coalesce_but_attention_and_stop_are_immediate() {
        let mut clock = PhaseClock::default();
        let start = Instant::now();
        let waiting = ObservedActivity::Agent(Activity::WaitingForModel);
        let thinking = ObservedActivity::Agent(Activity::Thinking);
        let streaming = ObservedActivity::Agent(Activity::Streaming);
        let tools = ObservedActivity::Agent(Activity::RunningTools);
        assert_eq!(waiting, clock.sample(waiting, start));
        let half = PHASE_CONFIRMATION / 2;
        assert_eq!(waiting, clock.sample(thinking, start + half));
        assert_eq!(waiting, clock.sample(streaming, start + 2 * half));
        assert_eq!(waiting, clock.sample(thinking, start + 3 * half));
        assert_eq!(waiting, clock.sample(tools, start + 4 * half));
        assert_eq!(tools, clock.sample(tools, start + 6 * half));
        for urgent in [
            ObservedActivity::Agent(Activity::Stopping),
            ObservedActivity::Agent(Activity::Waiting),
            ObservedActivity::Agent(Activity::Failed),
            ObservedActivity::Service(ServiceStatus::Offline),
        ] {
            assert_eq!(
                urgent,
                clock.sample(urgent, start + PHASE_CONFIRMATION * 10)
            );
        }
    }
}
