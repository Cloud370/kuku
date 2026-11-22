import contextlib
import io
import json
import unittest
from unittest.mock import MagicMock, Mock, patch

from interactive import wait_restart_and_recover_crash
from interactive_fixture import InstalledClient, ProviderCall, Terminal, TOOL_OUTPUT, run_cases
from interactive_rendering import (
    click_queued_input,
    has_selected_queued_input,
    has_standalone_assistant_rail,
    page_up_after_rendered_resize,
    queued_input_click_context,
    queued_input_click_position,
    rendered_running_transcript,
    select_message_action,
)


class AcceptanceCaseTests(unittest.TestCase):
    def test_failed_case_does_not_hide_later_cases_or_turn_into_success(self):
        calls = []

        def failure():
            calls.append("failed")
            raise ValueError("original failure")

        with patch("sys.stderr") as stderr, self.assertRaisesRegex(AssertionError, "failed case"):
            run_cases([
                ("failed case", failure),
                ("later case", lambda: calls.append("later")),
            ])

        self.assertEqual(["failed", "later"], calls)
        self.assertIn("original failure", "".join(str(call) for call in stderr.write.call_args_list))

    def test_failed_frame_survives_later_output_and_nested_case_collection(self):
        frame = "original frame\n" + "终端内容\n" * 100
        output = io.StringIO()

        def failure():
            raise AssertionError(frame)

        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            with self.assertRaises(AssertionError) as error:
                run_cases([
                    ("nested", lambda: run_cases([("restore", failure)])),
                    ("later", lambda: print("later output" * 10000)),
                ])
            print(error.exception)

        self.assertIn(frame, output.getvalue().encode()[-65536:].decode())
        self.assertIn("nested", str(error.exception))
        self.assertIn("restore", str(error.exception))

    def test_failure_details_are_bounded_without_losing_failed_case_names(self):
        def failure():
            raise ValueError("界" * 10000)

        with patch("sys.stderr"), patch("sys.stdout"), self.assertRaises(AssertionError) as error:
            run_cases([("first", failure), ("second", failure)])

        message = str(error.exception)
        self.assertIn("first, second", message)
        self.assertIn("retained byte limit", message)
        self.assertNotIn("\ufffd", message)
        self.assertLess(len(message.encode()), 17000)


class ProviderToolResultTests(unittest.TestCase):
    def call(self, result):
        body = json.dumps({"messages": [{"role": "user", "content": [result]}]})
        return ProviderCall(body, None, 1)

    def test_success_requires_the_exact_tool_result_and_output(self):
        call = self.call({"type": "tool_result", "tool_use_id": "fixture", "content": "proof"})
        self.assertIs(call, call.expect_tool_output("fixture", "proof"))
        with self.assertRaisesRegex(AssertionError, "Expected one tool result"):
            call.expect_tool_output("other", "proof")
        with self.assertRaisesRegex(AssertionError, "did not return"):
            call.expect_tool_output("fixture", "missing")

    def test_error_cannot_be_reported_as_success_even_when_output_is_present(self):
        call = self.call({"type": "tool_result", "tool_use_id": "fixture", "is_error": True,
                          "content": "proof followed by capture failure"})
        with self.assertRaisesRegex(AssertionError, "Tool fixture failed"):
            call.expect_tool_output("fixture", "proof")

    def job_client(self, exit_code=0):
        client = object.__new__(InstalledClient)
        handle = self.call({"type": "tool_result", "tool_use_id": "fixture",
                            "content": json.dumps({"kind": "job", "job_id": "job-1"})})
        handle.tool = Mock()
        waited = self.call({"type": "tool_result", "tool_use_id": "fixture-wait", "content": TOOL_OUTPUT})
        client.provider = Mock()
        client.provider.next.side_effect = [handle, waited]
        client.session = Mock(return_value="session")
        client.request = Mock()
        client.request.return_value.as_json.return_value = {
            "id": "job-1", "tool_call_id": "fixture", "status": "completed",
            "result": {"status": "ok", "structured": {"exit_code": exit_code,
                "capture_complete": True, "cleanup_incomplete": False, "output_preview": TOOL_OUTPUT}},
        }
        return client, handle, waited

    def test_background_handle_requires_wait_and_successful_durable_command_facts(self):
        client, handle, waited = self.job_client()
        self.assertIs(waited, client.completed_tool("workspace", "fixture"))
        handle.tool.assert_called_once_with("wait_job", "fixture-wait", job_id="job-1")
        self.assertEqual("job-1", waited.completed_job["id"])

    def test_background_output_marker_cannot_hide_a_failed_command(self):
        client, _, _ = self.job_client(exit_code=1)
        with self.assertRaises(AssertionError):
            client.completed_tool("workspace", "fixture")


