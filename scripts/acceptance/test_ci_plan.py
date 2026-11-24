from unittest import TestCase
from unittest.mock import patch

from ci_plan import changed_paths, plan


class CiPlanTests(TestCase):
    def test_normal_sdk_pr_needs_neither_installed_program_nor_web_build(self):
        jobs, tooling, verify_web = plan("pull_request", paths=("crates/kuku/src/tool/run_js/recovery.rs",))
        self.assertEqual(["ubuntu-latest"], tooling)
        self.assertFalse(verify_web)
        self.assertEqual(1, len(jobs))
        self.assertEqual("workspace", jobs[0]["contract"])
        self.assertEqual("none", jobs[0]["installed"])
        self.assertFalse(jobs[0]["web"])

    def test_terminal_pr_selects_terminal_cases_on_linux_without_web(self):
        jobs, _, _ = plan("pull_request", paths=("crates/kuku-tui/src/interaction.rs",))
        self.assertEqual(1, len(jobs))
        self.assertEqual("selected", jobs[0]["installed"])
        cases = jobs[0]["cases"].split(",")
        self.assertIn("keyboard-queue", cases)
        self.assertIn("statusline", cases)
        self.assertFalse(jobs[0]["web"])

    def test_web_pr_runs_browser_only_and_checks_frontend_in_the_web_build(self):
        jobs, _, verify_web = plan("pull_request", paths=("apps/web/src/main.ts",))
        self.assertTrue(verify_web)
        core, browser = jobs
        self.assertEqual("workspace", core["contract"])
        self.assertEqual("none", core["installed"])
        self.assertFalse(core["web"])
        self.assertTrue(browser["web"])
        self.assertTrue(browser["browser"])
        self.assertEqual("none", browser["contract"])
        self.assertEqual("browser", browser["cases"])
        self.assertNotEqual(core["artifact"], browser["artifact"])

    def test_session_http_pr_selects_browser_and_session_cases_once(self):
        jobs, _, verify_web = plan("pull_request",
                                   paths=("crates/kuku-server/src/routes/events.rs",))
        core, browser = jobs
        cases = core["cases"].split(",")
        self.assertIn("rewind-history", cases)
        self.assertNotIn("browser", cases)
        self.assertEqual(len(set(cases)), len(cases))
        self.assertFalse(core["web"])
        self.assertEqual("browser", browser["cases"])
        self.assertFalse(verify_web)

    def test_ci_changes_verify_independent_platform_and_browser_jobs(self):
        jobs, tooling, _ = plan("pull_request", paths=(".github/workflows/ci-platform.yml",))
        self.assertEqual(["ubuntu-latest", "macos-latest", "windows-latest"], tooling)
        core, browser, macos, windows = jobs
        self.assertEqual("workspace", core["contract"])
        self.assertTrue(browser["web"])
        self.assertFalse(core["web"])
        for native in (macos, windows):
            self.assertEqual("sdk", native["contract"])
            self.assertEqual("smoke", native["installed"])
            self.assertFalse(native["web"])

    def test_shared_file_lock_changes_run_native_sdk_checks_without_installing(self):
        jobs, _, _ = plan("pull_request", paths=("crates/kuku/src/util/fs_sync.rs",))
        self.assertEqual(["ubuntu-latest", "macos-latest", "windows-latest"],
                         [entry["os"] for entry in jobs])
        for entry in jobs[1:]:
            self.assertEqual("sdk", entry["contract"])
            self.assertEqual("none", entry["installed"])
            self.assertFalse(entry["web"])

    def test_windows_terminal_change_only_adds_windows_native_smoke(self):
        jobs, _, _ = plan("pull_request", paths=("crates/kuku-tui/src/terminal/windows.rs",))
        self.assertEqual(["ubuntu-latest", "windows-latest"], [entry["os"] for entry in jobs])
        self.assertEqual("smoke", jobs[1]["installed"])

    def test_draft_and_main_push_keep_the_fast_gate(self):
        for event, draft in (("push", False), ("pull_request", True)):
            with self.subTest(event=event):
                jobs, tooling, verify_web = plan(event, draft, paths=("apps/web/src/main.ts",))
                self.assertEqual(["ubuntu-latest"], tooling)
                self.assertEqual("none", jobs[0]["installed"])
                self.assertFalse(verify_web)

    def test_weekly_and_manual_platform_keep_full_checks(self):
        jobs, tooling, _ = plan("schedule")
        self.assertEqual(3, len(jobs))
        self.assertEqual(3, len(tooling))
        self.assertTrue(all(entry["installed"] == "full" and entry["web"] for entry in jobs))
        jobs, tooling, verify_web = plan("workflow_dispatch", platform="windows")
        self.assertEqual(["windows-latest"], tooling)
        self.assertEqual("workspace", jobs[0]["contract"])
        self.assertEqual("full", jobs[0]["installed"])
        self.assertFalse(jobs[0]["browser"])
        self.assertFalse(verify_web)

    def test_manual_windows_desktop_targets_only_the_graphical_client(self):
        jobs, tooling, verify_web = plan("workflow_dispatch", platform="windows-desktop")
        self.assertEqual(["windows-latest"], tooling)
        windows, = jobs
        self.assertEqual("windows-latest", windows["os"])
        self.assertEqual("none", windows["contract"])
        self.assertEqual("desktop", windows["installed"])
        self.assertFalse(windows["web"])
        self.assertFalse(verify_web)

    def test_release_parallelizes_native_contracts_with_archive_builds(self):
        jobs, tooling, verify_web = plan("release")
        self.assertEqual(["ubuntu-latest", "macos-latest", "windows-latest"], tooling)
        self.assertEqual(tooling, [entry["os"] for entry in jobs])
        self.assertTrue(all(entry["contract"] == "workspace" for entry in jobs))
        self.assertTrue(all(entry["installed"] == "none" and not entry["web"] for entry in jobs))
        self.assertFalse(verify_web)

    @patch("ci_plan.subprocess.check_output", return_value=b"old\nname\0new name\0")
    def test_changed_paths_use_merge_tree_and_preserve_both_rename_paths(self, git):
        self.assertEqual(("old\nname", "new name"), changed_paths())
        git.assert_called_once_with(["git", "diff", "--name-only", "--no-renames", "-z", "HEAD^1", "HEAD"])
