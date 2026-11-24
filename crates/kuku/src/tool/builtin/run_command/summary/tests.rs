use crate::execution::{
    CommandExecutionFacts, CommandInvocation, CommandOutputSummary, OutputStream,
};

use super::super::super::test_helpers::workspace;
use super::super::output::{OutputLimits, OutputRead, read_output};
use super::super::{CommandCapture, CommandCwd, ModelOutputAccess, run_command_to};
use super::Collector;

fn cargo(args: &[&str]) -> CommandInvocation {
    CommandInvocation::Program {
        program: "cargo".into(),
        args: args.iter().map(|arg| (*arg).into()).collect(),
    }
}

fn node(args: &[&str]) -> CommandInvocation {
    CommandInvocation::Program {
        program: "node".into(),
        args: args.iter().map(|arg| (*arg).into()).collect(),
    }
}

#[test]
fn node_spec_folds_passing_rows_and_preserves_diagnostic_ranges() {
    let invocation = node(&["--test", "--test-reporter", "spec", "fixture.cjs"]);
    let report = concat!(
        "▶ routes\n",
        "  ✔ confirms the route (1ms)\n",
        "  ✔ another passing case (2ms)\n",
        "  ✖ reports a mismatch (3ms)\n",
        "ℹ tests 4\nℹ suites 1\nℹ pass 2\nℹ fail 1\nℹ cancelled 0\nℹ skipped 0\nℹ todo 1\n",
        "ℹ duration_ms 25.7\n",
        "✖ failing tests:\n",
        "test at fixture.cjs:7:3\n",
        "✖ reports a mismatch (3ms)\n",
        "  AssertionError: route selection mismatch\n",
        "      at TestContext.<anonymous> (/app/fixture.cjs:8:12)\n",
        "      at Test.runInAsyncScope (node:async_hooks:227:14)\n",
        "      at Array.map (<anonymous>) {\n",
        "  ⚠ repair empty cache # TODO\n"
    );
    let error = "node: fixture warning\n";
    let mut collector = Collector::new(&invocation).unwrap();
    collector.observe(OutputStream::Stdout, 0, report.as_bytes());
    collector.observe(OutputStream::Stderr, report.len() as u64, error.as_bytes());
    let summary = collector.summarize(1).unwrap();
    assert!(summary.text.contains("4 total, 2 passed, 1 failed"));
    assert!(summary.text.contains("Suites: 1"));
    assert!(summary.text.contains("1 todo"));
    assert!(!summary.text.contains("confirms the route"));
    assert!(!summary.text.contains("another passing case"));
    assert!(
        summary
            .text
            .contains("AssertionError: route selection mismatch")
    );
    assert!(summary.text.contains("/app/fixture.cjs:8:12"));
    assert!(!summary.text.contains("node:async_hooks"));
    assert!(!summary.text.contains("Array.map"));
    assert!(summary.text.contains("Full stack traces are available"));
    assert!(summary.text.contains("repair empty cache # TODO"));
    assert_eq!(1, summary.text.matches("reports a mismatch").count());
    assert!(summary.text.contains("node: fixture warning"));
    let CommandOutputSummary::Rule { rule, ranges } = summary.source;
    assert_eq!("node-test-spec", rule);
    assert!(ranges.iter().any(|span| {
        span.stream == OutputStream::Stdout
            && report[span.offset as usize..span.end_offset as usize]
                .contains("route selection mismatch")
    }));
    assert!(
        ranges.iter().any(|span| {
            span.stream == OutputStream::Stderr && span.offset == report.len() as u64
        })
    );
}

