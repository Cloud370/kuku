use serde::Deserialize;
use serde_json::Value;

use crate::text::{cap, service_tier_label, shorten, single_line};

const MAX_SCAN_BYTES: usize = 32 * 1024;
const MAX_LABEL_BYTES: usize = 200;
const MAX_AGENTS_SCANNED: usize = 256;
const MAX_AGENTS_SHOWN: usize = 3;
const MAX_EVENT_ROWS: usize = 128;

#[derive(Debug, Clone)]
pub(super) struct Observation {
    tool: &'static str,
    pub(super) caption: String,
    pub(super) detail: String,
    pub(super) attention: bool,
    participants: Vec<String>,
}

impl Observation {
    pub(super) fn from_result(
        structured: &Value,
        summary: &str,
        content: &str,
        truncated: bool,
    ) -> Option<Self> {
        if let Some(count) = summary.strip_suffix(" events returned") {
            return Some(query(count.parse().ok()?, content, truncated));
        }
        if summary == "session operation committed" {
            if let Some(agents) = structured.as_array() {
                return listing(agents);
            }
            if content.len() <= MAX_SCAN_BYTES && content.trim_start().starts_with('[') {
                let agents: Vec<Value> = serde_json::from_str(content).ok()?;
                return listing(&agents);
            }
        }
        None
    }

    pub(super) fn checks_delegation(&self, sender: &str, recipient: &str) -> bool {
        !self.attention
            && self.participants.iter().any(|name| name == recipient)
            && self
                .participants
                .iter()
                .all(|name| name == sender || name == recipient)
    }

    pub(super) fn single_agent(&self) -> Option<&str> {
        (self.tool == "list_agents" && self.participants.len() == 1).then_some(&self.detail)
    }

    pub(super) fn is_for(&self, tool: &str) -> bool {
        self.tool == tool
    }
}

pub(super) fn is_session_read(tool: &str) -> bool {
    matches!(tool, "list_agents" | "query_session")
}

