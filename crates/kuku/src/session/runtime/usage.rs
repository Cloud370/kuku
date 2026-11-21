use std::collections::HashMap;

use serde_json::Value;

use crate::event::EventPayload;
use crate::provider::RequestCost;
use crate::{Error, ProviderUsage, Result};

use super::record::State;
use super::{CostTotals, ModelUsageSample, Session, UsageTotals};

#[derive(Debug, Default)]
pub(super) struct AgentUsage {
    turn: Option<u64>,
    settled: bool,
    attempts: HashMap<String, ModelAttempt>,
    latest_request: Option<String>,
    totals: UsageTotals,
}

#[derive(Debug, Default)]
struct ModelAttempt {
    context_window_tokens: Option<u32>,
    retry: Option<u64>,
    finished: bool,
}

impl AgentUsage {
    pub(super) fn settle(
        &mut self,
        turn: Option<u64>,
        total: &mut UsageTotals,
        mut legacy: UsageTotals,
    ) -> Result<()> {
        if !self.has_turn(turn) {
            if legacy.model_requests > 0 {
                total.latest_request = None;
            }
            if legacy.unreported_requests == 0 {
                if legacy.input_tokens.is_some() {
                    legacy.input_reported_requests = legacy.model_requests;
                }
                if legacy.output_tokens.is_some() {
                    legacy.output_reported_requests = legacy.model_requests;
                }
            }
            add_totals(total, &legacy)?;
        }
        *self = Self {
            turn,
            settled: true,
            ..Self::default()
        };
        Ok(())
    }

    pub(super) fn turn_totals(&self, turn: Option<u64>) -> UsageTotals {
        if self.has_turn(turn) {
            self.totals.clone()
        } else {
            UsageTotals::default()
        }
    }

    fn prepare(
        &mut self,
        request: &str,
        context_window_tokens: Option<u32>,
        usage: &mut UsageTotals,
    ) -> Result<()> {
        if self.attempts.contains_key(request) {
            return Ok(());
        }
        self.attempts.insert(
            request.into(),
            ModelAttempt {
                context_window_tokens,
                ..ModelAttempt::default()
            },
        );
        self.latest_request = Some(request.into());
        self.totals.latest_request = None;
        usage.latest_request = None;
        self.count_request(usage)
    }

    fn retry(&mut self, request: &str, retry: u64, usage: &mut UsageTotals) -> Result<()> {
        let attempt = self
            .attempts
            .get_mut(request)
            .filter(|attempt| !attempt.finished && attempt.retry.is_none_or(|old| retry > old));
        if let Some(attempt) = attempt {
            attempt.retry = Some(retry);
            self.count_request(usage)?;
        }
        Ok(())
    }

    fn finish(
        &mut self,
        request: &str,
        report: ProviderUsage,
        elapsed_us: Option<u64>,
        cost: RequestCost,
        usage: &mut UsageTotals,
    ) -> Result<()> {
        if !self.attempts.contains_key(request) {
            self.prepare(request, None, usage)?;
        }
        let attempt = self.attempts.get_mut(request).expect("prepared request");
        if attempt.finished {
            return Ok(());
        }
        attempt.finished = true;
        if self.latest_request.as_deref() == Some(request) {
            let latest = ModelUsageSample {
                input_tokens: report.input_tokens,
                output_tokens: report.output_tokens,
                context_window_tokens: attempt.context_window_tokens,
                elapsed_us,
            };
            usage.latest_request = Some(latest.clone());
            self.totals.latest_request = Some(latest);
        }
        let complete = report.input_tokens.is_some() && report.output_tokens.is_some();
        let (measured_output_tokens, measured_elapsed_us) = match (report.output_tokens, elapsed_us)
        {
            (Some(output), Some(elapsed)) if elapsed > 0 => (output, elapsed),
            _ => (0, 0),
        };
        let (cache_measured_read_tokens, cache_measured_input_tokens) =
            report.cache_input_counts().unwrap_or_default();
        let costs = match cost {
            RequestCost::Reported { nanodollars } => CostTotals {
                reported_nanodollars: nanodollars,
                reported_requests: 1,
                ..CostTotals::default()
            },
            RequestCost::Estimated { nanodollars, .. } => CostTotals {
                estimated_nanodollars: nanodollars,
                estimated_requests: 1,
                ..CostTotals::default()
            },
            RequestCost::Unknown => CostTotals::default(),
        };
        let delta = UsageTotals {
            input_tokens: report.input_tokens,
            output_tokens: report.output_tokens,
            cache_read_input_tokens: report.cache_read_input_tokens,
            cache_creation_input_tokens: report.cache_creation_input_tokens,
            input_reported_requests: u64::from(report.input_tokens.is_some()),
            output_reported_requests: u64::from(report.output_tokens.is_some()),
            cache_measured_input_tokens,
            cache_measured_read_tokens,
            measured_output_tokens,
            measured_elapsed_us,
            costs,
            ..UsageTotals::default()
        };
        for total in [&mut self.totals, usage] {
            add_totals(total, &delta)?;
            if complete {
                total.unreported_requests -= 1;
            }
        }
        Ok(())
    }

