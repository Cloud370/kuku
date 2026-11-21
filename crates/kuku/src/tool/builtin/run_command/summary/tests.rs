use crate::execution::{
    CommandExecutionFacts, CommandInvocation, CommandOutputSummary, OutputStream,
};

use super::super::super::test_helpers::workspace;
use super::super::output::{read_output, OutputLimits, OutputRead};
use super::super::{run_command_to, CommandCapture, ModelOutputAccess};
use super::Collector;

fn cargo(args: &[&str]) -> CommandInvocation {
    CommandInvocation::Program {
        program: "cargo".into(),
        args: args.iter().map(|arg| (*arg).into()).collect(),
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
    assert!(summary
        .text
        .contains("error[E0308]: mismatched types\n  --> src/lib.rs:8:1"));
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
    assert!(ranges
        .iter()
        .any(|range| range.stream == OutputStream::Stdout && range.offset == location));

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
        dir.path(),
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
    assert!(result.model_content.contains("src/lib.rs:"));
    assert!(result.model_content.contains("read_output({id:"));
    assert!(facts
        .output_preview
        .contains("Output summary (cargo-humans"));
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
    assert!(raw
        .segments
        .iter()
        .any(|segment| segment.text.contains("test passes_00 ... ok")));
    assert!(raw
        .segments
        .iter()
        .any(|segment| segment.as_bytes().contains(&0x1b)));
}
