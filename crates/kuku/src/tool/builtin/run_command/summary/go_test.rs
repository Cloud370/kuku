use std::borrow::Cow;

use crate::execution::{CommandOutputRange, CommandOutputSummary, OutputStream};

use super::{EvidenceLines, Rule, Summary};

#[derive(Default)]
pub(super) struct Report {
    ok: usize,
    no_tests: usize,
    failed: usize,
    build_failed: usize,
    example: Option<(String, CommandOutputRange)>,
    details: EvidenceLines,
}

enum PackageResult {
    Ok,
    NoTests,
    Failed,
    BuildFailed,
}

impl Report {
    pub(super) fn observe(&mut self, line: Cow<'_, str>, source: CommandOutputRange) -> bool {
        if source.stream == OutputStream::Stdout {
            let trimmed = line.trim_start();
            if trimmed.starts_with("=== ")
                || trimmed.starts_with("--- PASS:")
                || trimmed.starts_with("--- SKIP:")
            {
                return false;
            }
            match package_result(&line) {
                Ok(Some(PackageResult::Ok)) => {
                    self.ok += 1;
                    self.remember_example(line, source);
                    return true;
                }
                Ok(Some(PackageResult::NoTests)) => {
                    self.no_tests += 1;
                    self.remember_example(line, source);
                    return true;
                }
                Ok(Some(PackageResult::Failed)) => self.failed += 1,
                Ok(Some(PackageResult::BuildFailed)) => self.build_failed += 1,
                Ok(None) => {}
                Err(()) => return false,
            }
        }
        line.trim().is_empty() || self.details.push(line, source)
    }

    fn remember_example(&mut self, line: Cow<'_, str>, source: CommandOutputRange) {
        if self.example.is_none() {
            self.example = Some((line.into_owned(), source));
        }
    }

    pub(super) fn summarize(self, rule: &Rule, exit_code: i32) -> Option<Summary> {
        let failures = self.failed + self.build_failed;
        if self.ok + self.no_tests + failures == 0 || (exit_code != 0) != (failures != 0) {
            return None;
        }
        let mut text = format!(
            "Go test packages: {} ok, {} no test files, {} failed, {} build failed.",
            self.ok, self.no_tests, self.failed, self.build_failed
        );
        let mut ranges = Vec::new();
        if self.details.entries.is_empty() {
            let (sample, range) = self.example?;
            text.push_str("\nExample package result: ");
            text.push_str(&sample);
            ranges.push(range);
        } else {
            for detail in self.details.entries {
                text.push('\n');
                text.push_str(&detail.text);
                ranges.push(detail.source);
            }
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

fn package_result(line: &str) -> Result<Option<PackageResult>, ()> {
    let Some((label, body)) = line.split_once('\t') else {
        return Ok(None);
    };
    let label = label.trim_end();
    if !matches!(label, "ok" | "?" | "FAIL") {
        return Ok(None);
    }
    if label == "FAIL"
        && body
            .strip_suffix(" [build failed]")
            .is_some_and(is_import_path)
    {
        return Ok(Some(PackageResult::BuildFailed));
    }
    let (package, report) = body.split_once('\t').ok_or(())?;
    if !is_import_path(package) {
        return Err(());
    }
    match (label, report) {
        ("ok", "(cached)") => Ok(Some(PackageResult::Ok)),
        ("ok", value) if is_elapsed(value) => Ok(Some(PackageResult::Ok)),
        ("?", "[no test files]") => Ok(Some(PackageResult::NoTests)),
        ("FAIL", value) if is_elapsed(value) => Ok(Some(PackageResult::Failed)),
        _ => Err(()),
    }
}

fn is_import_path(value: &str) -> bool {
    !value.is_empty() && !value.contains(char::is_whitespace)
}

fn is_elapsed(value: &str) -> bool {
    value
        .strip_suffix('s')
        .and_then(|seconds| seconds.parse::<f64>().ok())
        .is_some_and(|seconds| seconds.is_finite() && seconds >= 0.0)
}
