import unittest

from interactive_rendering import (
    has_selected_queued_input,
    has_standalone_assistant_rail,
    queued_input_click_position,
)


class MixedTableScrollTests(unittest.TestCase):
    def test_ignores_user_card_gutter_above_assistant(self):
        frame = [" │", "", " Assistant", " Before table 00", " After table 00"]

        self.assertFalse(has_standalone_assistant_rail(frame))

    def test_detects_orphan_rail_in_assistant_body(self):
        frame = ["", " Assistant", " Before table 00", " │", " After table 00"]

        self.assertTrue(has_standalone_assistant_rail(frame))


class QueuedInputFocusTests(unittest.TestCase):
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

    def test_scans_table_frame_without_assistant_heading(self):
        frame = [" Before table 00", " After table 00"]

        self.assertFalse(has_standalone_assistant_rail(frame))

    def test_detects_orphan_rail_without_assistant_heading(self):
        frame = [" Before table 00", " │", " After table 00"]

        self.assertTrue(has_standalone_assistant_rail(frame))