class PendingQuestionRestartTests(unittest.TestCase):
    def test_visible_question_waits_for_settlement_before_restart(self):
        client = MagicMock()
        question = {"id": "pending-question", "text": "Which durable answer should I use?"}
        client.snapshot.side_effect = [
            {"status": "running", "question": question},
            {"status": "waiting", "question": question},
        ]

        class RestartObserved(Exception):
            pass

        def restart(*args):
            self.assertEqual(2, client.snapshot.call_count)
            raise RestartObserved

        client.cli.side_effect = restart
        with self.assertRaises(RestartObserved):
            wait_restart_and_recover_crash(client, "workspace", "session", "title")

        client.cli.assert_called_once_with("workspace", "server", "restart")
        client.provider.next.assert_called_once_with("persist a pending question")

    def test_unsettled_question_times_out_without_restarting_or_resubmitting(self):
        client = MagicMock()
        client.snapshot.return_value = {"status": "running", "question": None}
        with (
            patch("interactive_fixture.time.monotonic", side_effect=[0, 31]),
            self.assertRaisesRegex(AssertionError, "pending question to settle"),
        ):
            wait_restart_and_recover_crash(client, "workspace", "session", "title")

        client.cli.assert_not_called()
        client.provider.next.assert_called_once_with("persist a pending question")


class MixedTableScrollTests(unittest.TestCase):
    def test_ignores_user_card_gutter_above_assistant(self):
        frame = [" │", "", " Assistant", " Before table 00", " After table 00"]

        self.assertFalse(has_standalone_assistant_rail(frame))

    def test_detects_orphan_rail_in_assistant_body(self):
        frame = ["", " Assistant", " Before table 00", " │", " After table 00"]

        self.assertTrue(has_standalone_assistant_rail(frame))

    def test_scans_table_frame_without_assistant_heading(self):
        frame = [" Before table 00", " After table 00"]

        self.assertFalse(has_standalone_assistant_rail(frame))

    def test_detects_orphan_rail_without_assistant_heading(self):
        frame = [" Before table 00", " │", " After table 00"]

        self.assertTrue(has_standalone_assistant_rail(frame))


