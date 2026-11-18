"""Drive the installed client in Windows Terminal on a dedicated Windows test desktop."""

import argparse
from contextlib import contextmanager, suppress
import ctypes
from ctypes import wintypes
import hashlib
import io
from itertools import groupby
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from desktop.windows_setup import pinyin_configured, pinyin_registered, powershell, require_active_desktop
from interactive_fixture import DenseTranscript, InstalledClient, csv_review, desktop_wheel, eventually


TITLE = "Kuku Windows Terminal Acceptance"
PASTE = "粘贴第一行 e\u0301 👩‍💻\r\n粘贴第二行\r\n最后X"
PROMPT = "粘贴第一行 e\u0301 👩‍💻\n粘贴第二行\n最后\nshiftline\nctrlline"
SELECTION = "selectable desktop text"
SELECTION_MARKER = "||||||"
SELECTION_LINE = f"{SELECTION_MARKER} {SELECTION} {SELECTION_MARKER}"
REPLY = f"**复制检查**\n\n```text\na\u0301  👩‍💻\n```\n终端回显\n\n{SELECTION_LINE}"
SCREENSHOT_LIMIT = 1024 * 1024
CLIPBOARD_TEXT_LIMIT = 1024 * 1024
CF_UNICODETEXT, GMEM_MOVEABLE, HWND_MESSAGE = 13, 0x0002, -3
VK_BACK, VK_RETURN, VK_SHIFT, VK_CONTROL, VK_SPACE = 0x08, 0x0D, 0x10, 0x11, 0x20
VK_LWIN, VK_NUMPAD_PLUS, VK_OEM_SLASH = 0x5B, 0x6B, 0xBF
VK_LEFT_BUTTON, VK_ALT, VK_RWIN = 0x01, 0x12, 0x5C
MOUSE_MOVE, MOUSE_LEFT_DOWN, MOUSE_LEFT_UP = 0x01, 0x02, 0x04
MOUSE_MOVE_NOCOALESCE, MOUSE_VIRTUAL_DESKTOP, MOUSE_ABSOLUTE = 0x2000, 0x4000, 0x8000
VIRTUAL_SCREEN_METRICS = (76, 77, 78, 79)
PER_MONITOR_AWARE_V2 = -4
ROOT_WINDOW = 2
LANG_CHINESE, LANG_ENGLISH = 0x0804, 0x0409


class KeyboardInput(ctypes.Structure):
    _fields_ = [("key", wintypes.WORD), ("scan", wintypes.WORD), ("flags", wintypes.DWORD),
                ("time", wintypes.DWORD), ("extra", wintypes.WPARAM)]


class MouseInput(ctypes.Structure):
    _fields_ = [("x", wintypes.LONG), ("y", wintypes.LONG), ("data", wintypes.DWORD), ("flags", wintypes.DWORD),
                ("time", wintypes.DWORD), ("extra", wintypes.WPARAM)]


class InputEvent(ctypes.Union):
    _fields_ = [("keyboard", KeyboardInput), ("mouse", MouseInput)]


class Input(ctypes.Structure):
    _fields_ = [("type", wintypes.DWORD), ("event", InputEvent)]


