use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::{Regex, RegexSet};
use serde::Deserialize;

const MAX_LINES: usize = 256;

static ANSI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\x1b(?:\][^\x07\x1b]*(?:\x07|\x1b\\)|\[[0-?]*[ -/]*[@-~])")
        .expect("valid ANSI control sequence pattern")
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    schema_version: u32,
    filters: BTreeMap<String, Definition>,
    #[serde(default)]
    tests: BTreeMap<String, Vec<Fixture>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Definition {
    description: Option<String>,
    match_command: String,
    #[serde(default)]
    strip_ansi: bool,
    #[serde(default)]
    strip_lines_matching: Vec<String>,
    #[serde(default)]
    keep_lines_matching: Vec<String>,
    head_lines: Option<usize>,
    tail_lines: Option<usize>,
    max_lines: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    name: String,
    input: String,
    expected: String,
}

pub(super) struct Rule {
    pub(super) name: String,
    command: Regex,
    strip_ansi: bool,
    filter: Filter,
    head: Option<usize>,
    tail: Option<usize>,
    max: Option<usize>,
}

enum Filter {
    All,
    Strip(RegexSet),
    Keep(RegexSet),
}

pub(super) enum Selection {
    Line(usize),
    Omitted(usize),
    Truncated(usize),
}

impl Rule {
    fn compile(name: String, def: Definition) -> Result<Self, String> {
        if def
            .description
            .as_deref()
            .is_some_and(|description| description.trim().is_empty())
            || def.match_command.trim().is_empty()
        {
            return Err("match_command and any description must be nonempty".into());
        }
        let filter = match (
            def.strip_lines_matching.is_empty(),
            def.keep_lines_matching.is_empty(),
        ) {
            (true, true) => Filter::All,
            (false, true) => Filter::Strip(regex_set(def.strip_lines_matching)?),
            (true, false) => Filter::Keep(regex_set(def.keep_lines_matching)?),
            (false, false) => return Err("strip and keep patterns are mutually exclusive".into()),
        };
        if [def.head_lines, def.tail_lines, def.max_lines]
            .into_iter()
            .flatten()
            .any(|limit| !(1..=MAX_LINES).contains(&limit))
        {
            return Err(format!("line bounds must be in 1..={MAX_LINES}"));
        }
        Ok(Self {
            name,
            command: Regex::new(&def.match_command).map_err(|error| error.to_string())?,
            strip_ansi: def.strip_ansi,
            filter,
            head: def.head_lines,
            tail: def.tail_lines,
            max: def.max_lines,
        })
    }

    pub(super) fn prepare_line<'a>(&self, line: &'a str) -> Cow<'a, str> {
        if self.strip_ansi {
            ANSI.replace_all(line, "")
        } else {
            Cow::Borrowed(line)
        }
    }

    pub(super) fn keeps(&self, line: &str) -> bool {
        match &self.filter {
            Filter::All => true,
            Filter::Strip(regex) => !regex.is_match(line),
            Filter::Keep(regex) => regex.is_match(line),
        }
    }

    pub(super) fn select_lines(&self, count: usize) -> Vec<Selection> {
        let head = self.head.unwrap_or(0).min(count);
        let tail = self.tail.unwrap_or(0).min(count.saturating_sub(head));
        let mut selected = if (self.head.is_some() || self.tail.is_some()) && count > head + tail {
            (0..head)
                .map(Selection::Line)
                .chain(std::iter::once(Selection::Omitted(count - head - tail)))
                .chain((count - tail..count).map(Selection::Line))
                .collect::<Vec<_>>()
        } else {
            (0..count).map(Selection::Line).collect()
        };
        if let Some(max) = self.max.filter(|max| selected.len() > *max) {
            let dropped = selected.len() - max;
            selected.truncate(max);
            selected.push(Selection::Truncated(dropped));
        }
        selected
    }

    fn verify(&self, fixture: &Fixture) -> Result<(), String> {
        let lines = fixture
            .input
            .lines()
            .map(|line| self.prepare_line(line))
            .filter(|line| self.keeps(line))
            .collect::<Vec<_>>();
        let actual = self
            .select_lines(lines.len())
            .into_iter()
            .map(|entry| match entry {
                Selection::Line(index) => lines[index].clone().into_owned(),
                Selection::Omitted(count) => format!("... ({count} lines omitted)"),
                Selection::Truncated(count) => format!("... ({count} lines truncated)"),
            })
            .collect::<Vec<_>>()
            .join("\n");
        (actual == fixture.expected)
            .then_some(())
            .ok_or_else(|| format!("fixture {:?} did not match", fixture.name))
    }
}

fn regex_set(patterns: Vec<String>) -> Result<RegexSet, String> {
    RegexSet::new(patterns).map_err(|error| error.to_string())
}

fn load(source: &str) -> Result<Vec<Rule>, String> {
    let file: File = toml::from_str(source).map_err(|error| error.to_string())?;
    if file.schema_version != 1 {
        return Err(format!(
            "unsupported command output rule schema {}",
            file.schema_version
        ));
    }
    let mut fixtures = file.tests;
    let mut rules = Vec::new();
    for (name, def) in file.filters {
        let rule = Rule::compile(name.clone(), def).map_err(|error| format!("{name}: {error}"))?;
        for fixture in fixtures.remove(&name).into_iter().flatten() {
            rule.verify(&fixture)
                .map_err(|error| format!("{name}: {error}"))?;
        }
        rules.push(rule);
    }
    if let Some((name, _)) = fixtures.first_key_value() {
        return Err(format!("fixtures reference an undeclared rule {name:?}"));
    }
    Ok(rules)
}

const BUILTIN: &str = include_str!("../../../../../assets/command-output.toml");
static RULES: LazyLock<Result<Vec<Rule>, String>> = LazyLock::new(|| load(BUILTIN));

pub(super) fn select(command: &str) -> Option<&'static Rule> {
    let mut matches = RULES
        .as_ref()
        .ok()?
        .iter()
        .filter(|rule| rule.command.is_match(command));
    let selected = matches.next()?;
    matches.next().is_none().then_some(selected)
}

#[cfg(test)]
mod tests {
    use super::{BUILTIN, load};

    #[test]
    fn built_in_fixtures_and_unsupported_rule_fields_are_checked() {
        let names = load(BUILTIN)
            .unwrap()
            .into_iter()
            .map(|rule| rule.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "cargo-humans",
                "git-status",
                "go-test-human",
                "node-test-spec",
                "pytest-human",
                "tsc-diagnostics"
            ]
        );
        let unsupported = BUILTIN.replace("strip_ansi = true", "replace = []");
        assert!(load(&unsupported).err().unwrap().contains("unknown field"));
        let conflicting = BUILTIN.replace("head_lines = 16", "keep_lines_matching = ['error']");
        assert!(
            load(&conflicting)
                .err()
                .unwrap()
                .contains("mutually exclusive")
        );
    }

    #[test]
    fn declarative_cut_applies_max_after_head_and_tail_markers() {
        let rule = r#"
schema_version = 1
[filters.demo]
match_command = '^demo$'
keep_lines_matching = ['^keep']
head_lines = 1
tail_lines = 1
max_lines = 2
[[tests.demo]]
name = "head, omission, truncation"
input = "skip\nkeep A\nkeep B\nkeep C\nkeep D\n"
expected = "keep A\n... (2 lines omitted)\n... (1 lines truncated)"
"#;
        assert!(load(rule).is_ok());
    }
}