class QueuedInputFocusTests(unittest.TestCase):
    def test_terminal_tracks_resize_dimensions_in_screen_snapshots(self):
        with (
            patch("interactive_fixture.subprocess.Popen"),
            patch("interactive_fixture.threading.Thread"),
        ):
            terminal = Terminal("bridge", "binary", "workspace", {})

        self.assertEqual((32, 140), (terminal.rows, terminal.cols))
        terminal.receive = Mock(side_effect=[
            {"rows": 40, "cols": 120},
            {"screen": "full screen frame"},
        ])

        terminal.request("resize", rows=40, cols=120)
        snapshot = terminal.request("screen")

        self.assertEqual(
            {"screen": "full screen frame", "rows": 40, "cols": 120},
            snapshot,
        )
        self.assertEqual(2, terminal.receive.call_count)

    def test_click_queued_input_returns_and_sends_target_coordinates(self):
        screen = "\n".join([
            "Pending inputs · ↑ select · Enter edit · Del withdraw",
            "  You · Queued · next Turn",
            "  Process this after the checkpoint report",
            "  You · Queued · next Turn",
            "  Obsolete checkpoint follow-up",
            "│ ↓  Reply",
        ])
        terminal = Mock()

        self.assertEqual(
            (4, 3),
            click_queued_input(terminal, screen, "Obsolete checkpoint follow-up"),
        )
        terminal.press.assert_called_once_with("\x1b[<0;3;4M\x1b[<0;3;4m")

    def test_click_queued_input_reports_when_no_coordinates_were_sent(self):
        terminal = Mock()

        with self.assertRaisesRegex(
            AssertionError,
            "target row unavailable; no click coordinates were sent",
        ):
            click_queued_input(terminal, "│ ⢎⡱ Ready Draft", "missing")

        terminal.press.assert_not_called()

    def test_click_failure_context_includes_original_screen_and_dimensions(self):
        screen = "\n".join([
            "Transcript context above the pending inputs",
            "Pending inputs · ↑ select · Enter edit · Del withdraw",
            "  You · Queued · next Turn",
            "  Obsolete checkpoint follow-up",
            "Composer draft below the pending inputs",
        ])
        snapshot = {"screen": screen, "rows": 32, "cols": 140}

        self.assertEqual(
            "Queued input target row: 24\n"
            "Mouse click coordinates (column, row; 1-based): (6, 24)\n"
            "Terminal size: 32 rows × 140 columns\n"
            "Full screen used to compute click coordinates:\n"
            f"{screen}",
            queued_input_click_context(24, 6, snapshot),
        )

    def test_detects_the_selected_queued_input(self):
        screen = "\n".join([
            "Pending inputs · ↑ select · Enter edit · Del withdraw",
            "  You · Queued · next Turn",
            "  Process this after the checkpoint report",
            "  › You · Queued · next Turn",
            "  Obsolete checkpoint follow-up",
            "│ ↓  Reply",
        ])

        self.assertTrue(has_selected_queued_input(screen, "Obsolete checkpoint follow-up"))
        self.assertFalse(has_selected_queued_input(screen, "Process this after the checkpoint report"))
        self.assertEqual(
            (4, 5),
            queued_input_click_position(screen, "Obsolete checkpoint follow-up"),
        )

    def test_requires_a_pending_inputs_row(self):
        self.assertFalse(has_selected_queued_input("│ ⢎⡱ Ready Draft", "anything"))

    def test_does_not_match_text_only_in_composer(self):
        screen = "\n".join(
            [
                "Pending inputs · ↑ select · Enter edit · Del withdraw",
                "  › You · Queued · next Turn",
                "  Actual queued input",
                "│ Send to reviewer ▾        ⢎⡱ Ready Draft",
                "Composer text: Obsolete checkpoint follow-up",
            ]
        )

        self.assertIsNone(
            queued_input_click_position(screen, "Obsolete checkpoint follow-up")
        )
        self.assertFalse(
            has_selected_queued_input(screen, "Obsolete checkpoint follow-up")
        )


class MessageActionSelectionTests(unittest.TestCase):
    def test_selects_a_message_action_with_a_transcript_frame_at_the_right(self):
        terminal = MagicMock()
        terminal.request.return_value = {"screen": "The message action ran"}
        screen = "\n".join([
            "  │   Message actions                                           │",
            "  │   › Copy                                             ───────╮",
            "  │     Copy this entry                                         │",
            "  │   Operate message · Enter Run                               │",
        ])
        select_message_action(terminal, screen, "Copy")
        terminal.press.assert_called_once_with("\r")

    def test_tabs_through_a_choice_with_visible_transcript_text_to_its_right(self):
        terminal = MagicMock()
        screen = "\n".join([
            "  │   Message actions                                           │",
            "  │   › Copy                                                    │",
            "  │     Expand                                      transcript text",
            "  │     Full output                                             │",
            "  │   Operate message · Enter Run                               │",
        ])
        terminal.request.side_effect = [
            {"screen": screen.replace("› Copy", "  Copy").replace("  Expand", "› Expand")},
            {"screen": screen.replace("› Copy", "  Copy").replace("  Full output", "› Full output")},
            {"screen": "The message action ran"},
        ]
        select_message_action(terminal, screen, "Full output")
        assert [entry.args[0] for entry in terminal.press.call_args_list] == ["\t", "\t", "\r"]