#[test]
fn node_spec_requires_complete_consistent_counts_and_human_reporter() {
    for args in [
        &["--test", "--test-reporter=tap"][..],
        &["--test", "--test-reporter", "junit"],
        &["--test", "--test-reporter=spec", "--test-reporter=spec"],
        &["--test", "--test-reporter-destination=output.txt"],
        &["--test", "--experimental-test-coverage"],
        &["fixture.cjs", "--test"],
    ] {
        assert!(Collector::new(&node(args)).is_none(), "{args:?}");
    }
    let passing = concat!(
        "✔ passes (1ms)\n",
        "ℹ tests 2\nℹ suites 0\nℹ pass 1\nℹ fail 0\nℹ cancelled 0\nℹ skipped 0\nℹ todo 1\n",
        "ℹ duration_ms 8\n\n✖ failing tests:\n  ⚠ expected todo diagnostic\n"
    );
    let cases = [
        (passing.into(), 0, true),
        (passing.into(), 1, false),
        (passing.replace("tests 2", "tests 3"), 0, false),
        (passing.replace("ℹ suites 0\n", ""), 0, false),
        (
            "TAP version 13\nok 1 - passes\n# tests 1\n".into(),
            0,
            false,
        ),
    ];
    for (report, exit, expected) in cases {
        let mut collector = Collector::new(&node(&["--test", "fixture.cjs"])).unwrap();
        collector.observe(OutputStream::Stdout, 0, report.as_bytes());
        assert_eq!(expected, collector.summarize(exit).is_some(), "{report}");
    }
}

#[test]
fn split_line_uses_merged_offsets_and_keeps_its_original_pipe() {
    let mut collector = Collector::new(&cargo(&["check"])).unwrap();
    let prefix = b"\x1b[1merror[E0308]";
    let unrelated = b"build script details\n";
    let suffix = b"\x1b[0m: mismatched types\n  --> src/lib.rs:8:1\n";
    collector.observe(OutputStream::Stderr, 0, prefix);
    collector.observe(OutputStream::Stdout, prefix.len() as u64, unrelated);
    collector.observe(
        OutputStream::Stderr,
        (prefix.len() + unrelated.len()) as u64,
        suffix,
    );
    let summary = collector.summarize(101).unwrap();
    assert!(
        summary
            .text
            .contains("error[E0308]: mismatched types\n  --> src/lib.rs:8:1")
    );
    let CommandOutputSummary::Rule { ranges, .. } = summary.source;
    assert_eq!(ranges.len(), 2);
    assert_eq!(ranges[1].stream, OutputStream::Stderr);
    assert_eq!(ranges[1].offset, 0);
    assert_eq!(
        ranges[1].end_offset,
        (prefix.len() + unrelated.len() + suffix.len()) as u64
    );
}

#[test]
fn declines_machine_reporters_and_unrecognized_captures() {
    for args in [
        &["check", "--message-format=json"][..],
        &["test", "--", "--format", "json"],
        &["test", "--", "--nocapture"],
        &["nextest", "run"],
        &["check --quiet"],
    ] {
        assert!(Collector::new(&cargo(args)).is_none(), "{args:?}");
    }
    for output in [
        &b"arbitrary output\n"[..],
        b"   Finished dev in 0.1s\n{\"reason\":\"build-finished\"}\n",
        b"   Finished dev in 0.1s\npartial",
    ] {
        let mut collector = Collector::new(&cargo(&["build"])).unwrap();
        collector.observe(OutputStream::Stderr, 0, output);
        assert!(collector.summarize(0).is_none());
    }
}

