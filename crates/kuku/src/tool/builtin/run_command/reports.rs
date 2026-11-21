use std::sync::LazyLock;

use regex::Regex;

use crate::execution::{ReportedTests, TestReportFormat};

const MAX_REPORT_EVIDENCE: usize = 2;
const MAX_REPORT_LINE_BYTES: usize = 192;

static RUST_REPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^test result: (?:ok|FAILED)\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored(?:; ([0-9]+) measured)?(?:; ([0-9]+) filtered out)?(?:; finished in [0-9]+(?:\.[0-9]+)?s)?$")
        .expect("valid Rust test report pattern")
});
static PYTEST_REPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?P<counts>[0-9]+ [a-z]+(?:, [0-9]+ [a-z]+)*) in [0-9]+(?:\.[0-9]+)?s(?: \([0-9:]+\))?$",
    )
    .expect("valid pytest report pattern")
});

#[derive(Default)]
pub(super) struct ReportCollector {
    reports: Vec<ReportedTests>,
    first_rust_failure: Option<String>,
    first_pytest_failure: Option<String>,
}

impl ReportCollector {
    pub(super) fn observe(&mut self, line: &str) {
        if self.first_rust_failure.is_none() {
            self.first_rust_failure = line
                .strip_prefix("test ")
                .and_then(|line| line.strip_suffix(" ... FAILED"))
                .map(bounded);
        }
        if self.first_pytest_failure.is_none() {
            self.first_pytest_failure = line
                .strip_prefix("FAILED ")
                .or_else(|| line.strip_prefix("ERROR "))
                .filter(|name| name.contains("::"))
                .map(|name| bounded(name.split_once(" - ").map_or(name, |(test, _)| test)));
        }
        if let Some(report) = rust_report(line).or_else(|| pytest_report(line)) {
            self.add(report, line);
        }
    }

    pub(super) fn finish(mut self) -> Vec<ReportedTests> {
        for report in &mut self.reports {
            if report.failed == 0 && report.errors == 0 {
                continue;
            }
            report.first_failure = match report.format {
                TestReportFormat::Rust => self.first_rust_failure.take(),
                TestReportFormat::Pytest => self.first_pytest_failure.take(),
            };
        }
        self.reports
    }

    fn add(&mut self, observed: ReportedTests, line: &str) {
        let report = match self
            .reports
            .iter()
            .position(|report| report.format == observed.format)
        {
            Some(index) => &mut self.reports[index],
            None => {
                self.reports.push(empty_report(observed.format));
                self.reports.last_mut().unwrap()
            }
        };
        report.suites = report.suites.saturating_add(1);
        report.passed = report.passed.saturating_add(observed.passed);
        report.failed = report.failed.saturating_add(observed.failed);
        report.errors = report.errors.saturating_add(observed.errors);
        report.skipped = report.skipped.saturating_add(observed.skipped);
        report.measured = report.measured.saturating_add(observed.measured);
        report.filtered_out = report.filtered_out.saturating_add(observed.filtered_out);
        report.expected_failures = report
            .expected_failures
            .saturating_add(observed.expected_failures);
        report.unexpected_passes = report
            .unexpected_passes
            .saturating_add(observed.unexpected_passes);
        report.warnings = report.warnings.saturating_add(observed.warnings);
        if report.evidence.len() < MAX_REPORT_EVIDENCE {
            report.evidence.push(bounded(line));
            report.evidence_truncated |= line.len() > MAX_REPORT_LINE_BYTES;
        } else {
            report.evidence_truncated = true;
        }
    }
}

pub(super) fn rust_report(line: &str) -> Option<ReportedTests> {
    if !line.starts_with("test result: ") {
        return None;
    }
    let counts = RUST_REPORT.captures(line)?;
    let mut report = empty_report(TestReportFormat::Rust);
    report.passed = counts[1].parse().ok()?;
    report.failed = counts[2].parse().ok()?;
    report.skipped = counts[3].parse().ok()?;
    report.measured = counts
        .get(4)
        .map_or("0", |value| value.as_str())
        .parse()
        .ok()?;
    report.filtered_out = counts
        .get(5)
        .map_or("0", |value| value.as_str())
        .parse()
        .ok()?;
    Some(report)
}

