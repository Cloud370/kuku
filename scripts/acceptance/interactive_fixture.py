"""Local, deterministic boundaries for testing an installed interactive client."""

import contextlib
from dataclasses import dataclass
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from itertools import count
import json
import os
from pathlib import Path
import queue
import shutil
import signal
import subprocess
import threading
import time
from urllib.parse import urlencode, urlsplit


CONFIG = Path(__file__).resolve().with_name("fixtures") / "config.toml"
TIMEOUT = 30
TOOL_OUTPUT = "fixture-tool-output-42"
EXE = ".exe" if os.name == "nt" else ""


def eventually(description, probe, timeout=TIMEOUT):
    deadline = time.monotonic() + timeout
    while True:
        if result := probe():
            return result
        if time.monotonic() >= deadline:
            raise AssertionError(f"Timed out waiting for {description}")
        time.sleep(0.04)


@dataclass(frozen=True)
class Response:
    status: int
    body: bytes
    content_type: str = "application/json"

    def as_json(self, status=200):
        assert self.status == status, f"Expected HTTP {status}, got {self.status}"
        return json.loads(self.body)


def json_response(value, status=200):
    return Response(status, json.dumps(value).encode("utf-8"))


def local_request(address, method, path, *, body=None, headers=None):
    url = urlsplit(address if "://" in address else f"http://{address}")
    if url.scheme != "http" or url.hostname != "127.0.0.1" or url.port is None:
        raise ValueError("Acceptance HTTP requests require a loopback fixture or service")
    connection = http.client.HTTPConnection(url.hostname, url.port, timeout=3)
    try:
        connection.request(method, path, body=body, headers=headers or {})
        response = connection.getresponse()
        return Response(response.status, response.read(), response.getheader("Content-Type", ""))
    finally:
        connection.close()


@contextlib.contextmanager
def http_fixture(dispatch):
    class Handler(BaseHTTPRequestHandler):
        def handle_request(self):
            body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            response = dispatch(self.command, self.path, self.headers, body)
            if response is None:
                self.close_connection = True
                return
            with contextlib.suppress(BrokenPipeError, ConnectionResetError):
                self.send_response(response.status)
                self.send_header("Content-Type", response.content_type)
                self.send_header("Content-Length", str(len(response.body)))
                self.end_headers()
                self.wfile.write(response.body)

        do_GET = do_POST = handle_request

        def log_message(self, _format, *_args):
            pass

    with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
        thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05})
        thread.start()
        try:
            yield f"http://127.0.0.1:{server.server_port}"
        finally:
            server.shutdown()
            thread.join()


class ProviderCall:
    def __init__(self, body, bridge, index):
        self.request = json.loads(body)
        self.bridge = bridge
        self.id = f"msg_fixture_{index}"
        self.response = queue.Queue(maxsize=1)

    def expect(self, text):
        assert text in json.dumps(self.request["messages"], ensure_ascii=False), (
            f"Provider request missing fixture message or tool ID {text!r}"
        )
        return self

    def reply(self, content):
        has_tool = any(block["type"] == "tool_use" for block in content)
        message = {
            "id": self.id, "type": "message", "role": "assistant", "content": content,
            "stop_reason": "tool_use" if has_tool else "end_turn",
            "usage": {"input_tokens": 8, "output_tokens": 5},
        }
        result = subprocess.run([self.bridge, "--anthropic-response"],
                                input=json.dumps(message).encode("utf-8"),
                                capture_output=True, timeout=TIMEOUT, check=True)
        self.response.put_nowait(Response(200, result.stdout, "text/event-stream"))

    def text(self, text):
        self.reply([{"type": "text", "text": text}])

    def tool(self, name, call_id, **inputs):
        self.reply([{"type": "tool_use", "id": call_id, "name": name, "input": inputs}])

    def fail(self):
        self.response.put_nowait(json_response({
            "type": "error", "error": {"type": "invalid_request_error", "message": "fixture provider failure"},
        }, 400))


class Provider:
    def __init__(self, bridge):
        self.bridge = bridge
        self.calls = queue.Queue()
        self.sequence = count(1)

    def dispatch(self, method, path, _headers, body):
        if method != "POST" or path != "/v1/messages":
            return json_response({}, 404)
        call = ProviderCall(body, self.bridge, next(self.sequence))
        self.calls.put(call)
        try:
            return call.response.get(timeout=2 * TIMEOUT)
        except queue.Empty:
            return json_response({}, 503)

    def next(self, expected):
        try:
            return self.calls.get(timeout=TIMEOUT).expect(expected)
        except queue.Empty as error:
            raise AssertionError(f"No provider call for {expected!r}") from error

    def assert_idle(self, duration=0):
        try:
            self.calls.get(timeout=duration)
        except queue.Empty:
            return
        raise AssertionError("Unexpected additional provider request")