    fn count_request(&mut self, usage: &mut UsageTotals) -> Result<()> {
        let delta = UsageTotals {
            model_requests: 1,
            unreported_requests: 1,
            ..UsageTotals::default()
        };
        add_totals(&mut self.totals, &delta)?;
        add_totals(usage, &delta)
    }

    fn has_turn(&self, turn: Option<u64>) -> bool {
        self.turn.is_some() && self.turn == turn
    }
}

impl Session {
    pub(crate) fn commit_model_facts(
        &self,
        address: &str,
        execution: u64,
        payloads: impl IntoIterator<Item = EventPayload>,
    ) -> Result<()> {
        let mut state = self.inner.state.lock().unwrap();
        self.executing_agent(&state, address, execution)?;
        let mut writer = self.inner.writer.lock().unwrap();
        let writer = writer
            .as_mut()
            .ok_or_else(|| Error::InvalidArgument("session resources are closed".into()))?;
        for payload in payloads {
            let event = writer.append(payload)?;
            state.turns.observe(&event, None);
            if matches!(
                event.payload.kind_name(),
                "runtime.step_prepared" | "runtime.model_retry" | "model.response" | "model.error"
            ) {
                state.agent_mut(address)?.reply_preview = None;
            }
            reduce_model_fact(&mut state, &event.payload)?;
        }
        Ok(())
    }
}

pub(super) fn reduce_model_fact(state: &mut State, payload: &EventPayload) -> Result<()> {
    let EventPayload::Unknown(value) = payload else {
        return Ok(());
    };
    let Some(kind @ ("runtime.step_prepared" | "runtime.model_retry" | "runtime.model_usage")) =
        value.get("kind").and_then(Value::as_str)
    else {
        return Ok(());
    };
    let (Some(address), Some(turn), Some(request)) = (
        value["agent"].as_str(),
        value["turn"].as_u64(),
        value["request_id"].as_str(),
    ) else {
        return Ok(());
    };
    let Some(agent) = state.agents.get_mut(address) else {
        return Ok(());
    };
    if agent.snapshot.turn != Some(turn) {
        return Ok(());
    }
    let ledger = &mut agent.model_usage;
    if !ledger.has_turn(Some(turn)) {
        *ledger = AgentUsage {
            turn: Some(turn),
            ..AgentUsage::default()
        };
    }
    if ledger.settled {
        return Ok(());
    }
    let usage = &mut agent.snapshot.usage;
    match kind {
        "runtime.step_prepared" => ledger.prepare(
            request,
            value["context_window_tokens"]
                .as_u64()
                .and_then(|window| u32::try_from(window).ok()),
            usage,
        ),
        "runtime.model_retry" => match value.get("phase").and_then(Value::as_str) {
            None | Some("started") => {
                ledger.retry(request, value["attempt"].as_u64().unwrap_or(2), usage)
            }
            _ => Ok(()),
        },
        "runtime.model_usage" => {
            let report = serde_json::from_value::<Option<ProviderUsage>>(value["usage"].clone())?
                .unwrap_or_default();
            let cost = serde_json::from_value::<Option<RequestCost>>(value["cost"].clone())?
                .unwrap_or_default();
            ledger.finish(request, report, value["elapsed_us"].as_u64(), cost, usage)
        }
        _ => unreachable!(),
    }
}

