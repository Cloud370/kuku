import concurrent.futures
import importlib.util
import hashlib
import json
import os
from pathlib import Path, PureWindowsPath
import subprocess
import sys
import tempfile
import time
import tarfile
from types import SimpleNamespace
import unittest
from unittest import mock
import zipfile


SPEC = importlib.util.spec_from_file_location("acceptance", Path(__file__).with_name("run.py"))
ACCEPTANCE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ACCEPTANCE)


class AcceptanceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.directory = Path(self.temporary.name)

    def tearDown(self):
        self.temporary.cleanup()

    def budget(self, limit=1):
        budget = ACCEPTANCE.Budget(self.directory / "budget.json")
        budget.initialize("test-only authorization", limit, 0)
        return budget

    def test_unsettled_and_failed_runs_keep_their_full_reservation(self):
        budget = self.budget()
        budget.reserve("interrupted", 0.6, "bounded test request")
        with self.assertRaises(ValueError):
            budget.reserve("would_exceed", 0.5, "bounded test request")
        budget.settle("interrupted")
        data = json.loads(budget.path.read_text())
        self.assertEqual(budget.committed(data), 0.6)
        self.assertEqual(data["entries"][0]["basis"], "conservative_upper_bound")
        with self.assertRaises(ValueError):
            budget.settle("interrupted", 0)
        with self.assertRaises(ValueError):
            budget.reserve("interrupted", 0.1, "duplicate process")

    def test_only_evidenced_cost_releases_a_reservation(self):
        budget = self.budget()
        budget.reserve("completed", 0.6, "bounded test request")
        with self.assertRaises(ValueError):
            budget.settle("completed", 0.02, "provider_reported")
        budget.settle("completed", 0.02, "provider_reported", "test/result.json#usage.cost")
        self.assertEqual(budget.committed(json.loads(budget.path.read_text())), 0.02)
        with self.assertRaises(ValueError):
            budget.initialize("not new authorization", 100, 0)

    def test_concurrent_reservations_cannot_overcommit(self):
        budget = self.budget()

        def reserve(index):
            try:
                budget.reserve(str(index), 0.75, "bounded test request")
                return True
            except (OSError, ValueError):
                return False

        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:
            results = list(executor.map(reserve, range(2)))
        self.assertEqual(results.count(True), 1)
        self.assertEqual(budget.committed(json.loads(budget.path.read_text())), 0.75)

    def test_budget_rejects_nonfinite_or_negative_cost(self):
        for value in (float("nan"), float("inf"), -1):
            with self.assertRaises(ValueError):
                ACCEPTANCE.money(value)

    def test_live_boundary_binds_route_effort_price_and_existing_reservation(self):
        from interactive_fixture import local_request, serve
        from paid_model import RequestBoundary, RunLimits, route_for

        endpoint = {"endpoints": [{
            "tag": "example/standard", "provider_name": "Example", "name": "Example | sample/model-v2",
            "context_length": 10_000, "max_completion_tokens": 1000,
            "supported_parameters": ["tools", "reasoning_effort"],
            "pricing": {"prompt": "0.000001", "completion": "0.000002",
                        "overrides": [{"prompt": "0.000003", "input_cache_write": "0.000004"}]},
        }]}
        with mock.patch("paid_model.upstream_json", return_value=(200, endpoint)):
            route = route_for("openrouter", model="sample/model", effort="high", upstream="example/standard")
        limits = RunLimits(requests=2, request_bytes=2048, output_tokens=1000)
        self.assertEqual(str(route.reserve(limits)), "0.084000")
        larger_wire = RunLimits(requests=2, request_bytes=8192, output_tokens=1000)
        self.assertEqual(route.reserve(larger_wire), route.reserve(limits))
        self.assertEqual(route.evidence(limits)["max_bytes"], 2048)
        self.assertEqual(route.evidence(limits)["provider_context_tokens"], 10_000)
        payload = {"model": "sample/model", "stream": True, "store": False, "max_output_tokens": 1000,
                   "reasoning": {"effort": "high"}, "provider": {
                       "order": ["example/standard"], "only": ["example/standard"],
                       "allow_fallbacks": False, "require_parameters": True,
                       "max_price": {"prompt": 3, "completion": 2},
                   }}
        boundary = RequestBoundary(route, limits, "unused-key", "loopback-token")
        self.assertEqual(boundary.checked_output(payload), (1000, []))
        self.assertIsNone(boundary.checked_output({**payload, "reasoning": {"effort": "low"}}))
        self.assertIsNone(boundary.checked_output({**payload, "max_output_tokens": 1001}))
        self.assertIsNone(boundary.checked_output({**payload, "models": ["sample/other"]}))
        headers = {"Authorization": "Bearer loopback-token", "Content-Type": "application/json"}
        body = json.dumps(payload).encode()
        with mock.patch.object(boundary, "forward") as forward, serve(boundary.handler()) as address:
            self.assertEqual(local_request(address, "POST", route.path, body=body, headers=headers).status, 429)
            forward.assert_not_called()
        budget = self.budget()
        boundary.reserve(budget, "bounded", "unit fixture")
        self.assertTrue(boundary.armed)
        self.assertEqual(budget.committed(json.loads(budget.path.read_text())), 0.084)

    def test_paid_workspace_validates_selected_service_ledger(self):
        from paid_model import paid_budget

        ledger = self.budget().path
        tracker = self.directory / "benchmarks" / "rewrite-state.json"
        ACCEPTANCE.write_json(tracker, {"paid_acceptance_ledgers": {"deepseek": str(ledger),
                                                                     "openrouter": str(ledger.with_name("router.json"))}})
        with mock.patch("paid_model.ROOT", self.directory):
            self.assertEqual(paid_budget(ledger, "deepseek").path, ledger.resolve())
            with self.assertRaises(ValueError):
                paid_budget(ledger, "openrouter")

    def test_paid_workspace_passes_provider_byte_bound_separately_from_context_tokens(self):
        from paid_model import RequestBoundary, RunLimits
        from workspace import run_sdk

        caller = self.directory / "session_task"
        caller.write_bytes(b"local fixture")
        help_log = self.directory / "sdk-help.log"
        help_log.write_text("--inspect-named-value --max-request-bytes --no-provider-retry")
        args = SimpleNamespace(sdk=caller, handoff_only=False, max_requests=2, max_output_tokens=1024)
        provider = RequestBoundary(SimpleNamespace(connection="fixture"), RunLimits(2, 8192, 1024), "unused", "unused")
        check = {"log": str(help_log), "exit_code": 0, "timed_out": False}
        with mock.patch.object(provider, "reserve"), mock.patch("workspace.capture", return_value=check) as capture:
            with self.assertRaisesRegex(ValueError, "did not produce its result"):
                run_sdk(args, self.directory, self.directory, self.directory / "config.toml", {}, provider,
                        self.directory, {"run_id": "fixture"}, None)
        command = capture.call_args.args[0]
        self.assertEqual(command[command.index("--max-request-bytes") + 1], "8192")
        self.assertEqual(command[command.index("--max-context-tokens") + 1], "131072")
        self.assertIn("--no-provider-retry", command)

    def test_scratch_cleanup_uses_extended_windows_paths(self):
        from interactive_fixture import cleanup_path

        for path, expected in (
            (r"W:\accept\long\leaf", r"\\?\W:\accept\long\leaf"),
            (r"\\host\share\accept\long\leaf", r"\\?\UNC\host\share\accept\long\leaf"),
            (r"\\?\W:\accept\long\leaf", r"\\?\W:\accept\long\leaf"),
        ):
            self.assertEqual(cleanup_path(PureWindowsPath(path)), expected)

    def test_scratch_cleanup_preserves_the_execution_error(self):
        from interactive_fixture import scratch_directory

        report = {}
        with mock.patch("interactive_fixture.shutil.rmtree", side_effect=OSError("cleanup failed")):
            with self.assertRaisesRegex(ValueError, "execution failed"):
                with scratch_directory(self.directory, report, prefix="fixture-") as scratch:
                    raise ValueError("execution failed")
        self.assertIn("cleanup failed", report["scratch_cleanup_error"])
        self.assertEqual(report["scratch_path"], str(scratch))

    def test_scratch_cleanup_error_fails_a_completed_fixture(self):
        from interactive_fixture import scratch_directory

        report = {}
        with mock.patch("interactive_fixture.shutil.rmtree", side_effect=OSError("cleanup failed")):
            with self.assertRaisesRegex(OSError, "cleanup failed"):
                with scratch_directory(self.directory, report, prefix="fixture-"):
                    pass
        self.assertIn("cleanup failed", report["scratch_cleanup_error"])

    def test_workspace_restore_verifies_successful_script_reads_against_saved_reference(self):
        from workspace import saved_report_reads

        reference = {"id": "saved-output", "offset": 0, "bytes": 8}
        page = {"output_id": reference["id"], "offset": 0, "next_offset": 8,
                "complete": True, "capture_truncated": False}
        action = {"tool": "read_output", "status": "ok", "result": page}
        tool = {"name": "run_js", "status": "ok", "call_id": "script-read",
                "args": {"code": "return 1"}, "result": {"actions": [action]}}
        expected = [{"tool": "run_js", "call_id": "script-read", "reference": reference}]
        self.assertEqual(saved_report_reads([tool], reference), expected)
        tool["status"] = "error"
        self.assertEqual(saved_report_reads([tool], reference), expected)
        page["next_offset"] = 7
        self.assertEqual(saved_report_reads([tool], reference), [])
        page["next_offset"] = 8
        page["output_id"] = "another-output"
        self.assertEqual(saved_report_reads([tool], reference), [])

    def test_credentials_are_loaded_without_shell_evaluation(self):
        path = self.directory / "credentials.env"
        path.write_text("KUKU_TEST_KEY='$(do-not-run)'\n")
        self.assertEqual(ACCEPTANCE.load_secrets(path), {"KUKU_TEST_KEY": "$(do-not-run)"})
        path.write_text("OTHER_KEY=value\n")
        with self.assertRaises(ValueError):
            ACCEPTANCE.load_secrets(path)

    def test_config_evidence_hashes_without_copying_inline_credentials(self):
        path = self.directory / "config.toml"
        payload = b'api_key="private-inline-fixture-key"\n'
        path.write_bytes(payload)
        metadata = ACCEPTANCE.config_record(path)
        self.assertEqual(metadata["sha256"], hashlib.sha256(payload).hexdigest())
        self.assertNotIn("private-inline-fixture-key", json.dumps(metadata))
        self.assertEqual(list(self.directory.iterdir()), [path])

    def test_runtime_ca_bundle_is_copied_with_source_hash_and_explicit_trust_path(self):
        source = self.directory / "host-ca.pem"
        source.write_bytes(b"public CA fixture\n")
        runtime = self.directory / "runtime"
        runtime.mkdir()
        metadata = ACCEPTANCE.bundle_ca(runtime, source)
        self.assertEqual(metadata["sha256"], hashlib.sha256(source.read_bytes()).hexdigest())
        self.assertEqual(metadata["source"], str(source.resolve()))
        self.assertEqual(metadata["environment"], {"SSL_CERT_FILE": "/kuku/runtime/ca-certificates.crt"})
        self.assertEqual((runtime / "ca-certificates.crt").read_bytes(), source.read_bytes())

    def test_missing_ca_bundle_fails_before_starting_a_paid_container(self):
        with self.assertRaises(ValueError):
            ACCEPTANCE.bundle_ca(self.directory, self.directory / "missing.pem")

    def test_prompt_bundle_snapshots_only_catalog_assets(self):
        source = self.directory / "source"
        (source / "blocks").mkdir(parents=True)
        (source / "notes").mkdir()
        guidance = source / "blocks" / "tool-guidance.md"
        guidance.write_bytes(b"Plain guidance.\n")
        (source / "notes" / "scratch.md").write_bytes(b"irrelevant")
        (source / "blocks" / "scratch.txt").write_bytes(b"irrelevant")
        target = self.directory / "snapshot"
        digest = hashlib.sha256(guidance.read_bytes()).hexdigest()
        self.assertEqual(ACCEPTANCE.bundle_prompts(source, target), {str(Path("blocks") / "tool-guidance.md"): digest})
        guidance.write_bytes(b"Changed after snapshot.\n")
        self.assertEqual((target / "blocks" / "tool-guidance.md").read_bytes(), b"Plain guidance.\n")

    def test_log_is_bounded_and_redacts_secrets_across_the_retained_boundary(self):
        secret = "test-secret-" + "z" * 80
        program = ("import sys;sys.stdout.write('prefix'*20000+" + repr(secret)
                   + "+'.'*" + str(ACCEPTANCE.LOG_BYTES - 40) + ")")
        result = ACCEPTANCE.capture([sys.executable, "-c", program], self.directory, "output", secrets=(secret,))
        log = Path(result["log"]).read_text()
        self.assertTrue(result["truncated"])
        self.assertLessEqual(Path(result["log"]).stat().st_size, ACCEPTANCE.LOG_BYTES)
        self.assertNotIn("z" * 20, log)
        self.assertNotIn(secret, json.dumps(result))

    def test_capture_timeout_finishes_and_reports_failure(self):
        start = time.monotonic()
        result = ACCEPTANCE.capture([sys.executable, "-c", "import time;time.sleep(60)"],
                                    self.directory, "timeout", timeout=0.1)
        self.assertTrue(result["timed_out"])
        self.assertLess(time.monotonic() - start, 10)

    def test_log_preserves_newline_bytes_after_utf8_replacement_and_redaction(self):
        payload = "first\n\u7b2c\u4e8c\u884c\r\n".encode() + b"\xff\nprivate-test-key\n"
        program = "import sys;sys.stdout.buffer.write(" + repr(payload) + ")"
        result = ACCEPTANCE.capture([sys.executable, "-c", program], self.directory, "newlines",
                                    secrets=("private-test-key",))
        raw = Path(result["log"]).read_bytes()
        self.assertEqual(raw, "first\n\u7b2c\u4e8c\u884c\r\n\ufffd\n[REDACTED]\n".encode())
        self.assertEqual(result["retained_bytes"], len(raw))
        self.assertEqual(result["output_sha256"], hashlib.sha256(payload).hexdigest())
        self.assertEqual(result["exit_code"], 0)
        first, last = b"first\nsecond\r\n", b"\xff\nx\n"
        count = (ACCEPTANCE.LOG_BYTES - len(first) - len(last)) // 3
        payload = first + "\u754c".encode() * count + last
        program = ("import sys;sys.stdout.buffer.write(" + repr(first)
                   + "+b'\\xe7\\x95\\x8c'*" + str(count) + "+" + repr(last) + ")")
        result = ACCEPTANCE.capture([sys.executable, "-c", program], self.directory, "boundary",
                                    secrets=("x",))
        raw = Path(result["log"]).read_bytes()
        self.assertLessEqual(len(payload), ACCEPTANCE.LOG_BYTES)
        self.assertEqual(len(raw), ACCEPTANCE.LOG_BYTES)
        self.assertEqual(result["retained_bytes"], len(raw))
        self.assertTrue(result["truncated"])
        self.assertIn("\ufffd\n[REDACTED]\n", raw.decode("utf-8"))
        self.assertNotIn(b"\r\r\n", raw)
        self.assertEqual(result["output_sha256"], hashlib.sha256(payload).hexdigest())

    def test_contract_summary_keeps_early_failures_after_later_output(self):
        first = (b"test first::broken ... \x1b[31mFAILED\x1b[0m\n"
                 b"thread 'first::broken' panicked at fixture.rs:17:5:\nprivate-test-token\n")
        last = b"test last::broken ... FAILED\nerror: 2 targets failed:\n    `--test first`\n    `--test last`\n"
        payload = first + b"later success\n" * 20000 + last
        program = ("import sys;sys.stdout.buffer.write(" + repr(first)
                   + "+b'later success\\n'*20000+" + repr(last) + ");sys.exit(101)")
        result = ACCEPTANCE.capture([sys.executable, "-c", program], self.directory, "contract",
                                    secrets=("private-test-token",), retain_failures=True)
        tail = Path(result["log"]).read_text()
        summary = result["failure_summary"]
        text = Path(summary["log"]).read_text()
        self.assertEqual(result["exit_code"], 101)
        self.assertNotIn("first::broken", tail)
        for expected in ("first::broken", "fixture.rs:17:5", "last::broken", "`--test first`", "`--test last`"):
            self.assertIn(expected, text)
        self.assertIn("[REDACTED]", text)
        self.assertNotIn("private-test-token", text)
        self.assertFalse(summary["truncated"])
        self.assertEqual(result["output_sha256"], hashlib.sha256(payload).hexdigest())
        self.assertEqual(result["output_bytes"], len(payload))
        self.assertEqual(result["retained_bytes"], Path(result["log"]).stat().st_size)
        self.assertLessEqual(Path(result["log"]).stat().st_size, ACCEPTANCE.LOG_BYTES)
        self.assertLessEqual(Path(summary["log"]).stat().st_size, ACCEPTANCE.FailureSummary.LIMIT)
        success = ACCEPTANCE.capture([sys.executable, "-c", "print('all passed')"], self.directory,
                                     "contract", retain_failures=True)
        self.assertEqual(success["exit_code"], 0)
        self.assertIsNone(success["failure_summary"]["log"])
        self.assertFalse(Path(summary["log"]).exists())

    def test_failure_summary_bounds_utf8_and_overlong_unterminated_lines(self):
        summary = ACCEPTANCE.FailureSummary()
        for _ in range(100):
            summary.feed(b"x" * 8192)
            self.assertLessEqual(len(summary.partial), summary.LINE_LIMIT)
        summary.feed(b"\ntest resumed ... FAILED\n")
        for _ in range(100):
            summary.feed(("test " + "\u9519\u8bef" * 300 + " ... FAILED\n").encode())
            summary.feed(("thread 'broken' panicked at fixture.rs:1:1:\n" + "\u9519\u8bef" * 300 + "\n").encode())
        result = summary.write(self.directory, "bounded",
                               lambda text: ACCEPTANCE.redact(text, ("\u8bef",)))["failure_summary"]
        raw = Path(result["log"]).read_bytes()
        self.assertTrue(result["truncated"])
        self.assertIn("test resumed", raw.decode("utf-8"))
        self.assertNotIn("\u8bef", raw.decode("utf-8"))
        self.assertEqual(result["retained_bytes"], len(raw))
        self.assertLessEqual(len(raw), result["limit_bytes"])
        self.assertLessEqual(len(summary.summaries) + len(summary.details), 2 * summary.capacity)

    @unittest.skipIf(os.name == "nt", "POSIX process-group orphan cleanup")
    def test_capture_cleans_a_child_holding_an_exited_parents_pipe(self):
        program = ("import subprocess,sys;subprocess.Popen([sys.executable,'-c',"
                   "'import time;time.sleep(60)']);print('parent completed')")
        start = time.monotonic()
        result = ACCEPTANCE.capture([sys.executable, "-c", program], self.directory, "orphan", timeout=5)
        self.assertFalse(result.get("capture_incomplete", False))
        self.assertLess(time.monotonic() - start, 10)

    def test_zero_reward_without_collected_tests_is_infrastructure_failure(self):
        execution = {"exit_code": 0}
        verification = {"exit_code": 0}
        self.assertFalse(ACCEPTANCE.attempt_passed("negative", execution, verification, 0, None))
        self.assertFalse(ACCEPTANCE.attempt_passed("negative", execution, verification, 0, {"tests": 0}))
        self.assertTrue(ACCEPTANCE.attempt_passed("negative", execution, verification, 0, {"tests": 6, "failed": 6}))
        self.assertFalse(ACCEPTANCE.attempt_passed("live", execution, verification, 1, {"tests": 6, "failed": 1}))
        complete = {"tests": 6, "failed": 0, "passed": 6, "skipped": 0, "pending": 0, "other": 0}
        self.assertTrue(ACCEPTANCE.attempt_passed("live", execution, verification, 1, complete))
        for field in ("failed", "skipped", "pending", "other"):
            incomplete = dict(complete, **{field: 1})
            self.assertFalse(ACCEPTANCE.attempt_passed("live", execution, verification, 1, incomplete))
        self.assertFalse(ACCEPTANCE.attempt_passed("live", execution, verification, 1, dict(complete, passed=1)))
        self.assertFalse(ACCEPTANCE.attempt_passed("live", execution, verification, 1, {"tests": 1, "passed": 1}))

    def test_frontier_selection_keeps_legacy_cases_and_pins_public_source(self):
        manifest, case = ACCEPTANCE.manifest_case("large-scale-text-editing", "frontier")
        self.assertEqual([item["id"] for item in manifest["cases"]], [
            "log-summary-date-ranges", "cancel-async-tasks", "regex-log", "large-scale-text-editing",
            "nginx-request-logging", "sqlite-with-gcov", "kv-store-grpc", "gcode-to-text",
        ])
        self.assertEqual(case["artifacts"], ["/app/apply_macros.vim"])
        _, gcode = ACCEPTANCE.manifest_case("gcode-to-text", "frontier")
        self.assertEqual((gcode["workspace"], gcode["artifacts"], gcode["timeout_sec"]),
                         ("/app", ["/app/out.txt"], 900))
        legacy, legacy_case = ACCEPTANCE.manifest_case("cancel-async-tasks")
        frontier, frontier_case = ACCEPTANCE.manifest_case("cancel-async-tasks", "frontier")
        self.assertEqual(legacy_case, frontier_case)
        self.assertNotEqual(legacy["source"]["revision"], frontier["source"]["revision"])
        for case_id, suite in (("large-scale-text-editing", "legacy"), ("openssl-selfsigned-cert", "frontier")):
            with self.assertRaisesRegex(ValueError, "unknown benchmark case"):
                ACCEPTANCE.manifest_case(case_id, suite)

    def test_benchmark_parser_selects_frontier_route_and_preserves_legacy_default(self):
        parser = ACCEPTANCE.parser()
        base = ["benchmark", "--case", "regex-log"]
        self.assertEqual(parser.parse_args(base).suite, "legacy")
        for flags, expected in (
            ([], ("openrouter", None)),
            (["--upstream", "provider/variant"], ("openrouter", "provider/variant")),
            (["--provider", "deepseek"], ("deepseek", None)),
        ):
            args = parser.parse_args([*base, "--suite", "frontier", "--mode", "live", *flags])
            self.assertEqual((args.provider, args.upstream), expected)
            self.assertIsNone(args.max_requests)
            self.assertIsNone(args.model)
            self.assertIsNone(args.reasoning_effort)
        for limit in (22, 48):
            args = parser.parse_args([*base, "--suite", "frontier", "--max-requests", str(limit)])
            self.assertEqual(args.max_requests, limit)

    def test_frontier_sources_are_independently_pinned_and_exclude_solution(self):
        from benchmark_source import prepare_source

        upstream = self.directory / "upstream"
        upstream.mkdir()

        def git(*args):
            return subprocess.check_output(["git", "-C", str(upstream), *args], text=True).strip()

        git("init", "--quiet")
        git("config", "uploadpack.allowFilter", "true")
        for case_id in ("one", "two"):
            for name in ("instruction.md", "task.toml", "environment/Dockerfile", "tests/test.sh", "solution/solve.sh"):
                path = upstream / "tasks" / case_id / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(name)
        git("add", ".")
        git("-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "--quiet", "-m", "benchmark")
        spec = {"repository": upstream.as_uri(), "revision": git("rev-parse", "HEAD"),
                "cache_key": "frontier-fixture", "task_root": "tasks"}

        def prepare(case_id, attempt=0):
            logs = self.directory / "logs" / f"{case_id}-{attempt}"
            return prepare_source(self.directory, {"source": spec}, logs, {"id": case_id})

        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
            futures = [executor.submit(prepare, case_id, attempt) for case_id in ("one", "two") for attempt in (1, 2)]
            first, first_peer, second, second_peer = [future.result() for future in futures]
        self.assertEqual(first, first_peer)
        self.assertEqual(second, second_peer)
        self.assertNotEqual(first, second)
        self.assertFalse((first / "tasks" / "two").exists())
        self.assertFalse((first / "tasks" / "one" / "solution").exists())
        self.assertTrue((second / "tasks" / "two" / "tests" / "test.sh").is_file())
        with ACCEPTANCE.locked(first.with_suffix(".lock")):
            self.assertEqual(prepare("one"), first)
        (first / "tasks" / "one" / "instruction.md").write_text("edited")
        with self.assertRaisesRegex(ValueError, "benchmark source is modified"):
            prepare("one")
        self.assertEqual(prepare("two"), second)

    def test_frontier_main_uses_host_meter_without_legacy_budget_or_config(self):
        fake = mock.Mock(return_value={"passed": True})
        creds = {"KUKU_TEST_KEY": "fixture"}
        image = "sha256:fixture-image"
        args = ["run.py", "--output", str(self.directory), "benchmark", "--suite", "frontier",
                "--mode", "live", "--case", "large-scale-text-editing", "--sdk-binary", "/fixture/sdk",
                "--provider", "deepseek", "--model", "deepseek-flash", "--reasoning-effort", "high"]

        def capture(command, directory, label, **kwargs):
            if label == "image-build":
                Path(command[command.index("--iidfile") + 1]).write_text(image)
            output = directory / (label + ".log")
            output.write_text(image)
            return {"log": str(output), "exit_code": 0, "timed_out": False}

        with mock.patch.object(sys, "argv", args), mock.patch.dict(sys.modules, {"frontier": mock.Mock(run_live=fake)}), \
                mock.patch.object(ACCEPTANCE, "load_secrets", return_value=creds), \
                mock.patch.object(ACCEPTANCE, "prepare_source", return_value=self.directory / "source"), \
                mock.patch.object(ACCEPTANCE, "tree_state", return_value={}), \
                mock.patch.object(ACCEPTANCE, "capture", side_effect=capture), \
                mock.patch.object(ACCEPTANCE, "Budget") as budget, mock.patch("builtins.print"):
            self.assertEqual(ACCEPTANCE.main(), 0)
        budget.assert_not_called()
        parsed, case, source, built, directory, secrets = fake.call_args.args
        self.assertEqual(parsed.run_id, directory.name)
        self.assertEqual((parsed.provider, parsed.model, parsed.upstream), ("deepseek", "deepseek-flash", None))
        self.assertEqual(parsed.reasoning_effort, "high")
        self.assertEqual((parsed.case_cap_usd, parsed.max_output_tokens, parsed.max_requests), (0.75, None, None))
        self.assertEqual(parsed.agent_timeout_sec, 1200)
        self.assertEqual(source, self.directory / "source" / "tasks" / case["id"])
        self.assertEqual((built, secrets), (image, creds))

    def test_benchmark_model_and_effort_overrides_require_live_direct_deepseek(self):
        base = ["run.py", "benchmark", "--case", "regex-log"]
        for flags in (
            ["--suite", "frontier", "--provider", "deepseek"],
            ["--suite", "frontier", "--mode", "live", "--provider", "openrouter"],
            ["--mode", "live", "--provider", "deepseek"],
        ):
            for option in (("--model", "deepseek-v4-pro"), ("--reasoning-effort", "high")):
                with self.subTest(flags=flags, option=option), \
                        mock.patch.object(sys, "argv", [*base, *flags, *option]):
                    with self.assertRaisesRegex(ValueError, f"{option[0]} selects a direct DeepSeek"):
                        ACCEPTANCE.main()

    def test_benchmark_deadline_requires_live_sdk_and_positive_seconds(self):
        base = ["run.py", "benchmark", "--suite", "frontier", "--case", "regex-log"]
        for context, seconds, error in (
            (["--sdk-binary", "/fixture/sdk"], "60", "requires a live Frontier SDK runner"),
            (["--mode", "live", "--sdk-binary", "/fixture/sdk"], "0", "positive 32-bit integer"),
            (["--mode", "live", "--sdk-binary", "/fixture/sdk"], "4294967296", "positive 32-bit integer"),
        ):
            with self.subTest(context=context, seconds=seconds), \
                    mock.patch.object(sys, "argv", [*base, *context, "--max-command-seconds", seconds]):
                with self.assertRaisesRegex(ValueError, error):
                    ACCEPTANCE.main()

    def test_legacy_paid_benchmark_keeps_finite_request_and_output_limits_and_disables_retries(self):
        config = self.directory / "config.toml"
        config.write_text("# local fixture\n")
        base = ["run.py", "--output", str(self.directory), "benchmark", "--mode", "live", "--case", "regex-log",
                "--sdk-binary", "/fixture/sdk", "--config", str(config), "--connection", "fixture",
                "--reserve-usd", "0.5", "--reservation-reason", "local fixture"]

        def benchmark(_args, _directory, record, _secrets):
            record["passed"] = True

        for requests, output, override in (("12", "32768", False), ("22", "16384", True)):
            limits = ["--max-requests", requests, "--max-output-tokens", output]
            tail = ["--", *limits] if override else []
            with mock.patch.object(sys, "argv", [*base, *tail]), \
                    mock.patch.object(ACCEPTANCE, "load_secrets", return_value={"KUKU_TEST_KEY": "fixture"}), \
                    mock.patch.object(ACCEPTANCE, "tree_state", return_value={}), \
                    mock.patch.object(ACCEPTANCE, "Budget"), \
                    mock.patch.object(ACCEPTANCE, "run_benchmark", side_effect=benchmark) as run, mock.patch("builtins.print"):
                self.assertEqual(ACCEPTANCE.main(), 0)
            self.assertEqual(run.call_args.args[0].sdk_args, [*limits, "--no-provider-retry"])

    def test_grader_cannot_reuse_a_report_left_by_the_agent(self):
        logs = self.directory / "logs"
        (logs / "verifier").mkdir(parents=True)
        (logs / "verifier" / "reward.txt").write_text("1")
        (logs / "verifier" / "ctrf.json").write_text('{"results":{"summary":{"tests":1,"passed":1}}}')
        ACCEPTANCE.fresh_verifier_directory(logs)
        self.assertEqual(list((logs / "verifier").iterdir()), [])
        self.assertIsNone(ACCEPTANCE.verifier_summary(logs))
        self.assertIsNone(ACCEPTANCE.reward(logs))

    def test_contract_selects_feature_gated_tests(self):
        command = ACCEPTANCE.selected_commands("contract")[0]
        self.assertIn("--workspace", command)
        self.assertIn("--no-fail-fast", command)
        self.assertIn("kuku/test_support", command)
        self.assertIn("kuku/test_support,kuku-app/embedded-web-assets",
                      ACCEPTANCE.selected_commands("contract", embedded_web_assets=True)[0])
        self.assertEqual(ACCEPTANCE.selected_commands("integration")[-1], command)
        for suite in ACCEPTANCE.PROTOCOL_TESTS:
            self.assertIn(suite, ACCEPTANCE.selected_commands("protocol")[0])
        with mock.patch.object(ACCEPTANCE.os, "name", "nt"):
            self.assertEqual(ACCEPTANCE.selected_commands("contract")[0][-2:],
                             ["--", "--test-threads=1"])
        with mock.patch.object(ACCEPTANCE.os, "name", "posix"):
            self.assertNotIn("--test-threads=1", ACCEPTANCE.selected_commands("contract")[0])

    def test_contract_failure_is_not_hidden_after_all_test_targets_run(self):
        args = ["run.py", "--output", str(self.directory), "--run-id", "failed-contract", "contract"]
        failed = {"exit_code": 101, "timed_out": False}
        with mock.patch.object(sys, "argv", args), \
                mock.patch.object(ACCEPTANCE, "tree_state", return_value={}), \
                mock.patch.object(ACCEPTANCE, "capture", return_value=failed) as capture, \
                mock.patch("builtins.print"):
            self.assertEqual(ACCEPTANCE.main(), 1)
        self.assertIn("--no-fail-fast", capture.call_args.args[0])
        self.assertEqual(capture.call_args.kwargs["timeout"], 1800)
        self.assertTrue(capture.call_args.kwargs["retain_failures"])
        report = json.loads((self.directory / "failed-contract" / "result.json").read_text())
        self.assertFalse(report["passed"])
        self.assertEqual(report["checks"], [failed])

    def test_cargo_checks_cannot_inherit_another_worktrees_target_directory(self):
        with mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": "/shared-target"}):
            env = ACCEPTANCE.command_environment()
            self.assertEqual(env["CARGO_TARGET_DIR"], str(ACCEPTANCE.ROOT / "target"))
            self.assertEqual(os.environ["CARGO_TARGET_DIR"], "/shared-target")

    def test_workspace_cargo_output_preserves_the_native_helper_identity(self):
        profile = {"opt_level": "0", "debuginfo": 2, "test": True}
        helper = {"reason": "compiler-artifact", "executable": "/build/native-helper",
                  "target": {"name": "interactive_pty", "kind": ["test"]},
                  "profile": profile, "features": ["default"], "fresh": True}
        application = {"reason": "compiler-artifact", "executable": "/build/kuku",
                       "target": {"name": "kuku", "kind": ["bin"]}}
        log = self.directory / "cargo.log"
        log.write_text("\n".join(json.dumps(item) for item in (helper, application)))
        self.assertEqual(ACCEPTANCE.pty_artifact(log), Path(helper["executable"]))
        self.assertEqual(ACCEPTANCE.pty_build(log), helper)

    def test_cargo_binary_cannot_be_used_as_the_native_helper(self):
        log = self.directory / "cargo.log"
        log.write_text(json.dumps({"reason": "compiler-artifact", "executable": "/build/decoy",
                                  "target": {"name": "interactive_pty", "kind": ["bin"]}}))
        with self.assertRaisesRegex(ValueError, "native PTY test helper"):
            ACCEPTANCE.pty_artifact(log)

    def test_release_archives_only_extract_the_binary(self):
        source = self.directory / "source"
        source.write_bytes(b"installed executable")
        tar = self.directory / "release.tar.gz"
        zip_path = self.directory / "release.zip"
        with tarfile.open(tar, "w:gz") as package:
            package.add(source, arcname="kuku")
            package.add(source, arcname="other")
        with zipfile.ZipFile(zip_path, "w") as package:
            package.write(source, arcname="kuku.exe")
            package.write(source, arcname="other")
        for archive, name in ((tar, "kuku"), (zip_path, "kuku.exe")):
            binary = self.directory / name
            ACCEPTANCE.extract_release_binary(archive, binary)
            self.assertEqual(source.read_bytes(), binary.read_bytes())
            self.assertFalse((self.directory / "other").exists())

    @unittest.skipUnless(sys.platform.startswith("linux"), "Linux /proc resource selector")
    def test_process_samples_keep_cpu_and_memory_when_io_counters_are_unavailable(self):
        from metrics import process_sample
        sample = process_sample(os.getpid())
        self.assertIsNotNone(sample)
        self.assertGreater(sample["rss_kib"], 0)
        self.assertGreater(sample["threads"], 0)
        self.assertGreaterEqual(sample["cpu_s"], 0)
        self.assertIn("write_bytes", sample)


if __name__ == "__main__":
    unittest.main()
