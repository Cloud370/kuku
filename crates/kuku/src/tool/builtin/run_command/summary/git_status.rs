use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::Write;

use crate::execution::{CommandOutputRange, CommandOutputSummary, OutputStream};

use super::{Rule, Summary};

const SAMPLES_PER_GROUP: usize = 2;

#[derive(Default)]
pub(super) struct Status {
    branch: Option<Sample>,
    groups: BTreeMap<Group, Rows>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    Unmerged,
    StagedAndUnstaged,
    Staged,
    Unstaged,
    Untracked,
    Ignored,
}

#[derive(Default)]
struct Rows {
    count: usize,
    samples: Vec<Sample>,
}

struct Sample {
    text: String,
    source: CommandOutputRange,
}

impl Status {
    pub(super) fn observe(&mut self, line: Cow<'_, str>, source: CommandOutputRange) -> bool {
        if source.stream != OutputStream::Stdout {
            return false;
        }
        if line.starts_with("## ") && self.branch.is_none() && self.groups.is_empty() {
            self.branch = Some(Sample {
                text: line.into_owned(),
                source,
            });
            return true;
        }
        let [x, y, b' ', path @ ..] = line.as_bytes() else {
            return false;
        };
        let Some(group) = Group::from_status(*x, *y).filter(|_| !path.is_empty()) else {
            return false;
        };
        let rows = self.groups.entry(group).or_default();
        rows.count += 1;
        if rows.samples.len() < SAMPLES_PER_GROUP {
            rows.samples.push(Sample {
                text: line.into_owned(),
                source,
            });
        }
        true
    }

    pub(super) fn summarize(self, rule: &Rule, exit_code: i32) -> Option<Summary> {
        if exit_code != 0 || self.groups.is_empty() {
            return None;
        }
        let mut text = String::from(
            "Git status rows start with a two-character status and a space. Paths keep Git's quoting.",
        );
        let mut ranges = Vec::new();
        if let Some(branch) = self.branch {
            write!(text, "\n{}", branch.text).ok()?;
            ranges.push(branch.source);
        }
        for (group, rows) in self.groups {
            write!(text, "\n\n{} ({})", group.label(), rows.count).ok()?;
            for sample in rows.samples {
                write!(text, "\n{}", sample.text).ok()?;
                ranges.push(sample.source);
            }
        }
        Some(Summary {
            text,
            source: CommandOutputSummary::Rule {
                rule: rule.name.clone(),
                ranges,
            },
            script_hint: Some(
                "For exact rows in run_js, iterate tools.output_lines({id, stream:\"stdout\"}). Skip the optional ## branch line. Filter by line.text.slice(0, 2) and keep line.text.slice(3) with Git's quoting.",
            ),
        })
    }
}

impl Group {
    fn from_status(index: u8, worktree: u8) -> Option<Self> {
        Some(match (index, worktree) {
            (b'D', b'D' | b'U') | (b'A', b'A' | b'U') | (b'U', b'D' | b'A' | b'U') => {
                Self::Unmerged
            }
            (b'?', b'?') => Self::Untracked,
            (b'!', b'!') => Self::Ignored,
            (b'M' | b'T' | b'A' | b'D' | b'R' | b'C', b' ') => Self::Staged,
            (b' ', b'M' | b'T' | b'A' | b'D' | b'R' | b'C' | b'm' | b'?') => Self::Unstaged,
            (b'M' | b'T' | b'A' | b'R' | b'C', b'M' | b'T' | b'D' | b'm' | b'?') => {
                Self::StagedAndUnstaged
            }
            _ => return None,
        })
    }

    fn label(self) -> &'static str {
        match self {
            Self::Unmerged => "Unmerged",
            Self::StagedAndUnstaged => "Staged and unstaged",
            Self::Staged => "Staged only",
            Self::Unstaged => "Unstaged only",
            Self::Untracked => "Untracked",
            Self::Ignored => "Ignored",
        }
    }
}