#[test]
fn pytest_rule_requires_a_test_report_and_preserves_failure_context() {
    let invocation = |program: &str, args: &[&str]| CommandInvocation::Program {
        program: program.into(),
        args: args.iter().map(|arg| (*arg).into()).collect(),
    };
    let python = invocation("python3.10", &["-m", "pytest", "-v", "--color=yes"]);
    let mut collector = Collector::new(&python).unwrap();
    let mut output =
        String::from("test_checkout.py::test_checkout[0] \x1b[31mFAILED\x1b[0m [  1%]\n");
    for index in 1..85 {
        output.push_str(&format!(
            "test_checkout.py::test_checkout[{index}] \x1b[32mPASSED\x1b[0m [100%]\n"
        ));
    }
    let location = output.len() as u64;
    output.push_str("E       AssertionError: minimum checkout amount\ntest_checkout.py:5: AssertionError\n========================= 1 failed, 84 passed in 0.06s =========================\n");
    collector.observe(OutputStream::Stdout, 0, output.as_bytes());
    let summary = collector.summarize(1).unwrap();
    let CommandOutputSummary::Rule { rule, ranges } = &summary.source;
    assert_eq!(rule, "pytest-human");
    assert!(summary.text.contains("minimum checkout amount"));
    assert!(summary.text.contains("test_checkout.py:5: AssertionError"));
    assert!(summary.text.contains("1 failed, 84 passed"));
    assert!(!summary.text.contains(" PASSED "));
    assert!(
        ranges
            .iter()
            .any(|range| range.stream == OutputStream::Stdout && range.offset == location)
    );

    for mode in [
        "--collect-only",
        "--capture=tee-sys",
        "--json-report",
        "--fixtures",
    ] {
        assert!(
            Collector::new(&invocation("pytest", &[mode])).is_none(),
            "{mode}"
        );
    }
    let mut unrecognized = Collector::new(&invocation("pytest", &[])).unwrap();
    unrecognized.observe(OutputStream::Stdout, 0, b"22 tests collected in 0.04s\n");
    assert!(unrecognized.summarize(0).is_none());
}

fn go_test(args: &[&str]) -> CommandInvocation {
    CommandInvocation::Program {
        program: "go".into(),
        args: args.iter().map(|arg| (*arg).into()).collect(),
    }
}

#[test]
fn go_test_counts_packages_and_retains_both_failure_streams() {
    let mut collector = Collector::new(&go_test(&["test", "./..."])).unwrap();
    let stdout = b"ok  \texample.com/probe/alpha\t(cached)\n?   \texample.com/probe/beta\t[no test files]\n--- FAIL: TestFixture (0.00s)\n    probe_test.go:6: values differ: got 2, want 3\n";
    let stderr = b"# example.com/probe/delta [example.com/probe/delta.test]\ndelta/probe.go:3:16: cannot use \"bad type\" as int value\n";
    let last = b"FAIL\nFAIL\texample.com/probe/gamma\t0.002s\nFAIL\texample.com/probe/delta [build failed]\nFAIL\n";
    collector.observe(OutputStream::Stdout, 0, stdout);
    collector.observe(OutputStream::Stderr, stdout.len() as u64, stderr);
    collector.observe(
        OutputStream::Stdout,
        (stdout.len() + stderr.len()) as u64,
        last,
    );
    let summary = collector.summarize(1).unwrap();
    assert!(
        summary
            .text
            .starts_with("Go test packages: 1 ok, 1 no test files, 1 failed, 1 build failed.")
    );
    assert!(summary.text.contains("probe_test.go:6: values differ"));
    assert!(summary.text.contains("delta/probe.go:3:16: cannot use"));
    assert!(!summary.text.contains("probe/alpha"));
    let CommandOutputSummary::Rule { rule, ranges } = summary.source;
    assert_eq!(rule, "go-test-human");
    let merged = [stdout.as_slice(), stderr.as_slice(), last.as_slice()].concat();
    for (stream, diagnostic) in [
        (
            OutputStream::Stdout,
            b"    probe_test.go:6: values differ: got 2, want 3\n".as_slice(),
        ),
        (
            OutputStream::Stderr,
            b"delta/probe.go:3:16: cannot use \"bad type\" as int value\n".as_slice(),
        ),
    ] {
        assert!(ranges.iter().any(|range| range.stream == stream
            && merged.get(range.offset as usize..range.end_offset as usize) == Some(diagnostic)));
    }
}

