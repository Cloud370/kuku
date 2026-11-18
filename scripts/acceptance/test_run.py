import concurrent.futures
import importlib.util
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import tarfile
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

    def test_workspace_cleanup_keeps_the_execution_error(self):
        from workspace import workspace_scratch

        report = {}
        with mock.patch("workspace.shutil.rmtree", side_effect=OSError("cleanup failed")):
            with self.assertRaisesRegex(ValueError, "execution failed"):
                with workspace_scratch(self.directory, report):
                    raise ValueError("execution failed")
        self.assertIn("cleanup failed", report["scratch_cleanup_error"])

    def test_workspace_restore_verifies_successful_script_reads_against_saved_reference(self):
        from workspace import saved_report_reads

        reference = {"id": "saved-output", "offset": 0, "bytes": 8}
        page = {"output_id": reference["id"], "offset": 0, "next_offset": 8,
                "complete": True, "capture_truncated": False}
        action = {"tool": "read_output", "status": "ok", "result": page}
        tool = {"name": "workspace_script", "status": "ok", "call_id": "script-read",
                "args": {"code": "return 1"}, "result": {"actions": [action]}}
        expected = [{"tool": "workspace_script", "call_id": "script-read", "reference": reference}]
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
        self.assertIn("kuku/test_support", command)
        for suite in ACCEPTANCE.PROTOCOL_TESTS:
            self.assertIn(suite, ACCEPTANCE.selected_commands("protocol")[0])

    def test_cargo_checks_cannot_inherit_another_worktrees_target_directory(self):
        with mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": "/shared-target"}):
            env = ACCEPTANCE.command_environment()
            self.assertEqual(env["CARGO_TARGET_DIR"], str(ACCEPTANCE.ROOT / "target"))
            self.assertEqual(os.environ["CARGO_TARGET_DIR"], "/shared-target")

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
