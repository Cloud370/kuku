use std::fmt::Write;
use std::sync::LazyLock;

use regex::Regex;

use crate::execution::{CommandOutputRange, CommandOutputSummary, OutputStream};

use super::{EvidenceLine, Rule, Summary};

const MAX_GROUPS: usize = 8;
const LOCATIONS_PER_GROUP: usize = 3;
static PLAIN_DIAGNOSTIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:(?P<location>.+\([1-9][0-9]*,[1-9][0-9]*\)): )?error (?P<code>TS[0-9]+): (?P<message>\S.*)$")
        .expect("valid plain TypeScript diagnostic pattern")
});

struct DiagnosticGroup<'a> {
    code: &'a str,
    message: &'a str,
    count: usize,
    samples: Vec<(Option<&'a str>, &'a CommandOutputRange)>,
}

pub(super) fn summarize(rule: &Rule, lines: &[EvidenceLine], exit_code: i32) -> Option<Summary> {
    if exit_code == 0 {
        return None;
    }
    let mut groups = Vec::<DiagnosticGroup<'_>>::new();
    for line in lines.iter().filter(|line| !line.blank) {
        if line.source.stream != OutputStream::Stdout {
            return None;
        }
        let diagnostic = PLAIN_DIAGNOSTIC.captures(&line.text)?;
        let code = diagnostic.name("code")?.as_str();
        let message = diagnostic.name("message")?.as_str();
        let location = diagnostic
            .name("location")
            .map(|location| location.as_str());
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.code == code && group.message == message)
        {
            group.count += 1;
            if group.samples.len() < LOCATIONS_PER_GROUP {
                group.samples.push((location, &line.source));
            }
        } else {
            if groups.len() == MAX_GROUPS {
                return None;
            }
            groups.push(DiagnosticGroup {
                code,
                message,
                count: 1,
                samples: vec![(location, &line.source)],
            });
        }
    }
    if groups.iter().all(|group| group.count == 1) {
        return None;
    }
    let mut text = String::from(
        "Grouped by exact diagnostic code and message. Locations are file(line,column).",
    );
    let mut ranges = Vec::new();
    for group in groups {
        let unit = if group.count == 1 {
            "occurrence"
        } else {
            "occurrences"
        };
        write!(
            text,
            "\n\n{} ({} {unit})\n{}",
            group.code, group.count, group.message
        )
        .ok()?;
        for (location, range) in group.samples {
            if let Some(location) = location {
                write!(text, "\n  {location}").ok()?;
            }
            ranges.push(range.clone());
        }
    }
    Some(Summary {
        text,
        source: CommandOutputSummary::Rule {
            rule: rule.name.clone(),
            ranges,
        },
    })
}
