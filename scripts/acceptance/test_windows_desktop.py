from contextlib import contextmanager
import unittest
from unittest import mock

from desktop import windows

try:
    from PIL import Image, ImageDraw
except ImportError:
    Image = None


@unittest.skipUnless(Image, "Pillow is required for Windows Terminal screenshot tests")
class WindowsSelectionTests(unittest.TestCase):
    origin = (40, 60)

    def reply(self, markers=2):
        image = Image.new("RGB", (360, 90))
        draw = ImageDraw.Draw(image)
        left, row, stride = 19, 40, 8
        marker_size = len(windows.SELECTION_MARKER)
        width = len(windows.SELECTION)
        spacing = (marker_size + width + 2) * stride
        for group in range(markers):
            for column in range(marker_size):
                x = left + group * spacing + column * stride
                draw.rectangle((x - 1, row - 3, x + 1, row + 3), fill="white")
        first = left + (marker_size + 1) * stride
        return image, (first, first + (width - 1) * stride, row, stride)

    def test_waits_for_the_same_target_pixels_at_the_same_window_position(self):
        partial, _ = self.reply(markers=1)
        complete, position = self.reply()
        repaint = complete.copy()
        repaint.putpixel((position[0], position[2]), (120, 120, 120))
        stable = repaint.copy()
        moved = (self.origin[0] + 10, self.origin[1])
        desktop = mock.Mock()
        desktop.capture.side_effect = [(partial, self.origin), (complete, self.origin),
                                       (repaint, self.origin), (repaint, moved), (stable, moved)]

        ready = windows.wait_for_selection_frame(desktop, "reply", "reply selection markers",
                                                 lambda image, _: windows.selection_pixels(image))

        self.assertIs(ready[0], stable)
        self.assertEqual(ready[1:], (moved, position))
        self.assertEqual(desktop.capture.call_count, 5)

    def test_missing_target_keeps_the_unready_frame_for_diagnosis(self):
        partial, _ = self.reply(markers=1)
        desktop = mock.Mock()
        desktop.capture.return_value = (partial, self.origin)

        def timeout(description, probe, timeout):
            self.assertIsNone(probe())
            raise AssertionError(f"Timed out waiting for {description}")

        with mock.patch.object(windows, "eventually", side_effect=timeout):
            with self.assertRaisesRegex(AssertionError, "Timed out waiting for reply selection markers"):
                windows.wait_for_selection_frame(desktop, "reply", "reply selection markers",
                                                 lambda image, _: windows.selection_pixels(image))

        desktop.save_picture.assert_called_once_with("reply-unready", partial)

    def test_drag_waits_for_the_full_highlight_and_checks_clipboard_after_release(self):
        before, position = self.reply()
        first, last, row, stride = position
        partial = before.copy()
        ImageDraw.Draw(partial).rectangle((first - stride // 2, row - 6, (first + last) // 2, row + 6), fill="gray")
        complete = before.copy()
        ImageDraw.Draw(complete).rectangle((first - stride // 2, row - 6, last + stride // 2, row + 6), fill="gray")
        desktop = mock.Mock()
        desktop.capture.side_effect = [(before, self.origin), (partial, self.origin),
                                       (complete, self.origin), (complete, self.origin)]
        clipboard = []
        desktop.clipboard.write.side_effect = clipboard.append
        desktop.clipboard.read.side_effect = lambda: clipboard[-1]

        @contextmanager
        def drag(start, end):
            self.assertEqual(start, (self.origin[0] + first, self.origin[1] + row))
            self.assertEqual(end, (self.origin[0] + last, self.origin[1] + row))
            yield
            clipboard.append(windows.SELECTION)

        desktop.drag.side_effect = drag
        checks = {}

        windows.check_mouse_selection(desktop, checks, (before, self.origin, position))

        self.assertEqual(desktop.capture.call_count, 4)
        desktop.save_picture.assert_called_once_with("01a-mouse-selection", complete)
        self.assertTrue(checks["mouse_selection"]["clipboard_unchanged_while_dragging"])
        self.assertEqual(checks["mouse_selection"]["clipboard_text"], windows.SELECTION)


if __name__ == "__main__":
    unittest.main()