class Terminal:
    def __init__(self, bridge, binary, workspace, env, args=()):
        self.process = subprocess.Popen(
            [bridge, binary, *args], cwd=workspace, env=env, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, text=True, encoding="utf-8",
        )
        self.responses = queue.Queue()
        self.reader = threading.Thread(target=self.read_responses, daemon=True)
        self.reader.start()

    def __enter__(self):
        try:
            receipt = self.receive()
            expected = "conpty" if os.name == "nt" else "unix-pty"
            assert receipt["backend"] == expected, "Native PTY backend was not used"
            return self
        except BaseException:
            self.close()
            raise

    def __exit__(self, *_):
        self.close()

    def read_responses(self):
        try:
            for line in self.process.stdout:
                self.responses.put(json.loads(line))
        finally:
            self.responses.put(None)

    def receive(self):
        try:
            value = self.responses.get(timeout=TIMEOUT)
        except queue.Empty as error:
            raise AssertionError("Native PTY did not respond") from error
        assert value is not None, "Native PTY process exited"
        return value

    def request(self, operation, **fields):
        self.process.stdin.write(json.dumps({"op": operation, **fields}) + "\n")
        self.process.stdin.flush()
        return self.receive()

    def press(self, text):
        self.request("write", text=text)

    def type(self, text):
        self.press(text)
        self.expect(text)

    def submit(self, prompt):
        self.type(prompt)
        self.press("\r")

    def resume(self, title):
        self.expect("New session", state="Ready")
        self.press("/resume\r")
        self.expect(title)
        self.press("\r")

    def expect(self, text, state=None):
        viewport = {}

        def contains_text():
            viewport.update(self.request("screen"))
            return viewport if text in viewport["screen"] and (state is None or state in viewport["screen"]) else None

        try:
            return eventually(f"terminal text {text!r} in state {state!r}", contains_text)
        except AssertionError as error:
            raise AssertionError(f"{error}\nLast terminal screen:\n{viewport.get('screen', '')}") from error

    def exit_cleanly(self):
        self.press("\x03")

        def restored():
            value = self.request("screen")
            assert value["exit_code"] in (None, 0), "Terminal client exited unsuccessfully"
            if value["exit_code"] == 0 and value["console_restored"] and not any(
                value[mode] for mode in ("alternate_screen", "cursor_hidden", "bracketed_paste")
            ):
                return value

        eventually("terminal exit with restored console modes, cursor, and paste settings", restored)

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=TIMEOUT)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=5)
            raise AssertionError("Native PTY did not close")
        finally:
            self.reader.join(timeout=5)
            self.process.stdout.close()
        assert self.process.returncode == 0, "Native PTY exited unsuccessfully"


