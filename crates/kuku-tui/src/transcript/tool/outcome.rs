use kuku_client::Receipt;
use ratatui::style::Color;
use serde_json::{Map, Value};

use crate::text::cap;
use crate::theme::{ACCENT, ERROR, MUTED, SUCCESS, WARNING};

use super::command::CommandPreview;
use super::file_change::FileChange;
use super::script::ScriptPreview;
use super::session::Observation;
use super::{capped, MAX_DETAIL_BYTES, MAX_LABEL_BYTES};

#[derive(Debug, Clone, Default)]
pub(super) enum State {
    #[default]
    Requested,
    Running,
    Reported(Outcome),
    Recorded(Outcome),
}

impl State {
    pub(super) fn outcome(&self) -> Option<&Outcome> {
        match self {
            Self::Requested | Self::Running => None,
            Self::Reported(outcome) | Self::Recorded(outcome) => Some(outcome),
        }
    }

    pub(super) fn label(&self) -> (&str, Color) {
        match self {
            Self::Requested => ("requested", MUTED),
            Self::Running => ("running", ACCENT),
            Self::Reported(outcome) | Self::Recorded(outcome) => outcome.label(),
        }
    }
}

impl Outcome {
    fn label(&self) -> (&str, Color) {
        if self.background_job {
            return ("started in background", ACCENT);
        }
        if let Some(command) = &self.command_preview {
            return command.label();
        }
        if let Some(label) = self
            .script
            .as_ref()
            .and_then(|script| script.label(&self.status))
        {
            return label;
        }
        match self.status.as_str() {
            "ok" if self.exit_code.is_some_and(|code| code != 0) => ("failed", ERROR),
            "ok" if self.attention => ("ok with warnings", WARNING),
            "ok" => ("ok", SUCCESS),
            "error" => ("error", ERROR),
            "blocked" | "cancelled" => (&self.status, WARNING),
            "" => ("finished", MUTED),
            status => (status, MUTED),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct Outcome {
    pub(super) status: String,
    pub(super) summary: String,
    pub(super) invocation: String,
    pub(super) content: String,
    pub(super) diff: String,
    pub(super) exit_code: Option<i64>,
    pub(super) output_id: Option<String>,
    pub(super) inline_output_complete: bool,
    pub(super) command_preview: Option<Box<CommandPreview>>,
    pub(super) script: Option<Box<ScriptPreview>>,
    pub(super) file_read: Option<FileRead>,
    pub(super) receipt: Option<Receipt>,
    pub(super) observation: Option<Observation>,
    pub(super) change: Option<FileChange>,
    pub(super) attention: bool,
    background_job: bool,
}

impl Outcome {
    pub(super) fn from_fields(fields: &Map<String, Value>, fallback_summary: Option<&str>) -> Self {
        let get = |key| fields.get(key).and_then(Value::as_str);
        let structured = fields
            .get("structured")
            .or_else(|| fields.get("result"))
            .unwrap_or(&Value::Null);
        let content = get("model_content");
        let summary = get("summary").or(fallback_summary);
        let succeeded = get("status") == Some("ok");
        let background_job = structured["kind"].as_str() == Some("job");
        let output_id = if background_job {
            None
        } else {
            get("output_id")
                .or_else(|| structured["output_id"].as_str())
                .or_else(|| structured["output_ref"]["id"].as_str())
        };
        let truncated = fields
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let diff = structured["diff"].as_str().or_else(|| get("diff"));
        let inline_output_complete = !truncated
            && structured["capture_complete"].as_bool() != Some(false)
            && structured["quota_truncated"].as_bool() != Some(true)
            && structured["output_preview_truncated"].as_bool() != Some(true)
            && structured["change"]["preview_truncated"].as_bool() != Some(true)
            && structured["timed_out"].as_bool() != Some(true)
            && structured["cancelled"].as_bool() != Some(true)
            && !content.is_some_and(|text| cap(text, MAX_DETAIL_BYTES).len() < text.len())
            && !diff.is_some_and(|text| cap(text, MAX_DETAIL_BYTES).len() < text.len());
        let command_preview = CommandPreview::from_result(structured);
        let attention = [
            "cleanup_incomplete",
            "quota_truncated",
            "output_persistence_failed",
            "timed_out",
            "cancelled",
        ]
        .into_iter()
        .any(|key| structured[key].as_bool() == Some(true))
            || structured["capture_complete"].as_bool() == Some(false)
            || structured["stderr_lines"]
                .as_u64()
                .is_some_and(|lines| lines > 0);
        let change = FileChange::from_result(structured, truncated)
            .filter(|change| succeeded || change.unconfirmed);
        Self {
            status: capped(get("status"), MAX_LABEL_BYTES),
            summary: capped(summary, MAX_DETAIL_BYTES),
            invocation: command_preview.as_ref().map_or_else(
                || capped(structured["path"].as_str(), MAX_DETAIL_BYTES),
                |command| command.invocation(),
            ),
            content: capped(content, MAX_DETAIL_BYTES),
            diff: capped(diff, MAX_DETAIL_BYTES),
            exit_code: command_preview
                .as_ref()
                .and_then(|command| command.exit_code())
                .or_else(|| structured["exit_code"].as_i64()),
            output_id: output_id.map(str::to_owned),
            inline_output_complete,
            command_preview,
            script: ScriptPreview::from_result(structured),
            file_read: FileRead::from_result(structured, truncated),
            receipt: succeeded.then(|| agent_receipt(structured)).flatten(),
            observation: succeeded
                .then(|| {
                    Observation::from_result(
                        structured,
                        summary.unwrap_or_default(),
                        content.unwrap_or_default(),
                        truncated,
                    )
                })
                .flatten(),
            change,
            attention,
            background_job,
        }
    }

    pub(super) fn evidence(&self) -> String {
        if let Some(script) = &self.script {
            return script.evidence(&self.status);
        }
        self.command_preview
            .as_ref()
            .map_or_else(String::new, |command| command.evidence())
    }
}

fn agent_receipt(structured: &Value) -> Option<Receipt> {
    let agent = structured["agent"].as_str()?;
    let command_id = structured["command_id"].as_str()?;
    let event_id = structured["event_id"].as_u64()?;
    if agent.is_empty() || command_id.is_empty() || event_id == 0 {
        return None;
    }
    Some(Receipt {
        agent: agent.to_owned(),
        command_id: command_id.to_owned(),
        event_id,
    })
}

#[derive(Debug, Clone)]
pub(super) struct FileRead {
    start: u64,
    count: u64,
    total: u64,
    cached: bool,
    truncated: bool,
}

impl FileRead {
    fn from_result(structured: &Value, truncated: bool) -> Option<Self> {
        if structured["kind"].as_str() != Some("file_content") {
            return None;
        }
        Some(Self {
            start: structured["start_line"].as_u64()?,
            count: structured["line_count"].as_u64()?,
            total: structured["total_lines"].as_u64()?,
            cached: structured["cached"].as_bool().unwrap_or(false),
            truncated,
        })
    }

    pub(super) fn caption(&self) -> String {
        let Self {
            start,
            count,
            total,
            cached,
            truncated,
        } = *self;
        let mut caption = match (total, count) {
            (0, _) => "empty file".to_owned(),
            (_, 0) => format!("no lines at {start} of {total}"),
            (_, 1) => format!("line {start} of {total}"),
            _ => {
                let end = start.saturating_add(count - 1);
                format!("lines {start}–{end} of {total}")
            }
        };
        if cached {
            caption.push_str(" · cached");
        }
        if truncated {
            caption.push_str(" · truncated");
        }
        caption
    }
}