class TranscriptResizeSynchronizationTests(unittest.TestCase):
    activity_composition = "12 file reads · 3 file searches · 2 text searches"
    draft = "draft survives the review"
    page_up = "\x1b[5~"
    history = "Reading history · ↑↓ scroll"

    def running_frame(self, status_row, *, history=False):
        rows = [""] * 32
        rows[5] = self.activity_composition
        rows[status_row] = self.history if history else "│ ↳  Tools 31s"
        rows[28] = self.draft
        rows[-1] = "Connected · Context 0% used"
        return "\n".join(rows)

    def read_while(self, terminal, predicate, description):
        for _ in range(3):
            screen = terminal.request("screen")["screen"]
            if predicate(screen):
                return screen
        raise AssertionError(description)

    def test_page_up_waits_for_rendered_target_size_after_resize(self):
        stale = self.running_frame(30)
        resized = self.running_frame(26)
        history = self.running_frame(26, history=True)
        markers = (self.activity_composition, self.draft)
        stale_rows = stale.splitlines()
        self.assertEqual(32, len(stale_rows))
        self.assertTrue(
            any("↳  Tools" in row for row in stale_rows[-8:])
        )
        self.assertFalse(rendered_running_transcript(stale, 32, markers))
        self.assertTrue(rendered_running_transcript(resized, 32, markers))
        self.assertTrue(rendered_running_transcript(self.running_frame(27), 32, markers))

        background = resized.replace("Tools", "Running")
        self.assertTrue(rendered_running_transcript(background, 32, markers))
        self.assertFalse(rendered_running_transcript(
            background.replace("Running", "Finished"), 32, markers
        ))

        class DelayedResizeTerminal:
            def __init__(self):
                self.screen = stale
                self.resized = resized
                self.history = history
                self.redraw_pending = False
                self.page_up_frame = None
                self.reading = False

            def request(self, operation, **_fields):
                if operation == "resize":
                    self.redraw_pending = True
                    return {"rows": 32}
                if operation == "screen":
                    if self.reading:
                        return {"screen": self.history}
                    screen = self.screen
                    if self.redraw_pending:
                        self.screen = self.resized
                        self.redraw_pending = False
                    return {"screen": screen}
                raise AssertionError(operation)

            def press(self, text):
                self.page_up_frame = self.screen
                if (
                    text == TranscriptResizeSynchronizationTests.page_up
                    and self.screen == self.resized
                ):
                    self.reading = True

        old_order = DelayedResizeTerminal()
        old_order.request("resize", rows=32, cols=140)
        old_order.press(self.page_up)
        self.assertEqual(stale, old_order.page_up_frame)
        self.assertFalse(old_order.reading)
        with self.assertRaisesRegex(AssertionError, "Reading history"):
            self.read_while(
                old_order,
                lambda screen: self.history in screen,
                "Reading history",
            )

        new_order = DelayedResizeTerminal()
        new_order.request("resize", rows=32, cols=140)
        page_up_after_rendered_resize(
            new_order,
            self.read_while,
            32,
            markers,
        )
        self.assertEqual(resized, new_order.page_up_frame)
        self.assertEqual(
            history,
            self.read_while(
                new_order,
                lambda screen: self.history in screen,
                "Reading history",
            ),
        )
