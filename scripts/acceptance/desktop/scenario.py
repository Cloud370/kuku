"""Drive the installed client through a disposable Xfce Terminal and IBus session."""

from contextlib import contextmanager
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import secrets
import struct
import subprocess
import sys
import tempfile
import termios
import time

from interactive_fixture import InstalledClient, csv_review, eventually


TEST = Path("/test")
EVIDENCE = Path("/evidence")
DISPLAY = ":99"
TITLE = "Kuku Desktop Acceptance"
SCREEN_SIZE = "1440x1000x24"
SCREENSHOT_LIMIT = 1024 * 1024
PASTE = "\r\n第二行\r第三行\n末尾X"
PROMPT = "你好\n第二行\n第三行\n末尾"
REPLY = "**复制检查**\n\n```text\ná  👩‍💻\n```\n终端回显"


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def command(*args, data=None, check=True):
    result = subprocess.run(args, input=data, capture_output=True, timeout=10)
    if check and result.returncode:
        detail = result.stderr[-4096:].decode(errors="replace").strip()
        raise RuntimeError(f"{args[0]} returned {result.returncode}: {detail}")
    return result


@contextmanager
def process(args, **kwargs):
    child = subprocess.Popen(args, **kwargs)
    try:
        yield child
    finally:
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=5)


