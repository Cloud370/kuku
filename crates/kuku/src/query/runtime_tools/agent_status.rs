use std::path::Path;

use serde_json::{Value, json};

use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::{AgentSnapshot, AgentStatus};

const REQUEST_LOOKBACK: usize = 128;

pub(super) fn listing(agent: AgentSnapshot, events_path: Option<&Path>) -> Value {
    let mut item = serde_json::json!({"address": agent.address, "status": agent.status});
    if agent.pending_inputs > 0 {
        item["pending_inputs"] = agent.pending_inputs.into();
    }
    if agent.permission.is_some() {
        item["needs_approval"] = true.into();
    }
    if agent.question.is_some() {
        item["needs_reply"] = true.into();
    }
    if let Some(error) = agent.last_error {
        item["last_error"] = error.into();
    } else if agent.status == AgentStatus::Idle {
        if let Some(output) = agent.last_output {
            if events_path.is_some() {
                item["last_output"] = output.into();
            } else {
                item["reply_available"] = true.into();
            }
        }
    }
    if let (Some(path), Some(turn), AgentStatus::Idle) = (events_path, agent.turn, agent.status) {
        if let Some(request) = EventStore::read_page_before(path, None, REQUEST_LOOKBACK)
            .ok()
            .and_then(|events| last_request(&events, &agent.address, turn))
        {
            item["last_request"] = request;
        }
    }
    item
}

fn last_request(events: &[StoredEvent], address: &str, turn: u64) -> Option<Value> {
    let facts = events.iter().rev().filter_map(|event| {
        let EventPayload::Unknown(value) = &event.payload else {
            return None;
        };
        (value["agent"] == address && value["turn"] == turn).then_some(value)
    });
    let mut reports = Vec::new();
    for fact in facts {
        match fact["kind"].as_str() {
            Some("runtime.provider_report") => reports.push(fact),
            Some("runtime.step_prepared") => {
                let request_id = fact["request_id"].as_str()?;
                let mut result = json!({"model": fact["model"].as_str()?});
                if let Some(tier) = fact["settings"]["service_tier"].as_str() {
                    result["requested_service_tier"] = tier.into();
                }
                if let Some(tier) = reports
                    .into_iter()
                    .filter(|report| report["request_id"] == request_id)
                    .filter_map(|report| report["report"]["service_tier"].as_str())
                    .find(|tier| !matches!(*tier, "" | "auto"))
                {
                    result["reported_service_tier"] = tier.into();
                }
                return Some(result);
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::last_request;
    use crate::event::{EventPayload, StoredEvent};

    #[test]
    fn reports_only_the_latest_request_for_the_settled_turn_and_agent() {
        let event = |id, payload| StoredEvent {
            id,
            payload: EventPayload::Unknown(payload),
        };
        let prepared = |request: &str, tier: Value| {
            json!({"kind": "runtime.step_prepared", "agent": "worker", "turn": 7,
                   "request_id": request, "model": "example/model", "settings": {"service_tier": tier}})
        };
        let report = |address: &str, turn, request: &str, tier: &str| {
            json!({"kind": "runtime.provider_report", "agent": address, "turn": turn,
                   "request_id": request, "report": {"service_tier": tier}})
        };
        let events = [
            event(1, prepared("old", json!("priority"))),
            event(2, report("worker", 7, "old", "priority")),
            event(3, prepared("new", json!("flex"))),
            event(4, report("worker", 7, "new", "default")),
            event(5, report("worker", 7, "new", "auto")),
            event(6, report("worker", 8, "new", "priority")),
            event(7, report("other", 7, "new", "priority")),
            event(8, report("worker", 7, "old", "priority")),
        ];
        assert_eq!(
            last_request(&events, "worker", 7),
            Some(
                json!({"model": "example/model", "requested_service_tier": "flex",
                        "reported_service_tier": "default"})
            )
        );
        assert_eq!(
            last_request(&events[..3], "worker", 7),
            Some(json!({"model": "example/model", "requested_service_tier": "flex"}))
        );
        assert_eq!(last_request(&events, "worker", 8), None);
    }
}
