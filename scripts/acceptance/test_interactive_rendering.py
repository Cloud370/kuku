import unittest
from unittest.mock import Mock

from interactive_rendering import (
    click_queued_input,
    has_selected_queued_input,
    has_standalone_assistant_rail,
    page_up_after_rendered_resize,
    queued_input_click_context,
    queued_input_click_position,
    rendered_running_transcript,
)


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
    def test_click_queued_input_returns_and_sends_target_coordinates(self):
        screen = "\n".join([
            "Pending inputs · ↑ select · Enter edit · Del withdraw",
            "  You · Queued · next Turn",
            "  Process this after the checkpoint report",
            "  You · Queued · next Turn",
            "  Obsolete checkpoint follow-up",
            "│ To main · Streaming reply",
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
            click_queued_input(terminal, "To main · Draft", "missing")

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
            "│ To main · Streaming reply",
        ])

        self.assertTrue(has_selected_queued_input(screen, "Obsolete checkpoint follow-up"))
        self.assertFalse(has_selected_queued_input(screen, "Process this after the checkpoint report"))
        self.assertEqual(
            (4, 5),
            queued_input_click_position(screen, "Obsolete checkpoint follow-up"),
        )

    def test_requires_a_pending_inputs_row(self):
        self.assertFalse(has_selected_queued_input("To main · Draft", "anything"))

    def test_does_not_match_text_only_in_composer(self):
        screen = "\n".join(
            [
                "Pending inputs · ↑ select · Enter edit · Del withdraw",
                "  › You · Queued · next Turn",
                "  Actual queued input",
                "│ To main · Draft",
                "Composer text: Obsolete checkpoint follow-up",
            ]
        )

        self.assertIsNone(
            queued_input_click_position(screen, "Obsolete checkpoint follow-up")
        )
        self.assertFalse(
            has_selected_queued_input(screen, "Obsolete checkpoint follow-up")
        )


class TranscriptResizeSynchronizationTests(unittest.TestCase):
    activity_composition = "12 file reads · 3 file searches · 2 text searches"
    draft = "draft survives the review"
    page_up = "\x1b[5~"
    history = "Reading history · ↑↓ scroll"

    def running_frame(self, status_row, *, history=False):
        rows = [""] * 32
        rows[5] = self.activity_composition
        rows[status_row] = self.history if history else "To main · Running tools 31s"
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
            any("To main · Running tools" in row for row in stale_rows[-8:])
        )
        self.assertFalse(rendered_running_transcript(stale, 32, markers))
        self.assertTrue(rendered_running_transcript(resized, 32, markers))

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