#[test]
fn go_test_requires_plain_package_evidence_and_consistent_exit() {
    for args in [
        vec!["test", "-json", "./..."],
        vec!["test", "-test.v=test2json", "."],
        vec!["test", "-bench=.", "."],
        vec!["test", "-coverprofile", "cover.out"],
        vec!["test", "./...", "-args", "--reporter"],
    ] {
        assert!(Collector::new(&go_test(&args)).is_none(), "{args:?}");
    }
    for (output, code) in [
        ("ok  \texample.com/probe\t0.003s\n", 1),
        (
            "ok  \texample.com/probe\t0.003s\nFAIL\texample.com/other [setup failed]\n",
            1,
        ),
        (
            "=== RUN   TestFixture\n--- PASS: TestFixture (0.00s)\nok  \texample.com/probe\t0.003s\n",
            0,
        ),
        (
            "ok  \texample.com/probe\t0.003s\tcoverage: 2.0% of statements\n",
            0,
        ),
    ] {
        let mut collector = Collector::new(&go_test(&["test", "./..."])).unwrap();
        collector.observe(OutputStream::Stdout, 0, output.as_bytes());
        assert!(collector.summarize(code).is_none(), "{output}");
    }
    let mut collector = Collector::new(&go_test(&["test", "./..."])).unwrap();
    collector.observe(
        OutputStream::Stdout,
        0,
        b"ok  \texample.com/probe\t0.003s\n?   \texample.com/util\t[no test files]\n",
    );
    let summary = collector.summarize(0).unwrap();
    assert!(summary.text.contains("1 ok, 1 no test files"));
    assert!(summary.text.contains("Example package result:"));
    assert!(!summary.text.contains("example.com/util"));
}

#[test]
fn tsc_groups_exact_repeats_and_keeps_other_diagnostics_and_source_ranges() {
    let invocation = CommandInvocation::Program {
        program: "tsc".into(),
        args: vec!["--pretty".into(), "false".into()],
    };
    let repeated = "Type 'string' is not assignable to type 'number'.";
    let common = format!(
        "alpha.ts(1,14): error TS2322: {repeated}\nbeta.ts(2,14): error TS2322: {repeated}\n"
    );
    let other = "delta.ts(1,14): error TS2322: Type 'number' is not assignable to type 'string'.\nerror TS5058: The specified path does not exist.\n";
    let mut collector = Collector::new(&invocation).unwrap();
    collector.observe(OutputStream::Stdout, 0, common.as_bytes());
    collector.observe(OutputStream::Stdout, common.len() as u64, other.as_bytes());
    let summary = collector.summarize(2).unwrap();
    assert_eq!(summary.text.matches(repeated).count(), 1);
    assert!(summary.text.contains("TS2322 (2 occurrences)"));
    assert!(summary.text.contains("alpha.ts(1,14)\n  beta.ts(2,14)"));
    assert!(summary.text.contains("delta.ts(1,14)"));
    assert!(summary.text.contains("The specified path does not exist."));
    let CommandOutputSummary::Rule { rule, ranges } = summary.source;
    assert_eq!(rule, "tsc-diagnostics");
    assert_eq!(ranges.len(), 4);
    assert_eq!(ranges[2].offset, common.len() as u64);

    let mut unrecognized = Collector::new(&invocation).unwrap();
    unrecognized.observe(OutputStream::Stdout, 0, common.as_bytes());
    unrecognized.observe(
        OutputStream::Stdout,
        common.len() as u64,
        b"  Additional diagnostic detail.\n",
    );
    assert!(unrecognized.summarize(2).is_none());
}

