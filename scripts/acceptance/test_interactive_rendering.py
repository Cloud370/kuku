import unittest

from interactive_rendering import has_standalone_assistant_rail


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
