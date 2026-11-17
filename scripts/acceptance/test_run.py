import concurrent.futures
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest


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

    def test_credentials_are_loaded_without_shell_evaluation(self):
        path = self.directory / "credentials.env"
        path.write_text("KUKU_TEST_KEY='$(do-not-run)'\n")
        self.assertEqual(ACCEPTANCE.load_secrets(path), {"KUKU_TEST_KEY": "$(do-not-run)"})
        path.write_text("OTHER_KEY=value\n")
        with self.assertRaises(ValueError):
            ACCEPTANCE.load_secrets(path)

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
        self.assertTrue(ACCEPTANCE.attempt_passed("live", execution, verification, 1, {"tests": 6, "failed": 0, "passed": 6}))

    def test_contract_selects_feature_gated_tests(self):
        command = ACCEPTANCE.selected_commands("contract")[0]
        self.assertIn("kuku/test_support", command)
        for suite in ACCEPTANCE.PROTOCOL_TESTS:
            self.assertIn(suite, ACCEPTANCE.selected_commands("protocol")[0])


if __name__ == "__main__":
    unittest.main()