class InstalledClient:
    def __init__(self, binary, bridge, directory):
        self.binary = binary
        self.bridge = bridge
        self.directory = directory
        self.home = directory / "data"
        self.home.mkdir(parents=True)
        self.discovery = self.home / "service" / "connection.json"
        self.tool_workspaces = []
        self.provider = Provider(bridge)
        self.env = {key: value for key, value in os.environ.items()
                    if not key.upper().startswith("KUKU_") and not key.upper().endswith("_PROXY")}
        self.env.update(KUKU_HOME=str(self.home), NO_PROXY="127.0.0.1,localhost",
                        RUST_LOG="warn", CLICOLOR="0")

    def __enter__(self):
        with contextlib.ExitStack() as startup:
            endpoint = startup.enter_context(http_fixture(self.provider.dispatch))
            config = CONFIG.read_text(encoding="utf-8").replace("MOCK_PORT", str(urlsplit(endpoint).port))
            (self.home / "config.toml").write_text(config, encoding="utf-8")
            self.fixture = startup.pop_all()
        return self

    def __exit__(self, *_):
        try:
            for workspace in self.tool_workspaces:
                self.release_tool(workspace)
            self.stop()
        finally:
            self.fixture.close()

    def workspace(self, name):
        workspace = self.directory / name
        workspace.mkdir()
        return workspace

    def terminal(self, workspace, *args):
        return Terminal(self.bridge, self.binary, workspace, self.env, args)

    @contextlib.contextmanager
    def command(self, workspace, *args, token=None):
        env = self.env if token is None else {**self.env, "KUKU_TOKEN": token}
        process = subprocess.Popen(
            [self.binary, *args], cwd=workspace, env=env, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8", errors="replace",
        )
        try:
            yield process
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
            process.stdout.close()
            process.stderr.close()

    def cli(self, workspace, *args, success=True, token=None):
        with self.command(workspace, *args, token=token) as process:
            return self.finish(process, success)

    @staticmethod
    def finish(process, success=True):
        stdout, stderr = process.communicate(timeout=TIMEOUT)
        assert (process.returncode == 0) == success, (
            f"Client returned {process.returncode}, expected success={success}\n"
            f"{stderr[:3000]}"
        )
        return stdout, stderr

    def connection(self):
        return json.loads(self.discovery.read_text(encoding="utf-8"))

    def request(self, method, path, *, payload=None, authenticated=True, connection=None):
        connection = connection or self.connection()
        headers = {"Authorization": f"Bearer {connection['token']}"} if authenticated else {}
        body = None
        if payload is not None:
            body = json.dumps(payload).encode("utf-8")
            headers["Content-Type"] = "application/json"
        return local_request(connection["address"], method, path, body=body, headers=headers)

    def status(self, connection=None):
        return self.request("GET", "/service/status", connection=connection).as_json()

    def sessions(self, workspace):
        return self.request("GET", f"/sessions?{urlencode({'workspace': workspace})}").as_json()["sessions"]

    def session(self, workspace):
        sessions = self.sessions(workspace)
        assert len(sessions) == 1, "Expected exactly one Session in the workspace"
        return sessions[0]["session_id"]

    def snapshot(self, workspace, session):
        query = urlencode({"workspace": workspace, "agent": "main"})
        return self.request("GET", f"/sessions/{session}/snapshot?{query}").as_json()["agent"]

    def assert_current_service(self, connection):
        current = self.connection()
        assert current["pid"] == connection["pid"], "Managed service PID changed"
        assert current["instance_id"] == connection["instance_id"], "Managed service identity changed"

    def gated_tool(self, workspace, call, call_id):
        fixture = workspace / f"fixture-command{EXE}"
        shutil.copy2(self.bridge, fixture)
        self.tool_workspaces.append(workspace)
        command = fixture.name if os.name == "nt" else f"./{fixture.name}"
        call.tool("run_command", call_id, command=f"{command} --gated-tool", timeout=95,
                  brief="Run installed acceptance fixture")
        eventually("gated tool dispatch", lambda: (workspace / "tool-attempts.txt").exists())

    @staticmethod
    def release_tool(workspace):
        (workspace / "release-tool").touch()

    @staticmethod
    def verify_tool(workspace):
        assert (workspace / "tool-result.txt").read_text(encoding="utf-8") == TOOL_OUTPUT
        attempts = (workspace / "tool-attempts.txt").read_text(encoding="utf-8").splitlines()
        assert [Path(path).resolve() for path in attempts] == [workspace.resolve()], (
            "Fixture command must execute exactly once in the selected workspace"
        )

    def stop(self):
        if not self.discovery.exists():
            return
        try:
            connection = self.connection()
            status = self.status(connection)
        except (OSError, http.client.HTTPException):
            return
        assert status["instance_id"] == connection["instance_id"] and status["pid"] == connection["pid"]
        if self.request("POST", "/service/restart", connection=connection).status != 202:
            self.kill(connection)
            return
        try:
            self.wait_stopped(connection)
        except AssertionError:
            self.kill(connection)
            raise

    def kill(self, connection):
        self.assert_current_service(connection)
        status = self.status(connection)
        assert status["instance_id"] == connection["instance_id"] and status["pid"] == connection["pid"]
        pid = int(status["pid"])
        if os.name == "nt":
            subprocess.run(["taskkill", "/PID", str(pid), "/F"], check=True, capture_output=True, timeout=10)
        else:
            os.kill(pid, signal.SIGKILL)
        self.wait_stopped(connection)

    def wait_stopped(self, connection):
        def ended():
            try:
                self.status(connection)
            except (OSError, http.client.HTTPException):
                return True
        eventually("isolated managed service to release its listener", ended, timeout=10)