pub(super) fn pytest_report(line: &str) -> Option<ReportedTests> {
    let decorated = line.starts_with('=');
    if decorated != line.ends_with('=') {
        return None;
    }
    let line = line.trim_matches('=').trim();
    if !line.as_bytes().first()?.is_ascii_digit() {
        return None;
    }
    let counts = PYTEST_REPORT.captures(line)?;
    let mut report = empty_report(TestReportFormat::Pytest);
    for count in counts["counts"].split(", ") {
        let (number, label) = count.split_once(' ')?;
        let number = number.parse::<u64>().ok()?;
        let observed = match label {
            "passed" => &mut report.passed,
            "failed" => &mut report.failed,
            "error" | "errors" => &mut report.errors,
            "skipped" => &mut report.skipped,
            "deselected" => &mut report.filtered_out,
            "xfailed" => &mut report.expected_failures,
            "xpassed" => &mut report.unexpected_passes,
            "warning" | "warnings" => &mut report.warnings,
            _ => return None,
        };
        if *observed != 0 {
            return None;
        }
        *observed = number;
    }
    Some(report)
}

fn empty_report(format: TestReportFormat) -> ReportedTests {
    ReportedTests {
        format,
        suites: 0,
        passed: 0,
        failed: 0,
        errors: 0,
        skipped: 0,
        measured: 0,
        filtered_out: 0,
        expected_failures: 0,
        unexpected_passes: 0,
        warnings: 0,
        first_failure: None,
        evidence: Vec::new(),
        evidence_truncated: false,
    }
}

fn bounded(line: &str) -> String {
    line[..line.floor_char_boundary(MAX_REPORT_LINE_BYTES.min(line.len()))]
        .chars()
        .filter(|character| !character.is_control())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_reports_aggregate_counts_and_keep_the_first_failure() {
        let mut collector = ReportCollector::default();
        collector.observe("test parser::quoted ... FAILED");
        collector.observe("test result: FAILED. 3 passed; 1 failed; 2 ignored; 0 measured; 9 filtered out; finished in 0.01s");
        collector.observe("test result: ok. 5 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s");
        let reports = collector.finish();
        assert_eq!(1, reports.len());
        assert_eq!(8, reports[0].passed);
        assert_eq!(1, reports[0].failed);
        assert_eq!(3, reports[0].skipped);
        assert_eq!(9, reports[0].filtered_out);
        assert_eq!(Some("parser::quoted"), reports[0].first_failure.as_deref());
    }

    #[test]
    fn pytest_report_recognizes_its_complete_final_summary() {
        let mut collector = ReportCollector::default();
        collector.observe("we expected 10 passed and 0 failed");
        collector.observe("FAILED tests/test_csv.py::test_quotes - AssertionError");
        collector.observe("===== 1 failed, 4 passed, 2 skipped in 0.14s =====");
        let reports = collector.finish();
        assert_eq!(1, reports.len());
        assert_eq!(TestReportFormat::Pytest, reports[0].format);
        assert_eq!(4, reports[0].passed);
        assert_eq!(1, reports[0].failed);
        assert_eq!(2, reports[0].skipped);
        assert_eq!(
            Some("tests/test_csv.py::test_quotes"),
            reports[0].first_failure.as_deref()
        );
    }

    #[test]
    fn pytest_quiet_separates_the_runner_outcomes() {
        let mut collector = ReportCollector::default();
        collector.observe("7 passed, malformed summary");
        collector.observe("2 errors, 1 warning, 4 passed, 3 xfailed, 1 xpassed in 0.50s");
        let reports = collector.finish();
        assert_eq!(1, reports[0].suites);
        assert_eq!(4, reports[0].passed);
        assert_eq!(0, reports[0].failed);
        assert_eq!(2, reports[0].errors);
        assert_eq!(3, reports[0].expected_failures);
        assert_eq!(1, reports[0].unexpected_passes);
        assert_eq!(1, reports[0].warnings);
    }
}
