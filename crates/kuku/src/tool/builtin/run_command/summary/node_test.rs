use std::borrow::Cow;
use std::collections::HashSet;

use crate::execution::{CommandOutputRange, CommandOutputSummary, OutputStream};

use super::{EvidenceLines, Rule, Summary};

const COUNTERS: [&str; 7] = [
    "tests",
    "suites",
    "pass",
    "fail",
    "cancelled",
    "skipped",
    "todo",
];
const FOOTER_LINES: usize = COUNTERS.len() + 1;

#[derive(Default)]
pub(super) struct Report {
    numbers: [u64; COUNTERS.len()],
    counters: Vec<CommandOutputRange>,
    sample: Option<(String, CommandOutputRange)>,
    details: EvidenceLines,
    failure_start: Option<usize>,
    folded_frames: bool,
}

impl Report {
    pub(super) fn observe(&mut self, line: Cow<'_, str>, source: CommandOutputRange) -> bool {
        if source.stream == OutputStream::Stdout {
            let index = self.counters.len();
            if index < FOOTER_LINES && (index != 0 || line.starts_with("ℹ tests ")) {
                return self.observe_counter(&line, source);
            }
            if index == 0 && passing_row(line.trim_start()) {
                if self.sample.is_none() {
                    self.sample = Some((line.into_owned(), source));
                }
                return true;
            }
            if index == FOOTER_LINES {
                if line == "✖ failing tests:" {
                    self.failure_start.get_or_insert(self.details.entries.len());
                }
                if self.failure_start.is_some() && runtime_stack_frame(line.trim_start()) {
                    self.folded_frames = true;
                    return true;
                }
            }
        }
        self.details.push(line, source)
    }

    fn observe_counter(&mut self, line: &str, source: CommandOutputRange) -> bool {
        let Some((field, text)) = line.strip_prefix("ℹ ").and_then(|row| row.split_once(' '))
        else {
            return false;
        };
        let index = self.counters.len();
        if let Some(expected) = COUNTERS.get(index) {
            let Ok(value) = text.parse::<u64>() else {
                return false;
            };
            if field != *expected {
                return false;
            }
            self.numbers[index] = value;
        } else if field != "duration_ms" || !is_duration(text) {
            return false;
        }
        self.counters.push(source);
        true
    }

    pub(super) fn summarize(self, rule: &Rule, exit_code: i32) -> Option<Summary> {
        let [tests, suites, passed, failed, cancelled, skipped, todo] = self.numbers;
        let outcomes = [passed, failed, cancelled, skipped, todo];
        if self.counters.len() != FOOTER_LINES
            || outcomes.into_iter().try_fold(0, u64::checked_add)? != tests
            || (exit_code != 0) != (failed != 0 || cancelled != 0)
        {
            return None;
        }
        let mut text = format!(
            "Node.js tests: {tests} total, {passed} passed, {failed} failed, {cancelled} cancelled, {skipped} skipped, {todo} todo. Suites: {suites}."
        );
        let mut ranges = self.counters;
        let details = &self.details.entries;
        let header = self.failure_start.unwrap_or(details.len());
        let repeated_titles = details[header..]
            .iter()
            .filter(|line| {
                line.source.stream == OutputStream::Stdout
                    && (line.text.starts_with("✖ ") || line.text.starts_with("⚠ "))
            })
            .map(|line| line.text.as_str())
            .collect::<HashSet<_>>();
        for (index, line) in details.iter().enumerate() {
            if index < header
                && line.source.stream == OutputStream::Stdout
                && repeated_titles.contains(line.text.trim_start())
            {
                continue;
            }
            text.push('\n');
            text.push_str(&line.text);
            ranges.push(line.source.clone());
        }
        if let Some((sample, source)) = self.sample.filter(|_| exit_code == 0) {
            text.push_str("\nExample passed result: ");
            text.push_str(sample.trim_start());
            ranges.push(source);
        }
        if self.folded_frames {
            text.push_str("\nFull stack traces are available in retained output.");
        }
        Some(Summary {
            text,
            source: CommandOutputSummary::Rule {
                rule: rule.name.clone(),
                ranges,
            },
            script_hint: None,
        })
    }
}

fn passing_row(line: &str) -> bool {
    line.strip_prefix("✔ ")
        .and_then(|row| row.strip_suffix("ms)"))
        .and_then(|row| row.rsplit_once(" ("))
        .is_some_and(|(name, duration)| !name.is_empty() && is_duration(duration))
}

fn runtime_stack_frame(line: &str) -> bool {
    let Some(frame) = line.strip_prefix("at ") else {
        return false;
    };
    let frame = frame.strip_suffix(" {").unwrap_or(frame);
    frame.starts_with("node:")
        || frame.rsplit_once(" (").is_some_and(|(_, location)| {
            location.starts_with("node:") || location == "<anonymous>)"
        })
}

fn is_duration(value: &str) -> bool {
    value
        .parse::<f64>()
        .is_ok_and(|value| value.is_finite() && value >= 0.0)
}