fn add_totals(total: &mut UsageTotals, delta: &UsageTotals) -> Result<()> {
    for (tokens, add) in [
        (&mut total.input_tokens, delta.input_tokens),
        (&mut total.output_tokens, delta.output_tokens),
        (
            &mut total.cache_read_input_tokens,
            delta.cache_read_input_tokens,
        ),
        (
            &mut total.cache_creation_input_tokens,
            delta.cache_creation_input_tokens,
        ),
    ] {
        if let Some(add) = add {
            *tokens = Some(checked_add(tokens.unwrap_or(0), add)?);
        }
    }
    for (count, add) in [
        (&mut total.model_requests, delta.model_requests),
        (&mut total.unreported_requests, delta.unreported_requests),
        (
            &mut total.input_reported_requests,
            delta.input_reported_requests,
        ),
        (
            &mut total.output_reported_requests,
            delta.output_reported_requests,
        ),
        (
            &mut total.cache_measured_input_tokens,
            delta.cache_measured_input_tokens,
        ),
        (
            &mut total.cache_measured_read_tokens,
            delta.cache_measured_read_tokens,
        ),
        (
            &mut total.measured_output_tokens,
            delta.measured_output_tokens,
        ),
        (&mut total.measured_elapsed_us, delta.measured_elapsed_us),
        (
            &mut total.costs.reported_nanodollars,
            delta.costs.reported_nanodollars,
        ),
        (
            &mut total.costs.estimated_nanodollars,
            delta.costs.estimated_nanodollars,
        ),
        (
            &mut total.costs.reported_requests,
            delta.costs.reported_requests,
        ),
        (
            &mut total.costs.estimated_requests,
            delta.costs.estimated_requests,
        ),
    ] {
        *count = checked_add(*count, add)?;
    }
    Ok(())
}

