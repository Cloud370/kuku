"""Drive the installed client in Windows Terminal on a dedicated Windows test desktop."""

import argparse
import base64
from contextlib import suppress
import ctypes
from ctypes import wintypes
import hashlib
import io
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from desktop.windows_setup import pinyin_configured, pinyin_registered, powershell, require_active_desktop
from interactive_fixture import InstalledClient, eventually


TITLE = "Kuku Windows Terminal Acceptance"
PASTE = "粘贴第一行 e\u0301 👩‍💻\r\n粘贴第二行\r\n最后X"
PROMPT = "粘贴第一行 e\u0301 👩‍💻\n粘贴第二行\n最后\nshiftline\nctrlline"
REPLY = "**复制检查**\n\n```text\na\u0301  👩‍💻\n```\n终端回显"
SCREENSHOT_LIMIT = 1024 * 1024
VK_BACK, VK_RETURN, VK_SHIFT, VK_CONTROL, VK_SPACE = 0x08, 0x0D, 0x10, 0x11, 0x20
VK_LWIN, VK_NUMPAD_PLUS, VK_OEM_SLASH = 0x5B, 0x6B, 0xBF
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
        self.evidence = evidence
        self.screenshots = screenshots
        self.title = title
        self.window = eventually("the Windows Terminal acceptance window", self.find, timeout=15)
        self.thread = user.GetWindowThreadProcessId(self.window, None)

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
        events = (Input * (2 * len(keys)))(*presses, *releases)
        if self.user.SendInput(len(events), events, ctypes.sizeof(Input)) != len(events):
            raise ctypes.WinError(ctypes.get_last_error())
        time.sleep(0.08)

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

    def picture(self, name):
        from PIL import ImageGrab

        self.focus()
        time.sleep(0.2)
        bounds = wintypes.RECT()
        dwm = ctypes.WinDLL("dwmapi")
        dwm.DwmGetWindowAttribute.argtypes = (wintypes.HWND, wintypes.DWORD, ctypes.c_void_p, wintypes.DWORD)
        if dwm.DwmGetWindowAttribute(self.window, 9, ctypes.byref(bounds), ctypes.sizeof(bounds)):
            raise RuntimeError("Could not capture the Windows Terminal client bounds")
        image = ImageGrab.grab(bbox=(bounds.left, bounds.top, bounds.right, bounds.bottom))
        data = io.BytesIO()
        image.save(data, format="PNG")
        content = data.getvalue()
        if len(content) > SCREENSHOT_LIMIT:
            raise ValueError("Windows Terminal screenshot exceeds the evidence limit")
        filename = f"{name}.png"
        (self.evidence / filename).write_bytes(content)
        self.screenshots.append({"path": filename, "bytes": len(content), "sha256": hashlib.sha256(content).hexdigest()})
        return filename


def clipboard(text=None):
    require_active_desktop()
    script = "Add-Type -AssemblyName System.Windows.Forms; "
    if text is None:
        result = powershell(script + "[Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes("
                            "[Windows.Forms.Clipboard]::GetText()))")
        return base64.b64decode(result).decode("utf-16-le")
    encoded = base64.b64encode(text.encode("utf-16-le")).decode("ascii")
    powershell(script + "[Windows.Forms.Clipboard]::SetText("
               f"[Text.Encoding]::Unicode.GetString([Convert]::FromBase64String('{encoded}')))")


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

    clipboard("previous isolated clipboard value")
    desktop.press(VK_OEM_SLASH)
    desktop.spell("copy")
    desktop.press(VK_RETURN)
    eventually("the exact response in the Windows clipboard", lambda: clipboard() == REPLY, timeout=12)
    checks["copy"] = {"command": "/copy", "clipboard_utf8_sha256": hashlib.sha256(REPLY.encode()).hexdigest(),
                      "screenshot": desktop.picture("01-reply-copied")}
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
    clipboard(PASTE)
    assert clipboard() == PASTE, "Windows clipboard did not hold the paste fixture"
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
    call.text("Windows Terminal paste verified")
    eventually("the paste acknowledgement to persist",
               lambda: client.snapshot(workspace, session)["last_output"] == "Windows Terminal paste verified",
               timeout=10)
    client.provider.assert_idle(duration=0.25)

    if pinyin_registered() and pinyin_configured():
        checks["ime"] = {"language": desktop.language(LANG_CHINESE), "keys": "nihao<Space>"}
        desktop.spell("nihao")
        checks["ime"]["screenshots"] = [desktop.picture("05-pinyin-candidates")]
        desktop.press(VK_SPACE)
        checks["ime"]["screenshots"].append(desktop.picture("06-pinyin-commit"))
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

    desktop.press(VK_CONTROL, ord("C"))

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
