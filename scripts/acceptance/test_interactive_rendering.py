import unittest
from unittest.mock import Mock

from interactive_rendering import (
    click_queued_input,
    has_selected_queued_input,
    has_standalone_assistant_rail,
    queued_input_click_context,
    queued_input_click_position,
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

    def test_click_failure_context_includes_target_row_and_coordinates(self):
        self.assertEqual(
            "Queued input target row: 4\n"
            "Mouse click coordinates (column, row; 1-based): (3, 4)",
            queued_input_click_context(4, 3),
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