class Clipboard:
    def __init__(self, user):
        self.user = user
        self.kernel = kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        user.OpenClipboard.argtypes = (wintypes.HWND,)
        user.CloseClipboard.argtypes = ()
        user.EmptyClipboard.argtypes = ()
        user.IsClipboardFormatAvailable.argtypes = (wintypes.UINT,)
        user.GetClipboardData.argtypes = (wintypes.UINT,)
        user.GetClipboardData.restype = wintypes.HANDLE
        user.SetClipboardData.argtypes = (wintypes.UINT, wintypes.HANDLE)
        user.SetClipboardData.restype = wintypes.HANDLE
        user.CreateWindowExW.argtypes = (wintypes.DWORD, wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.DWORD,
                                        ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                                        wintypes.HWND, wintypes.HMENU, wintypes.HINSTANCE, ctypes.c_void_p)
        user.CreateWindowExW.restype = wintypes.HWND
        user.DestroyWindow.argtypes = (wintypes.HWND,)
        kernel.GlobalAlloc.argtypes = (wintypes.UINT, ctypes.c_size_t)
        kernel.GlobalAlloc.restype = wintypes.HGLOBAL
        kernel.GlobalFree.argtypes = (wintypes.HGLOBAL,)
        kernel.GlobalFree.restype = wintypes.HGLOBAL
        kernel.GlobalLock.argtypes = (wintypes.HGLOBAL,)
        kernel.GlobalLock.restype = ctypes.c_void_p
        kernel.GlobalUnlock.argtypes = (wintypes.HGLOBAL,)
        kernel.GlobalSize.argtypes = (wintypes.HGLOBAL,)
        kernel.GlobalSize.restype = ctypes.c_size_t

    @contextmanager
    def open(self, owner=None):
        deadline = time.monotonic() + 2
        while not self.user.OpenClipboard(owner):
            if time.monotonic() >= deadline:
                error = ctypes.WinError(ctypes.get_last_error())
                raise RuntimeError(f"OpenClipboard remained unavailable for 2 seconds ({error})") from error
            time.sleep(0.025)
        try:
            yield
        finally:
            if not self.user.CloseClipboard():
                raise ctypes.WinError(ctypes.get_last_error())

    @contextmanager
    def lock(self, handle):
        pointer = self.kernel.GlobalLock(handle)
        if not pointer:
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            yield pointer
        finally:
            self.kernel.GlobalUnlock(handle)

    def read(self):
        with self.open():
            if not self.user.IsClipboardFormatAvailable(CF_UNICODETEXT):
                return ""
            handle = self.user.GetClipboardData(CF_UNICODETEXT)
            if not handle:
                raise ctypes.WinError(ctypes.get_last_error())
            size = self.kernel.GlobalSize(handle)
            if not 2 <= size <= CLIPBOARD_TEXT_LIMIT:
                raise ValueError(f"Unexpected Windows clipboard text allocation size: {size}")
            with self.lock(handle) as pointer:
                data = ctypes.string_at(pointer, size - size % 2)
                return data.decode("utf-16-le", errors="surrogatepass").partition("\0")[0]

    def write(self, text):
        data = text.encode("utf-16-le") + b"\0\0"
        window = self.user.CreateWindowExW(0, "Static", "Kuku Acceptance Clipboard", 0, 0, 0, 0, 0,
                                           HWND_MESSAGE, None, None, None)
        if not window:
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            handle = self.kernel.GlobalAlloc(GMEM_MOVEABLE, len(data))
            if not handle:
                raise ctypes.WinError(ctypes.get_last_error())
            try:
                with self.lock(handle) as pointer:
                    ctypes.memmove(pointer, data, len(data))
                with self.open(window):
                    if not self.user.EmptyClipboard() or not self.user.SetClipboardData(CF_UNICODETEXT, handle):
                        raise ctypes.WinError(ctypes.get_last_error())
                    handle = None
            finally:
                if handle:
                    self.kernel.GlobalFree(handle)
        finally:
            self.user.DestroyWindow(window)