#[test]
fn git_status_counts_large_captures_and_keeps_quoted_conflict_and_rename_rows() {
    let git = |args: &[&str]| CommandInvocation::Program {
        program: "git".into(),
        args: std::iter::once("status")
            .chain(args.iter().copied())
            .map(str::to_owned)
            .collect(),
    };
    let mut collector = Collector::new(&git(&["--porcelain=v1", "--branch"])).unwrap();
    let prefix = b"## main\nMM checked.txt\nUU conflict.txt\nR  \"before name\" -> \"after name\"\n D removed.txt\n?? \"a note\\nwith newline.txt\"\n!! ignored.txt\n";
    collector.observe(OutputStream::Stdout, 0, prefix);
    let mut raw = prefix.to_vec();
    for index in 0..340 {
        let row = format!("?? scratch-{index:03}\n");
        collector.observe(OutputStream::Stdout, raw.len() as u64, row.as_bytes());
        raw.extend_from_slice(row.as_bytes());
    }
    let summary = collector.summarize(0).unwrap();
    assert!(summary.text.contains("Unmerged (1)\nUU conflict.txt"));
    assert!(
        summary
            .text
            .contains("Staged and unstaged (1)\nMM checked.txt")
    );
    assert!(
        summary
            .text
            .contains("Staged only (1)\nR  \"before name\" -> \"after name\"")
    );
    assert!(
        summary
            .text
            .contains("Untracked (341)\n?? \"a note\\nwith newline.txt\"")
    );
    assert!(!summary.text.contains("scratch-200"));
    let CommandOutputSummary::Rule { rule, ranges } = summary.source;
    assert_eq!(rule, "git-status");
    assert!(ranges.iter().any(
        |range| &raw[range.offset as usize..range.end_offset as usize] == b"UU conflict.txt\n"
    ));
    for args in [
        &["--porcelain=v2"][..],
        &["--porcelain", "-z"],
        &["--", "--short"],
        &[],
    ] {
        assert!(Collector::new(&git(args)).is_none(), "{args:?}");
    }
    let mut invalid = Collector::new(&git(&["--short"])).unwrap();
    invalid.observe(OutputStream::Stdout, 0, b"?? valid\nVV unsupported\n");
    assert!(invalid.summarize(0).is_none());
}

#[tokio::test]
async fn cargo_rule_retains_assertions_and_the_original_output() {
    let dir = workspace();
    let outputs = dir.path().join("outputs");
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"summary-test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let mut tests = (0..60)
        .map(|index| format!("#[test] fn passes_{index:02}() {{}}\n"))
        .collect::<String>();
    tests.push_str("#[test] fn checks_amount() { assert_eq!(1, 2, \"amount mismatch\"); }\n");
    std::fs::write(dir.path().join("src/lib.rs"), tests).unwrap();

    let result = run_command_to(
        &serde_json::json!({
            "program": env!("CARGO"),
            "args": ["test", "--offline", "--color", "always", "--target-dir", "build"],
            "cwd": ".",
        }),
        CommandCwd::unprepared(dir.path()),
        None,
        None,
        CommandCapture::model(
            &outputs,
            "cargo-test",
            OutputLimits::default(),
            ModelOutputAccess::Script,
        ),
    )
    .await;

    let facts =
        CommandExecutionFacts::from_structured(result.structured.as_ref().unwrap()).unwrap();
    assert_eq!(facts.exit_code, Some(101), "{}", result.model_content);
    assert!(facts.capture_complete);
    assert_eq!(facts.reported_tests[0].failed, 1);
    let Some(CommandOutputSummary::Rule { rule, ranges }) = &facts.output_summary else {
        panic!(
            "expected Cargo output summary, got {}",
            result.model_content
        );
    };
    assert_eq!(rule, "cargo-humans");
    assert!(result.model_content.contains("amount mismatch"));
    assert!(
        result
            .model_content
            .replace('\\', "/")
            .contains("src/lib.rs:")
    );
    assert!(result.model_content.contains("read_output({id:"));
    assert!(
        facts
            .output_preview
            .contains("Output summary (cargo-humans")
    );
    assert!(result.model_content.len() < facts.output_ref.bytes as usize);

    let mut evidence = String::new();
    for range in ranges
        .iter()
        .filter(|range| range.stream == OutputStream::Stdout)
    {
        let retained = read_output(
            &outputs,
            &facts.output_id,
            OutputRead {
                offset: range.offset,
                limit: (range.end_offset - range.offset) as usize,
                search: None,
            },
        )
        .unwrap();
        for segment in retained
            .segments
            .iter()
            .filter(|segment| segment.stream == range.stream)
        {
            evidence.push_str(&segment.text);
        }
    }
    assert!(evidence.contains("amount mismatch"));
    let raw = read_output(&outputs, &facts.output_id, OutputRead::default()).unwrap();
    assert!(
        raw.segments
            .iter()
            .any(|segment| segment.text.contains("test passes_00 ... ok"))
    );
    assert!(
        raw.segments
            .iter()
            .any(|segment| segment.as_bytes().contains(&0x1b))
    );
}
