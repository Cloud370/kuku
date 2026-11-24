use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::config::ProviderConfig;
use crate::event::ModelStopReason;

use super::trial::{TrialProblem, run_trial};
use super::{ModelTrial, TrialResult};

const DISCOVERY_VALUE: &str = "__kuku_unknown_effort__";
const MAX_LEVELS: usize = 8;
const COMMON_LEVELS: &[&str] = &["none", "low", "medium", "high", "max"];

/// A request for supported values or a check of one value through the runtime encoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReasoningCheck {
    Discover,
    Level(String),
}

/// Evidence of parameter acceptance and any separate upstream report of its effect.
pub enum ReasoningCheckResult {
    Candidates {
        values: Vec<String>,
        declared: bool,
        trial: Option<Box<TrialResult>>,
    },
    Accepted(Box<TrialResult>),
    Rejected(String),
}

fn valid_level(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 24
        && value != "provider-default"
        && value != DISCOVERY_VALUE
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(super) fn bounded_levels<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut levels = Vec::new();
    for value in values {
        if valid_level(value) && !levels.iter().any(|level| level == value) {
            levels.push(value.to_owned());
            if levels.len() == MAX_LEVELS {
                break;
            }
        }
    }
    levels
}

pub(super) fn reply_accepted(trial: &TrialResult) -> bool {
    trial.text_returned
        || trial.observed_reasoning
        || matches!(
            trial.stop_reason,
            Some(ModelStopReason::EndTurn | ModelStopReason::Length)
        )
}

pub(super) async fn check_reasoning(
    connection: &ProviderConfig,
    model: &str,
    service_tier: Option<String>,
    check: ReasoningCheck,
) -> Result<ReasoningCheckResult, TrialProblem> {
    let effort = match &check {
        ReasoningCheck::Discover => DISCOVERY_VALUE,
        ReasoningCheck::Level(level) if valid_level(level) => level,
        ReasoningCheck::Level(_) => {
            return Err(crate::Error::InvalidArgument("Invalid thinking value.".into()).into());
        }
    };
    let result = run_trial(
        connection,
        model,
        ModelTrial::Text {
            effort: None,
            service_tier,
        },
        Some(effort),
        None,
    )
    .await;
    match (check, result) {
        (ReasoningCheck::Level(_), Ok(trial)) if reply_accepted(&trial) => {
            Ok(ReasoningCheckResult::Accepted(Box::new(trial)))
        }
        (ReasoningCheck::Level(_), Ok(_)) => {
            Err(crate::Error::InvalidArgument("No usable model reply.".into()).into())
        }
        (ReasoningCheck::Level(_), Err(problem)) if is_parameter_error(&problem) => {
            Ok(ReasoningCheckResult::Rejected(problem.error.to_string()))
        }
        (ReasoningCheck::Discover, Ok(trial)) => Ok(ReasoningCheckResult::Candidates {
            values: bounded_levels(COMMON_LEVELS.iter().copied()),
            declared: false,
            trial: Some(Box::new(trial)),
        }),
        (ReasoningCheck::Discover, Err(problem)) if matches!(problem.status, Some(400 | 422)) => {
            let values = response_levels(&problem.parameter_error);
            if values.is_empty() {
                return Ok(ReasoningCheckResult::Candidates {
                    values: bounded_levels(COMMON_LEVELS.iter().copied()),
                    declared: false,
                    trial: None,
                });
            }
            Ok(ReasoningCheckResult::Candidates {
                values,
                declared: true,
                trial: None,
            })
        }
        (_, Err(problem)) => Err(problem),
    }
}

fn reasoning_field(value: &str) -> bool {
    [
        "reasoning_effort",
        "reasoning effort",
        ".effort",
        "thinking",
    ]
    .iter()
    .any(|field| value.contains(field))
}

fn is_parameter_error(problem: &TrialProblem) -> bool {
    matches!(problem.status, Some(400 | 422))
        && reasoning_field(&problem.parameter_error.to_ascii_lowercase())
}

fn response_levels(body: &str) -> Vec<String> {
    let Ok(body) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let error = body.get("error").unwrap_or(&body);
    let field = error
        .get("param")
        .or_else(|| error.get("field"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if reasoning_field(field) || reasoning_field(&message.to_ascii_lowercase()) {
        for name in ["allowed_values", "supported_values"] {
            if let Some(values) = error.get(name).and_then(Value::as_array) {
                let levels = bounded_levels(values.iter().filter_map(Value::as_str));
                if !levels.is_empty() {
                    return levels;
                }
            }
        }
    }
    if !reasoning_field(field)
        && !reasoning_field(&message.to_ascii_lowercase())
        && !message.contains(DISCOVERY_VALUE)
    {
        return Vec::new();
    }
    static ENUM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?:expected(?:\s+one\s+of)?|one of|allowed values|supported values)\s*:?\s*([^.;\n]+)").expect("enum regex")
    });
    let Some(values) = ENUM.captures(message).and_then(|matched| matched.get(1)) else {
        return Vec::new();
    };
    let inner = values.as_str().trim().trim_matches(['[', ']', '(', ')']);
    let values = inner
        .split([',', '|'])
        .map(|value| value.trim().trim_matches(['\'', '"', '`']))
        .collect::<Vec<_>>();
    if values.len() < 2 || !values.iter().all(|value| valid_level(value)) {
        return Vec::new();
    }
    bounded_levels(values)
}