fn listing(agents: &[Value]) -> Option<Observation> {
    let mut shown = Vec::new();
    let mut attention = 0;
    for entry in agents.iter().take(MAX_AGENTS_SCANNED) {
        let address = entry["address"]
            .as_str()
            .filter(|address| !address.is_empty())?;
        let state = AgentState::at_check(entry);
        attention += usize::from(state.needs_attention());
        let rank = shown.partition_point(|agent: &AgentFact| agent.state <= state);
        if rank < MAX_AGENTS_SHOWN {
            shown.insert(
                rank,
                AgentFact {
                    address: clip(address, 22),
                    state,
                    reply: (has_text(&entry["last_output"]) || entry["reply_available"] == true),
                    queued: entry["pending_inputs"].as_u64().unwrap_or_default(),
                },
            );
            shown.truncate(MAX_AGENTS_SHOWN);
        }
    }
    let caption = match agents.len() {
        1 => "1 agent".to_owned(),
        count => format!("{count} agents"),
    };
    let service = if let [entry] = agents {
        match (
            entry["last_request"]["requested_service_tier"].as_str(),
            entry["last_request"]["reported_service_tier"].as_str(),
        ) {
            (Some(requested), Some(reported))
                if !matches!(requested, "auto" | "default" | "standard") =>
            {
                let requested = service_tier_label(Some(requested));
                let reported = service_tier_label(Some(reported));
                if requested == reported {
                    Some(reported)
                } else {
                    Some(format!("Requested {requested} · Reported {reported}"))
                }
            }
            _ => None,
        }
    } else {
        None
    };
    let unshown = agents.len() - shown.len();
    let mut detail = shown.into_iter().map(AgentFact::label).collect::<Vec<_>>();
    if let Some(service) = service {
        detail.push(service);
    }
    if unshown > 0 {
        detail.push(format!("+{unshown} more"));
    }
    let caption = if attention == 0 {
        caption
    } else if agents.len() > MAX_AGENTS_SCANNED {
        format!("{caption} · {attention}+ need attention")
    } else {
        format!("{caption} · {attention} need attention")
    };
    Some(Observation {
        tool: "list_agents",
        participants: if agents.len() <= 2 {
            agents
                .iter()
                .filter_map(|agent| agent["address"].as_str().map(str::to_owned))
                .collect()
        } else {
            Vec::new()
        },
        caption,
        detail: detail.join(" · "),
        attention: attention > 0,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AgentState {
    Approval,
    Reply,
    Waiting,
    Interrupted,
    Stopped,
    Failed,
    Running,
    Idle,
    Unknown,
}

impl AgentState {
    fn at_check(value: &Value) -> Self {
        if value["permission"].is_object() || value["needs_approval"] == true {
            return Self::Approval;
        }
        if value["question"].is_object() || value["needs_reply"] == true {
            return Self::Reply;
        }
        match value["status"].as_str() {
            Some("waiting") => Self::Waiting,
            Some("interrupted") => Self::Interrupted,
            Some("stopped") => Self::Stopped,
            Some("running") => Self::Running,
            Some("idle") if has_text(&value["last_error"]) => Self::Failed,
            Some("idle") => Self::Idle,
            _ => Self::Unknown,
        }
    }

    fn needs_attention(self) -> bool {
        matches!(
            self,
            Self::Approval
                | Self::Reply
                | Self::Waiting
                | Self::Interrupted
                | Self::Stopped
                | Self::Failed
        )
    }

    fn label(self) -> &'static str {
        match self {
            Self::Approval => "Waiting for approval",
            Self::Reply => "Waiting for reply",
            Self::Waiting => "Waiting",
            Self::Interrupted => "Interrupted",
            Self::Stopped => "Stopped",
            Self::Failed => "Last run failed",
            Self::Running => "Running",
            Self::Idle => "Idle",
            Self::Unknown => "Status unknown",
        }
    }
}

struct AgentFact {
    address: String,
    state: AgentState,
    reply: bool,
    queued: u64,
}

impl AgentFact {
    fn label(self) -> String {
        let mut facts = self.state.label().to_owned();
        if self.reply {
            facts.push_str(", reply available");
        }
        if self.queued > 0 {
            facts.push_str(&format!(", {} queued", self.queued));
        }
        format!("{} ({facts})", self.address)
    }
}

fn has_text(value: &Value) -> bool {
    value.as_str().is_some_and(|text| !text.is_empty())
}

fn query(count: usize, content: &str, truncated: bool) -> Observation {
    let mut caption = if count == 1 {
        "1 event matched".to_owned()
    } else {
        format!("{count} events matched")
    };
    let content = cap(content, MAX_SCAN_BYTES).trim();
    let mut counts = [0; EventKind::ALL.len()];
    if let Some(lines) = content.strip_prefix("[\n") {
        for line in lines.lines().take(MAX_EVENT_ROWS) {
            if let Ok(event) = serde_json::from_str::<QueryEvent>(line.trim().trim_end_matches(','))
            {
                counts[event.kind() as usize] += 1;
            }
        }
    } else if let Ok(events) = serde_json::from_str::<Vec<QueryEvent>>(content) {
        for event in events.iter().take(MAX_EVENT_ROWS) {
            counts[event.kind() as usize] += 1;
        }
    }
    let detail = EventKind::ALL
        .into_iter()
        .zip(counts)
        .filter(|&(_, n)| n > 0)
        .map(|(kind, n)| format!("{n} {}", kind.label(n)))
        .collect::<Vec<_>>();
    if truncated || counts.iter().sum::<usize>() != count {
        caption.push_str(" · Preview incomplete");
    }
    let attention = counts[EventKind::Error as usize] + counts[EventKind::Retry as usize] > 0;
    Observation {
        tool: "query_session",
        participants: Vec::new(),
        caption,
        detail: detail.join(" · "),
        attention,
    }
}

#[derive(Deserialize)]
struct QueryEvent {
    kind: String,
}

impl QueryEvent {
    fn kind(&self) -> EventKind {
        match self.kind.as_str() {
            "model.recovery" | "runtime.model_retry" => EventKind::Retry,
            "model.error" => EventKind::Error,
            value if value.starts_with("message.") => EventKind::Message,
            value if value.starts_with("model.") => EventKind::Model,
            value if value.starts_with("tool.") => EventKind::Tool,
            value if value.starts_with("permission.") => EventKind::Permission,
            value if value.starts_with("turn.") => EventKind::Turn,
            _ => EventKind::Other,
        }
    }
}

#[derive(Clone, Copy)]
enum EventKind {
    Error,
    Retry,
    Message,
    Model,
    Tool,
    Permission,
    Turn,
    Other,
}

impl EventKind {
    const ALL: [Self; 8] = [
        Self::Error,
        Self::Retry,
        Self::Message,
        Self::Model,
        Self::Tool,
        Self::Permission,
        Self::Turn,
        Self::Other,
    ];

    fn label(self, count: usize) -> &'static str {
        match (self, count == 1) {
            (Self::Error, true) => "model error",
            (Self::Error, false) => "model errors",
            (Self::Retry, true) => "retry",
            (Self::Retry, false) => "retries",
            (Self::Message, true) => "message",
            (Self::Message, false) => "messages",
            (Self::Model, true) => "model event",
            (Self::Model, false) => "model events",
            (Self::Tool, true) => "tool event",
            (Self::Tool, false) => "tool events",
            (Self::Permission, true) => "permission event",
            (Self::Permission, false) => "permission events",
            (Self::Turn, true) => "turn event",
            (Self::Turn, false) => "turn events",
            (Self::Other, _) => "other",
        }
    }
}

pub(super) fn query_target(args: &Value) -> String {
    let mut terms = Vec::new();
    if let Some(address) = args["conversation"]
        .as_str()
        .filter(|value| !value.is_empty())
    {
        terms.push(format!("Agent {}", clip(address, 22)));
    }
    if let Some(kind) = args["kind"].as_str().filter(|value| !value.is_empty()) {
        let label = match kind {
            "message.user" => "User messages".to_owned(),
            "message.assistant" => "Assistant messages".to_owned(),
            "tool.call" => "Tool calls".to_owned(),
            "tool.result" => "Tool results".to_owned(),
            "model.response" => "Model replies".to_owned(),
            "model.error" => "Model errors".to_owned(),
            other => format!("Kind {}", clip(other, 22)),
        };
        terms.push(label);
    }
    if let Some(term) = args["search"].as_str().filter(|value| !value.is_empty()) {
        terms.push(format!("Search ‘{}’", clip(term, 26)));
    }
    if let Some(after) = args["after"].as_u64().filter(|&after| after > 0) {
        terms.push(format!("After #{after}"));
    }
    terms.join(" · ")
}

fn clip(text: &str, cells: usize) -> String {
    shorten(&single_line(cap(text, MAX_LABEL_BYTES)), cells)
}