class Desktop:
    def __init__(self, temporary, screenshots):
        self.temporary = temporary
        self.screenshots = screenshots

    @staticmethod
    def window(title):
        result = command("xdotool", "search", "--onlyvisible", "--name", f"^{title}$", check=False)
        if result.returncode:
            return None
        windows = result.stdout.decode().split()
        assert len(windows) == 1, f"Expected one window for {title}"
        return windows[0]

    @staticmethod
    def focus(window):
        command("xdotool", "windowactivate", "--sync", window)
        active = command("xdotool", "getactivewindow").stdout.decode().strip()
        assert active == window, "The test window must be focused before sending keys"

    def key(self, window, *keys):
        self.focus(window)
        command("xdotool", "key", "--clearmodifiers", "--delay", "100", *keys)

    def type(self, window, text):
        assert text.isascii(), "Chinese must be composed through IBus"
        self.focus(window)
        command("xdotool", "type", "--clearmodifiers", "--delay", "100", text)

    def screen(self, name):
        time.sleep(0.15)
        snapshot = self.temporary / "snapshot.png"
        command("scrot", "-z", "--overwrite", snapshot)
        with snapshot.open("rb") as stream:
            data = stream.read(SCREENSHOT_LIMIT + 1)
        assert len(data) <= SCREENSHOT_LIMIT and data.startswith(b"\x89PNG\r\n\x1a\n"), (
            "Expected a PNG screenshot within the retention limit"
        )
        path = f"{name}.png"
        (EVIDENCE / path).write_bytes(data)
        self.screenshots.append({"path": path, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
        return path

    @staticmethod
    def clipboard():
        return command("xclip", "-selection", "clipboard", "-out", check=False).stdout

    @staticmethod
    def set_clipboard(data):
        subprocess.run(["xclip", "-selection", "clipboard", "-in"], input=data,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True, timeout=5)


def select_engine(name):
    requested = command("ibus", "engine", name, check=False)
    selected = command("ibus", "engine").stdout.decode().strip()
    assert selected == name, f"Requested IBus engine {name}, got {selected}"
    return requested.returncode


def terminal_grid(pid):
    with open(f"/proc/{pid}/fd/0", "rb", buffering=0) as tty:
        rows, columns, _, _ = struct.unpack("HHHH", fcntl.ioctl(tty, termios.TIOCGWINSZ, bytes(8)))
    assert rows > 0 and columns > 0, "The client must have a live terminal grid"
    return {"rows": rows, "columns": columns}


def larger_font(pid, original):
    grid = terminal_grid(pid)
    if grid["rows"] < original["rows"] and grid["columns"] < original["columns"]:
        return grid


def original_font(pid, original):
    grid = terminal_grid(pid)
    return grid if grid == original else None


def capture_workspace_menus(desktop, window):
    desktop.key(window, "ctrl+p")
    screens = {"actions": desktop.screen("07a-actions")}
    desktop.key(window, "Escape")
    for name in ("model", "statusline"):
        desktop.type(window, "/" + name)
        desktop.key(window, "Return")
        screens[name] = desktop.screen("07b-" + name)
        desktop.key(window, "Escape")
    return screens


def exercise(client, desktop, checks):
    workspace = client.workspace("work-desktop")
    state = client.directory / "terminal.json"
    args = ["xfce4-terminal", "--disable-server", f"--title={TITLE}", "--hide-menubar", "--maximize",
            f"--working-directory={workspace}", "--execute", sys.executable, Path(__file__),
            "--terminal-child", state]
    with process(args, env=client.env, cwd=workspace) as terminal:
        def terminal_window():
            assert terminal.poll() is None, "Xfce Terminal exited before creating its window"
            return desktop.window(TITLE)

        window = eventually("the Xfce Terminal window", terminal_window, timeout=12)
        eventually("the GUI terminal child", state.is_file, timeout=6)
        pid = json.loads(state.read_text(encoding="utf-8"))["pid"]
        eventually("managed service discovery", client.discovery.exists, timeout=12)
        assert not client.status()["busy"] and client.sessions(workspace) == []
        checks["blank"] = {"sessions": 0, "screenshot": desktop.screen("00-blank")}

        desktop.focus(window)
        engine_status = select_engine("libpinyin")
        desktop.type(window, "nihao")
        preedit = desktop.screen("01-pinyin-preedit")
        client.provider.assert_idle(duration=0.25)
        desktop.key(window, "space")
        committed = desktop.screen("02-pinyin-commit")
        select_engine("xkb:us::eng")

        desktop.set_clipboard(PASTE.encode())
        assert desktop.clipboard() == PASTE.encode()
        desktop.key(window, "ctrl+shift+v")
        warning = "Warning: Potentially Unsafe Paste"
        dialog = eventually("the Xfce Terminal paste confirmation", lambda: desktop.window(warning), timeout=6)
        confirmation = desktop.screen("03-paste-confirmation")
        desktop.key(dialog, "Return")
        eventually("the paste confirmation to close", lambda: not desktop.window(warning), timeout=6)
        draft = desktop.screen("04-pasted-draft")
        assert client.sessions(workspace) == []
        client.provider.assert_idle(duration=0.25)

        original = terminal_grid(pid)
        desktop.key(window, "ctrl+plus")
        enlarged = eventually("font zoom to reduce the terminal grid", lambda: larger_font(pid, original), timeout=8)
        zoomed = desktop.screen("05-zoom-in")
        desktop.key(window, "ctrl+minus")
        restored = eventually("font zoom to restore the terminal grid", lambda: original_font(pid, original), timeout=8)
        zoom_restored = desktop.screen("06-zoom-restored")
        checks["zoom"] = {"original": original, "enlarged": enlarged, "restored": restored,
                          "screenshots": [draft, zoomed, zoom_restored]}
        client.provider.assert_idle()

        desktop.key(window, "BackSpace", "Return")
        call = client.provider.next("末尾")
        message = call.request["messages"][-1]
        assert message["role"] == "user"
        texts = [block["text"] for block in message["content"] if block["type"] == "text"]
        assert PROMPT in texts, f"The submitted draft did not match: {texts!r}"
        checks["ime"] = {"engine": "libpinyin", "switch_exit_code": engine_status, "keys": "nihao<Space>",
                         "committed": "你好", "screenshots": [preedit, committed]}
        checks["paste"] = {"shortcut": "Ctrl+Shift+V", "clipboard_input": PASTE, "submitted_draft": PROMPT,
                           "provider_requests_before_enter": 0, "screenshots": [confirmation, draft, zoom_restored]}

        call.text(REPLY)
        session = client.session(workspace)
        eventually("the fixture reply to persist", lambda: client.snapshot(workspace, session)["last_output"] == REPLY,
                   timeout=10)
        desktop.set_clipboard(b"previous isolated clipboard value")
        desktop.type(window, "/copy")
        desktop.key(window, "Return")
        eventually("the exact reply in the X11 clipboard", lambda: desktop.clipboard() == REPLY.encode(), timeout=10)
        checks["copy"] = {"command": "/copy", "clipboard_utf8_sha256": hashlib.sha256(REPLY.encode()).hexdigest(),
                          "clipboard_text": REPLY, "screenshot": desktop.screen("07-reply-copied")}
        client.provider.assert_idle(duration=0.25)

        checks["menus"] = capture_workspace_menus(desktop, window)
        desktop.type(window, "Review the CSV checks")
        desktop.key(window, "Return")
        overview = csv_review(client, workspace, client.provider.next("Review the CSV checks"))
        eventually("the review to commit", lambda: client.snapshot(workspace, session)["last_output"] == overview)
        checks["layout"] = {"screenshot": desktop.screen("08-turns-tools-table")}
        desktop.key(window, "ctrl+plus")
        eventually("a narrow terminal grid for the review", lambda: larger_font(pid, original))
        checks["layout"]["narrow"] = desktop.screen("09-narrow-table")

        desktop.type(window, "/quit")
        desktop.key(window, "Return")
        assert terminal.wait(timeout=10) == 0
        code = json.loads(state.read_text(encoding="utf-8"))["exit_code"]
        assert code == 0, f"Kuku exited with {code}"
        checks["exit"] = {"exit_code": code}


def environments(base):
    for name, suffix in (("HOME", "home"), ("XDG_RUNTIME_DIR", "runtime"), ("XDG_CACHE_HOME", "cache"),
                         ("XDG_CONFIG_HOME", "config"), ("XDG_DATA_HOME", "data")):
        path = base / suffix
        path.mkdir(mode=0o700)
        os.environ[name] = str(path)
    authority = base / "Xauthority"
    authority.touch(mode=0o600)
    os.environ.update(DISPLAY=DISPLAY, XAUTHORITY=str(authority), IBUS_ADDRESS=f"unix:path={base / 'runtime' / 'ibus'}")
    command("xauth", "source", "-", data=f"add {DISPLAY} MIT-MAGIC-COOKIE-1 {secrets.token_hex(16)}\n".encode())


def main():
    if not Path("/.dockerenv").is_file():
        raise RuntimeError("The desktop scenario requires its isolated Docker container")
    result = {"started_at": now(), "kernel": os.uname().release, "display": SCREEN_SIZE, "checks": {},
              "screenshots": [], "passed": False}
    stage = "environment"
    try:
        with (TEST / "kuku").open("rb") as stream:
            result["binary_sha256"] = hashlib.file_digest(stream, "sha256").hexdigest()
        with (TEST / "bridge").open("rb") as stream:
            result["bridge_sha256"] = hashlib.file_digest(stream, "sha256").hexdigest()
        packages = command("dpkg-query", "-W", "-f=${Package} ${Version}\n", "xfce4-terminal", "libvte-2.91-0",
                           "openbox", "xvfb", "ibus", "ibus-libpinyin").stdout
        result["packages"] = packages.decode().splitlines()
        with tempfile.TemporaryDirectory(prefix="kuku-desktop-") as temporary:
            base = Path(temporary)
            environments(base)
            desktop = Desktop(base, result["screenshots"])
            ibus = ["ibus-daemon", "--replace", "--xim", "--desktop=xfce", "--panel=/usr/libexec/ibus-ui-gtk3",
                    f"--address={os.environ['IBUS_ADDRESS']}"]
            with process(["Xvfb", DISPLAY, "-screen", "0", SCREEN_SIZE, "-nolisten", "tcp", "-auth", os.environ["XAUTHORITY"]]):
                eventually("the private X display", lambda: command("xdpyinfo", check=False).returncode == 0, timeout=8)
                with process(["openbox"]), process(ibus):
                    eventually("the IBus Pinyin engine", lambda: b"libpinyin" in
                               command("ibus", "list-engine", check=False).stdout, timeout=12)
                    stage = "scenario"
                    with InstalledClient(TEST / "kuku", TEST / "bridge", base / "test") as client:
                        exercise(client, desktop, result["checks"])
        result["passed"] = True
    except Exception as error:
        result["failure"] = {"stage": stage, "message": str(error)[:4096]}
        raise
    finally:
        result["finished_at"] = now()
        (EVIDENCE / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def terminal_child(state):
    child = subprocess.Popen([TEST / "kuku"])
    temporary = state.with_suffix(".tmp")
    temporary.write_text(json.dumps({"pid": child.pid}), encoding="utf-8")
    temporary.replace(state)
    code = child.wait()
    temporary.write_text(json.dumps({"pid": child.pid, "exit_code": code}), encoding="utf-8")
    temporary.replace(state)
    return code


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--terminal-child":
        sys.exit(terminal_child(Path(sys.argv[2])))
    main()