fn checked_add(total: u64, add: u64) -> Result<u64> {
    total
        .checked_add(add)
        .ok_or_else(|| Error::InvalidEventStream("usage counter overflow".into()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::event::EventStore;
    use crate::session::runtime::record::Record;

    use super::*;

    fn create_agent(store: &mut EventStore, address: &str) {
        store
            .append(
                Record::AgentCreated {
                    command_id: format!("create-{address}"),
                    fingerprint: String::new(),
                    address: address.into(),
                    config: Default::default(),
                }
                .payload()
                .unwrap(),
            )
            .unwrap();
    }

    fn begin_turn(store: &mut EventStore, address: &str) -> u64 {
        store
            .append(
                Record::InputsAccepted {
                    address: address.into(),
                    input_ids: Vec::new(),
                    message_input_ids: Vec::new(),
                    turn: None,
                }
                .payload()
                .unwrap(),
            )
            .unwrap()
            .id
    }

    fn settle_turn(store: &mut EventStore, address: &str, turn: u64, usage: Value) {
        store
            .append(EventPayload::Unknown(json!({
                "kind": "runtime.operation",
                "record": {"operation": "turn_settled", "address": address,
                    "status": "idle", "turn": turn, "usage": usage}
            })))
            .unwrap();
    }

    fn model_fact(
        store: &mut EventStore,
        address: &str,
        turn: u64,
        request: &str,
        mut fact: Value,
    ) {
        fact["agent"] = json!(address);
        fact["turn"] = json!(turn);
        fact["request_id"] = json!(request);
        store.append(EventPayload::Unknown(fact)).unwrap();
    }

    #[test]
    fn replay_accepts_legacy_totals_and_counts_each_request_attempt_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let mut store = EventStore::open(&path).unwrap();
        create_agent(&mut store, "main");
        let legacy = begin_turn(&mut store, "main");
        settle_turn(
            &mut store,
            "main",
            legacy,
            json!({"input_tokens":17,"output_tokens":9,"cache_read_input_tokens":10,
                "model_requests":1,"unreported_requests":0}),
        );
        let loaded = Session::read_view(&path, "main").unwrap().snapshot.usage;
        assert_eq!(Some(17), loaded.input_tokens);
        assert!(loaded.input_tokens_complete() && loaded.output_tokens_complete());
        assert_eq!(None, loaded.cache_hit_percent());
        assert_eq!(None, loaded.latest_request);
        assert_eq!(None, loaded.average_output_tokens_per_second());

        let turn = begin_turn(&mut store, "main");
        let model_facts = [
            json!({"kind":"runtime.step_prepared","context_window_tokens":100}),
            json!({"kind":"runtime.model_retry","attempt":2}),
            json!({"kind":"runtime.model_usage","usage":{"input_tokens":8,"cache_read_input_tokens":3}}),
        ];
        for fact in model_facts {
            model_fact(&mut store, "main", turn, "req_1", fact.clone());
            model_fact(&mut store, "main", turn, "req_1", fact);
        }
        let before = Session::read_view(&path, "main").unwrap().snapshot.usage;
        assert_eq!(3, before.model_requests);
        assert_eq!(2, before.unreported_requests);
        assert_eq!(Some(25), before.input_tokens);
        assert_eq!(Some(9), before.output_tokens);
        assert!(!before.input_tokens_complete() && !before.output_tokens_complete());
        assert_eq!(Some(8.0), before.context_percent());
        assert_eq!(None, before.output_tokens_per_second());
        assert_eq!(None, before.average_output_tokens_per_second());
        assert_eq!(Some(37.5), before.cache_hit_percent());
        settle_turn(
            &mut store,
            "main",
            turn,
            json!({"input_tokens":8,"cache_read_input_tokens":3,
                "model_requests":2,"unreported_requests":2}),
        );
        assert_eq!(
            before,
            Session::read_view(&path, "main").unwrap().snapshot.usage
        );
    }

    #[test]
    fn retry_usage_counts_started_and_legacy_events_but_not_waiting_or_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let mut store = EventStore::open(&path).unwrap();
        create_agent(&mut store, "main");
        let turn = begin_turn(&mut store, "main");
        model_fact(
            &mut store,
            "main",
            turn,
            "req_1",
            json!({"kind":"runtime.step_prepared"}),
        );
        for phase in ["waiting", "stopped"] {
            model_fact(
                &mut store,
                "main",
                turn,
                "req_1",
                json!({"kind":"runtime.model_retry","phase":phase,"attempt":2}),
            );
        }
        assert_eq!(
            1,
            Session::read_view(&path, "main")
                .unwrap()
                .snapshot
                .usage
                .model_requests
        );

        model_fact(
            &mut store,
            "main",
            turn,
            "req_1",
            json!({"kind":"runtime.model_retry","phase":"started","attempt":2}),
        );
        model_fact(
            &mut store,
            "main",
            turn,
            "req_1",
            json!({"kind":"runtime.model_retry","attempt":3}),
        );
        assert_eq!(
            3,
            Session::read_view(&path, "main")
                .unwrap()
                .snapshot
                .usage
                .model_requests
        );
    }

    #[test]
    fn cache_hits_weight_only_paired_reports_and_survive_unreported_attempts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let mut store = EventStore::open(&path).unwrap();
        create_agent(&mut store, "main");
        let turn = begin_turn(&mut store, "main");
        let reports = [
            json!({"input_tokens":10,"cache_read_input_tokens":0}),
            json!({"input_tokens":90,"cache_read_input_tokens":90}),
            json!({"input_tokens":500}),
            json!({"cache_read_input_tokens":30}),
            json!({"input_tokens":8,"cache_read_input_tokens":6,"cache_creation_input_tokens":4}),
            json!({"input_tokens":0,"cache_read_input_tokens":0}),
        ];
        for (index, usage) in reports.into_iter().enumerate() {
            model_fact(
                &mut store,
                "main",
                turn,
                &format!("request-{index}"),
                json!({"kind":"runtime.model_usage","usage":usage}),
            );
        }
        let previous = Session::read_view(&path, "main").unwrap().snapshot.usage;
        assert_eq!(Some(608), previous.input_tokens);
        assert_eq!(Some(126), previous.cache_read_input_tokens);
        assert_eq!(100, previous.cache_measured_input_tokens);
        assert_eq!(90, previous.cache_measured_read_tokens);
        assert_eq!(Some(90.0), previous.cache_hit_percent());
        settle_turn(&mut store, "main", turn, json!(previous));

        let next = begin_turn(&mut store, "main");
        for kind in ["runtime.step_prepared", "runtime.model_retry"] {
            model_fact(&mut store, "main", next, "unreported", json!({"kind":kind}));
        }
        let reopened = Session::read_view(&path, "main").unwrap().snapshot.usage;
        assert_eq!(8, reopened.model_requests);
        assert_eq!(Some(90.0), reopened.cache_hit_percent());
        assert_eq!(100, reopened.cache_measured_input_tokens);
        assert_eq!(90, reopened.cache_measured_read_tokens);

        create_agent(&mut store, "reviewer");
        let review = Session::read_view(&path, "reviewer")
            .unwrap()
            .snapshot
            .usage;
        assert_eq!(None, review.cache_hit_percent());
    }

    #[test]
    fn replays_weighted_throughput_for_each_agent_across_turns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let mut store = EventStore::open(&path).unwrap();
        for (agent, output) in [("main", 40), ("reviewer", 0)] {
            create_agent(&mut store, agent);
            let turn = begin_turn(&mut store, agent);
            let measurement = json!({
                "kind": "runtime.model_usage", "elapsed_us": 1_000_000,
                "usage": {"output_tokens": output}
            });
            model_fact(&mut store, agent, turn, "req_1", measurement.clone());
            model_fact(&mut store, agent, turn, "req_1", measurement);
            let usage = Session::read_view(&path, agent).unwrap().snapshot.usage;
            assert_eq!(
                Some(f64::from(output)),
                usage.average_output_tokens_per_second()
            );
            settle_turn(&mut store, agent, turn, json!(usage));
        }

        let turn = begin_turn(&mut store, "main");
        model_fact(
            &mut store,
            "main",
            turn,
            "req_1",
            json!({"kind": "runtime.step_prepared", "context_window_tokens": 100}),
        );
        let running = Session::read_view(&path, "main").unwrap().snapshot.usage;
        assert_eq!(None, running.output_tokens_per_second());
        assert_eq!(Some(40.0), running.average_output_tokens_per_second());

        for (request, output, elapsed) in [
            ("req_1", Some(8), Some(3_000_000)),
            ("req_2", Some(0), Some(2_000_000)),
            ("req_3", Some(50), None),
            ("req_4", Some(50), Some(0)),
            ("req_5", None, Some(5_000_000)),
        ] {
            model_fact(
                &mut store,
                "main",
                turn,
                request,
                json!({
                    "kind": "runtime.model_usage", "elapsed_us": elapsed,
                    "usage": {"output_tokens": output}
                }),
            );
        }
        let main = Session::read_view(&path, "main").unwrap().snapshot.usage;
        assert_eq!(Some(148), main.output_tokens);
        assert_eq!(48, main.measured_output_tokens);
        assert_eq!(6_000_000, main.measured_elapsed_us);
        assert_eq!(Some(8.0), main.average_output_tokens_per_second());
        assert_eq!(None, main.output_tokens_per_second());
        let reviewer = Session::read_view(&path, "reviewer")
            .unwrap()
            .snapshot
            .usage;
        assert_eq!(Some(0.0), reviewer.average_output_tokens_per_second());
    }

    #[test]
    fn zero_denominators_are_unknown() {
        let usage = UsageTotals {
            input_tokens: Some(0),
            cache_read_input_tokens: Some(0),
            model_requests: 1,
            latest_request: Some(ModelUsageSample {
                input_tokens: Some(0),
                output_tokens: Some(0),
                context_window_tokens: Some(0),
                elapsed_us: Some(0),
            }),
            ..UsageTotals::default()
        };
        assert_eq!(None, usage.cache_hit_percent());
        assert_eq!(None, usage.context_percent());
        assert_eq!(None, usage.output_tokens_per_second());
        assert_eq!(None, usage.average_output_tokens_per_second());
    }
}