class Desktop:
    def __init__(self, title, evidence, screenshots):
        require_active_desktop()
        user = self.user = ctypes.WinDLL("user32", use_last_error=True)
        user.FindWindowW.argtypes = (wintypes.LPCWSTR, wintypes.LPCWSTR)
        user.FindWindowW.restype = wintypes.HWND
        user.GetForegroundWindow.restype = wintypes.HWND
        user.GetWindowThreadProcessId.argtypes = (wintypes.HWND, ctypes.POINTER(wintypes.DWORD))
        user.GetWindowThreadProcessId.restype = wintypes.DWORD
        user.GetKeyboardLayout.argtypes = (wintypes.DWORD,)
        user.GetKeyboardLayout.restype = wintypes.HKL
        user.SendInput.argtypes = (wintypes.UINT, ctypes.POINTER(Input), ctypes.c_int)
        user.SendInput.restype = wintypes.UINT
        user.SetForegroundWindow.argtypes = (wintypes.HWND,)
        user.PostMessageW.argtypes = (wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM)
        user.GetAsyncKeyState.argtypes = (ctypes.c_int,)
        user.GetAsyncKeyState.restype = ctypes.c_short
        user.SetThreadDpiAwarenessContext.argtypes = (wintypes.HANDLE,)
        user.SetThreadDpiAwarenessContext.restype = wintypes.HANDLE
        if not user.SetThreadDpiAwarenessContext(PER_MONITOR_AWARE_V2):
            raise ctypes.WinError(ctypes.get_last_error())
        user.GetSystemMetrics.argtypes = (ctypes.c_int,)
        user.WindowFromPhysicalPoint.argtypes = (wintypes.POINT,)
        user.WindowFromPhysicalPoint.restype = wintypes.HWND
        user.GetAncestor.argtypes = (wintypes.HWND, wintypes.UINT)
        user.GetAncestor.restype = wintypes.HWND
        self.evidence = evidence
        self.screenshots = screenshots
        self.title = title
        self.window = eventually("the Windows Terminal acceptance window", self.find, timeout=15)
        self.thread = user.GetWindowThreadProcessId(self.window, None)
        self.clipboard = Clipboard(user)

    def find(self):
        return self.user.FindWindowW("CASCADIA_HOSTING_WINDOW_CLASS", self.title)

    def close(self):
        if self.find() == self.window:
            self.user.PostMessageW(self.window, 0x0010, 0, 0)

    def focus(self):
        require_active_desktop()
        assert self.find() == self.window, "The acceptance window is no longer available"
        if self.user.GetForegroundWindow() != self.window:
            self.user.SetForegroundWindow(self.window)
            eventually("Windows Terminal to receive foreground focus",
                       lambda: self.user.GetForegroundWindow() == self.window, timeout=4)

    def press(self, *keys):
        self.focus()
        presses = [Input(1, InputEvent(keyboard=KeyboardInput(key, 0, 0))) for key in keys]
        releases = [Input(1, InputEvent(keyboard=KeyboardInput(key, 0, 2))) for key in reversed(keys)]
        self.send(*presses, *releases)
        time.sleep(0.08)

    def send(self, *events):
        inputs = (Input * len(events))(*events)
        if self.user.SendInput(len(events), inputs, ctypes.sizeof(Input)) != len(events):
            raise ctypes.WinError(ctypes.get_last_error())

    def move_cursor(self, point):
        left, top, width, height = (self.user.GetSystemMetrics(index) for index in VIRTUAL_SCREEN_METRICS)
        x, y = point[0] - left, point[1] - top
        assert 0 <= x < width and 0 <= y < height, "Mouse position outside the virtual desktop"
        flags = MOUSE_MOVE | MOUSE_MOVE_NOCOALESCE | MOUSE_VIRTUAL_DESKTOP | MOUSE_ABSOLUTE
        absolute_x = (x * 65536 + width - 1) // width
        absolute_y = (y * 65536 + height - 1) // height
        self.send(Input(0, InputEvent(mouse=MouseInput(absolute_x, absolute_y, 0, flags))))

    @contextmanager
    def drag(self, start, end):
        self.focus()
        keys = (VK_LEFT_BUTTON, VK_SHIFT, VK_CONTROL, VK_ALT, VK_LWIN, VK_RWIN)
        assert not any(self.user.GetAsyncKeyState(key) & 0x8000 for key in keys), (
            "Mouse selection requires the left button and modifiers to be released"
        )
        for point in (start, end):
            hit = self.user.WindowFromPhysicalPoint(wintypes.POINT(*point))
            assert self.user.GetAncestor(hit, ROOT_WINDOW) == self.window, (
                "Mouse selection must stay inside Windows Terminal"
            )
        self.move_cursor(start)
        self.send(Input(0, InputEvent(mouse=MouseInput(0, 0, 0, MOUSE_LEFT_DOWN))))
        try:
            self.move_cursor(((start[0] + end[0]) // 2, start[1]))
            time.sleep(0.1)
            self.move_cursor(end)
            yield
        finally:
            self.send(Input(0, InputEvent(mouse=MouseInput(0, 0, 0, MOUSE_LEFT_UP))))

    def wheel_up(self):
        image, origin = self.capture()
        self.move_cursor((origin[0] + image.width // 2, origin[1] + image.height // 2))
        self.send(Input(0, InputEvent(mouse=MouseInput(0, 0, 360, 0x0800))))

    def spell(self, letters):
        for letter in letters:
            self.press(ord(letter.upper()))

    def language(self, expected):
        for _ in range(4):
            layout = self.user.GetKeyboardLayout(self.thread) or 0
            if layout & 0xFFFF == expected:
                return f"0x{layout & 0xFFFFFFFF:08x}"
            self.press(VK_LWIN, VK_SPACE)
            time.sleep(0.35)
        raise AssertionError(f"Windows Terminal did not switch to the keyboard language 0x{expected:04x}")

    def capture(self, settle=0):
        from PIL import ImageGrab

        self.focus()
        if settle:
            time.sleep(settle)
        bounds = wintypes.RECT()
        dwm = ctypes.WinDLL("dwmapi")
        dwm.DwmGetWindowAttribute.argtypes = (wintypes.HWND, wintypes.DWORD, ctypes.c_void_p, wintypes.DWORD)
        if dwm.DwmGetWindowAttribute(self.window, 9, ctypes.byref(bounds), ctypes.sizeof(bounds)):
            raise RuntimeError("Could not capture the Windows Terminal client bounds")
        rectangle = (bounds.left, bounds.top, bounds.right, bounds.bottom)
        image = ImageGrab.grab(bbox=rectangle, include_layered_windows=True, all_screens=True)
        return image, (bounds.left, bounds.top)

    def picture(self, name, settle=0.2):
        image, _ = self.capture(settle)
        return self.save_picture(name, image)

    def save_picture(self, name, image):
        data = io.BytesIO()
        image.save(data, format="PNG")
        content = data.getvalue()
        if len(content) > SCREENSHOT_LIMIT:
            raise ValueError("Windows Terminal screenshot exceeds the evidence limit")
        filename = f"{name}.png"
        (self.evidence / filename).write_bytes(content)
        self.screenshots.append({"path": filename, "bytes": len(content), "sha256": hashlib.sha256(content).hexdigest()})
        return filename


def terminal_child(state, binary):
    process = subprocess.Popen([binary])
    previous = None
    while process.poll() is None:
        columns, rows = os.get_terminal_size()
        grid = {"columns": columns, "rows": rows}
        if grid != previous:
            state.write_text(json.dumps({"grid": grid}), encoding="utf-8")
            previous = grid
        time.sleep(0.1)
    state.write_text(json.dumps({"exit_code": process.returncode}), encoding="utf-8")
    return process.returncode


def read_state(state):
    with suppress(FileNotFoundError, PermissionError, json.JSONDecodeError):
        return json.loads(state.read_text(encoding="utf-8"))
    return {}


def selection_pixels(image):
    rgb = image.convert("RGB")
    pixels = rgb.load()
    marker_size = len(SELECTION_MARKER)
    marker_spacing = marker_size + len(SELECTION) + 2
    matches = {}
    for y in range(image.height):
        background = pixels[0, y]
        runs = []
        start = None
        for x in range(1, image.width):
            if pixels[x, y] != background:
                if start is None:
                    start = x
            elif start is not None:
                runs.append((start, x - start))
                start = None
        markers = set()
        for i in range(len(runs) - marker_size + 1):
            x, width = runs[i]
            stride = runs[i + 1][0] - x
            columns = [(x + column * stride, width) for column in range(marker_size)]
            if runs[i:i + marker_size] == columns:
                markers.add((x, stride, width))
        for x, stride, width in markers:
            if (x + marker_spacing * stride, stride, width) in markers:
                matches.setdefault((x + width // 2, stride), []).append(y)

    candidates = []
    for (x, stride), rows in matches.items():
        for _, band in groupby(enumerate(rows), key=lambda row: row[1] - row[0]):
            contiguous_rows = [y for _, y in band]
            if len(contiguous_rows) >= 3:
                candidates.append((x, stride, contiguous_rows[len(contiguous_rows) // 2]))
    if len(candidates) != 1:
        return None
    left, stride, y = candidates[0]
    first = left + (marker_size + 1) * stride
    last = first + (len(SELECTION) - 1) * stride
    return first, last, y, stride


def wait_for_selection_frame(desktop, name, description, locate):
    previous = None
    image = None

    def sample():
        nonlocal previous, image
        image, origin = desktop.capture()
        selection = locate(image, origin)
        if selection is None:
            previous = None
            return None
        first, last, row, stride = selection
        strip = image.crop((first - stride, row, last + stride + 1, row + 1))
        current = (origin, image.size, selection, strip.tobytes())
        if current == previous:
            return image, origin, selection
        previous = current

    try:
        return eventually(description, sample, timeout=8)
    except AssertionError:
        if image is not None:
            desktop.save_picture(name + "-unready", image)
        raise


def selection_highlight(before, image, selection):
    first, last, row, stride = selection
    original, current = before.load(), image.load()
    half_cell = stride // 2

    def highlighted(center):
        columns = range(center - half_cell, center + half_cell + 1)
        return sum(original[x, row] != current[x, row] for x in columns) > half_cell

    return all(highlighted(center) for center in range(first, last + 1, stride))


def check_mouse_selection(desktop, checks, reply_frame):
    sentinel = f"previous clipboard value {secrets.token_hex(8)}"
    desktop.focus()
    desktop.clipboard.write(sentinel)
    assert desktop.clipboard.read() == sentinel, "Windows clipboard did not hold the mouse selection sentinel"
    before, origin, selection = reply_frame
    left, top = origin
    first, last, row, cell_width = selection
    start = (left + first, top + row)
    end = (left + last, top + row)
    result = checks["mouse_selection"] = {"button": "left", "modifiers": [], "start": start, "end": end,
                                          "expected_text": SELECTION, "cell_width_pixels": cell_width}

    def highlighted(image, current_origin):
        if current_origin == origin and image.size == before.size and selection_highlight(before, image, selection):
            return selection

    with desktop.drag(start, end):
        name = "01a-mouse-selection"
        image, _, _ = wait_for_selection_frame(desktop, name, "stable Windows Terminal selection highlight", highlighted)
        result["screenshot"] = desktop.save_picture(name, image)
        assert desktop.clipboard.read() == sentinel, "Mouse selection changed the clipboard before the button was released"
        result["clipboard_unchanged_while_dragging"] = True

    def copied_text():
        actual = desktop.clipboard.read()
        result["clipboard_text"] = actual[:512]
        return actual == SELECTION

    eventually("the exact dragged text in the Windows clipboard after release", copied_text, timeout=8)


def dense_layout(client, desktop, workspace, grid):
    original = grid()

    def capture_widths(name):
        wide = desktop.picture(name + "-wide")
        for _ in range(5):
            desktop.press(VK_CONTROL, VK_NUMPAD_PLUS)
        narrow = eventually("a narrow terminal for the dense transcript", narrow_grid)
        narrow_screen = desktop.picture(name + "-narrow")
        desktop.press(VK_CONTROL, ord("0"))
        eventually("the original terminal width", lambda: grid() == original)
        return {"wide": {"grid": original, "screenshot": wide},
                "narrow": {"grid": narrow, "screenshot": narrow_screen}}

    def narrow_grid():
        size = grid()
        if size and size["columns"] <= original["columns"] * 3 / 4:
            return size

    def copy_reply(expected):
        desktop.press(VK_CONTROL, ord("C"))
        desktop.press(VK_OEM_SLASH)
        desktop.spell("copy")
        desktop.press(VK_RETURN)
        eventually("the complete Chinese report in the Windows clipboard",
                   lambda: desktop.clipboard.read() == expected, timeout=12)

    return DenseTranscript(client, workspace).desktop(
        desktop.spell, lambda: desktop.press(VK_RETURN), capture_widths, copy_reply,
        lambda: desktop.press(VK_CONTROL, ord("O")))


def exercise(client, desktop, workspace, state, checks):
    def grid():
        return read_state(state).get("grid")

    eventually("the installed client console", grid, timeout=10)
    eventually("managed service discovery", client.discovery.is_file, timeout=12)
    assert not client.status()["busy"] and client.sessions(workspace) == []
    checks["blank"] = {"screenshot": desktop.picture("00-blank"), "sessions": 0}

    desktop.language(LANG_ENGLISH)
    desktop.spell("desktop")
    desktop.press(VK_RETURN)
    call = client.provider.next("desktop")
    expect_input(call, "desktop")
    call.text(REPLY)
    session = client.session(workspace)
    eventually("the copied reply to persist", lambda: client.snapshot(workspace, session)["last_output"] == REPLY,
               timeout=10)

    desktop.focus()
    desktop.clipboard.write("previous isolated clipboard value")
    desktop.press(VK_OEM_SLASH)
    desktop.spell("copy")
    desktop.press(VK_RETURN)
    eventually("the exact response in the Windows clipboard", lambda: desktop.clipboard.read() == REPLY, timeout=12)
    checks["copy"] = {"command": "/copy", "clipboard_utf8_sha256": hashlib.sha256(REPLY.encode()).hexdigest()}
    name = "01-reply-copied"
    reply_frame = wait_for_selection_frame(desktop, name, "stable Windows Terminal reply selection markers",
                                           lambda image, _: selection_pixels(image))
    checks["copy"]["screenshot"] = desktop.save_picture(name, reply_frame[0])
    check_mouse_selection(desktop, checks, reply_frame)
    client.provider.assert_idle(duration=0.25)

    original = grid()
    for _ in range(3):
        desktop.press(VK_CONTROL, VK_NUMPAD_PLUS)

    def enlarged_grid():
        size = grid()
        if size and all(size[name] < original[name] for name in original):
            return size

    enlarged = eventually("a smaller console grid after increasing the font", enlarged_grid, timeout=8)
    zoomed = desktop.picture("02-zoom-in")
    desktop.press(VK_CONTROL, ord("0"))
    restored = eventually("the original console grid after resetting the font",
                          lambda: grid() == original and original, timeout=8)
    checks["zoom"] = {"original": original, "enlarged": enlarged, "restored": restored,
                      "screenshots": [zoomed, desktop.picture("03-zoom-reset")]}
    client.provider.assert_idle()

    old = client.snapshot(workspace, session)
    checks["paste"] = {"shortcut": "Ctrl+Shift+V", "clipboard_input": PASTE}
    desktop.focus()
    desktop.clipboard.write(PASTE)
    assert desktop.clipboard.read() == PASTE, "Windows clipboard did not hold the paste fixture"
    desktop.press(VK_CONTROL, VK_SHIFT, ord("V"))
    checks["paste"]["screenshot"] = desktop.picture("04-pasted-draft")
    client.provider.assert_idle(duration=0.25)
    after = client.snapshot(workspace, session)
    assert after["turn"] == old["turn"] and after["pending_inputs"] == 0, "Pasting must leave the draft unsent"
    desktop.press(VK_BACK)
    desktop.press(VK_SHIFT, VK_RETURN)
    desktop.spell("shiftline")
    desktop.press(VK_CONTROL, ord("J"))
    desktop.spell("ctrlline")
    client.provider.assert_idle(duration=0.25)
    desktop.press(VK_RETURN)
    call = client.provider.next("最后")
    expect_input(call, PROMPT)
    checks["paste"]["submitted_draft"] = PROMPT
    overview = csv_review(client, workspace, call)
    eventually("the paste acknowledgement to persist",
               lambda: client.snapshot(workspace, session)["last_output"] == overview,
               timeout=10)
    client.provider.assert_idle(duration=0.25)

    checks["layout"] = {"screenshot": desktop.picture("04a-turns-tools-table")}
    checks["dense_transcript"] = dense_layout(client, desktop, workspace, grid)
    checks["wheel"] = desktop_wheel(client, workspace, desktop.spell, desktop.wheel_up,
                                    lambda: desktop.press(VK_CONTROL, 0x23),
                                    lambda: desktop.press(VK_RETURN), desktop.picture)

    if pinyin_registered() and pinyin_configured():
        checks["ime"] = {"language": desktop.language(LANG_CHINESE), "keys": "nihao<Space>"}
        desktop.spell("nihao")
        checks["ime"]["screenshots"] = [desktop.picture("05-pinyin-before-space", settle=1.5)]
        desktop.press(VK_SPACE)
        checks["ime"]["screenshots"].append(desktop.picture("06-pinyin-after-space", settle=1.0))
        client.provider.assert_idle(duration=0.25)
        desktop.language(LANG_ENGLISH)
        desktop.press(VK_RETURN)
        call = client.provider.next("你好")
        expect_input(call, "你好")
        call.text("Microsoft Pinyin verified")
        eventually("the IME acknowledgement to persist",
                   lambda: client.snapshot(workspace, session)["last_output"] == "Microsoft Pinyin verified",
                   timeout=10)
        checks["ime"].update(status="passed", committed="你好")
    else:
        checks["ime"] = {"status": "unavailable", "reason": "Microsoft Pinyin is not configured"}

    desktop.press(VK_OEM_SLASH)
    desktop.spell("quit")
    desktop.press(VK_RETURN)

    def exit_state():
        receipt = read_state(state)
        return receipt if "exit_code" in receipt else None

    exited = eventually("a clean client exit", exit_state, timeout=12)
    assert exited["exit_code"] == 0, f"Kuku exited with {exited['exit_code']}"
    checks["exit"] = exited


def expect_input(call, text):
    message = call.request["messages"][-1]
    texts = [block["text"] for block in message["content"] if block["type"] == "text"]
    assert message["role"] == "user" and text in texts, f"The submitted draft did not match: {texts!r}"


def run(args, result):
    require_active_desktop()
    terminal = args.windows_terminal.resolve()
    if not terminal.is_file() or terminal.name.lower() != "windowsterminal.exe":
        raise ValueError("Select the WindowsTerminal.exe from the dedicated test desktop")
    escaped = str(terminal).replace("'", "''")
    result.update(terminal_version=powershell(f"(Get-Item -LiteralPath '{escaped}').VersionInfo.FileVersion"),
                  windows_build=sys.getwindowsversion().build,
                  binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest())
    with InstalledClient(args.binary, args.bridge, args.directory) as client:
        workspace = client.workspace("work-windows-terminal")
        state = args.directory / "terminal-state.json"
        title = f"{TITLE} {secrets.token_hex(4)}"
        process = subprocess.Popen([terminal, "-w", "new", "--maximized", "new-tab", "--suppressApplicationTitle",
                                    "--title", title, "--startingDirectory", workspace, "--inheritEnvironment", "--",
                                    sys.executable, "-B", Path(__file__).resolve(), "--terminal-child", state, args.binary],
                                   cwd=workspace, env=client.env)
        desktop = None
        try:
            desktop = Desktop(title, args.evidence, result["screenshots"])
            exercise(client, desktop, workspace, state, result["checks"])
            assert process.wait(timeout=10) == 0, "Windows Terminal exited unsuccessfully"
        finally:
            if desktop is not None:
                desktop.close()
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)


def main():
    if len(sys.argv) == 4 and sys.argv[1] == "--terminal-child":
        return terminal_child(Path(sys.argv[2]), Path(sys.argv[3]))
    parser = argparse.ArgumentParser(description=__doc__)
    for argument in ("binary", "bridge", "directory", "evidence", "windows-terminal"):
        parser.add_argument(f"--{argument}", type=Path, required=True)
    args = parser.parse_args()
    result = {"passed": False, "checks": {}, "screenshots": []}
    try:
        run(args, result)
        if result["checks"]["ime"]["status"] == "unavailable":
            result["partial"] = True
            raise RuntimeError(result["checks"]["ime"]["reason"])
        result["passed"] = True
    except Exception as error:
        result["failure"] = str(error)[:2048]
        raise
    finally:
        (args.evidence / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
