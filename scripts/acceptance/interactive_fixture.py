"""Local, deterministic boundaries for testing an installed interactive client."""

import contextlib
from dataclasses import dataclass
import hashlib
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from itertools import count
import json
import os
from pathlib import Path
import queue
import shlex
import shutil
import signal
import subprocess
import sys
import threading
import time
from urllib.parse import quote, urlencode, urlsplit


CONFIG = Path(__file__).resolve().with_name("fixtures") / "config.toml"
TIMEOUT = 30
TOOL_BRIEF = "Run installed acceptance fixture"
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
    pause_at: tuple[int, threading.Event] | None = None

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
            with contextlib.suppress(ConnectionError):
                self.send_response(response.status)
                self.send_header("Content-Type", response.content_type)
                self.send_header("Content-Length", str(len(response.body)))
                self.end_headers()
                if response.pause_at is None:
                    self.wfile.write(response.body)
                else:
                    offset, release = response.pause_at
                    self.wfile.write(response.body[:offset])
                    self.wfile.flush()
                    if release.wait(TIMEOUT):
                        self.wfile.write(response.body[offset:])

        do_GET = do_POST = handle_request

        def log_message(self, _format, *_args):
            pass

    with serve(Handler) as endpoint:
        yield endpoint


@contextlib.contextmanager
def serve(handler):
    with ThreadingHTTPServer(("127.0.0.1", 0), handler) as server:
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

    def reply(self, content, usage=None, release=None, chunk_size=None):
        has_tool = any(block["type"] == "tool_use" for block in content)
        message = {
            "id": self.id, "type": "message", "role": "assistant", "content": content,
            "stop_reason": "tool_use" if has_tool else "end_turn",
            "usage": usage if usage is not None else {
                "input_tokens": 8, "output_tokens": 5,
                "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0,
            },
        }
        result = subprocess.run([self.bridge, "--anthropic-response"],
                                input=json.dumps(message).encode("utf-8"),
                                capture_output=True, timeout=TIMEOUT, check=True)
        stream = result.stdout
        if chunk_size:
            records = []
            for record in stream.split(b"\n\n"):
                if record.startswith(b"event: content_block_delta\n"):
                    _, data = record.split(b"\ndata: ", 1)
                    event = json.loads(data)
                    text = event["delta"]["text"]
                    for offset in range(0, len(text), chunk_size):
                        event["delta"]["text"] = text[offset:offset + chunk_size]
                        records.append(b"event: content_block_delta\ndata: " + json.dumps(event).encode())
                else:
                    records.append(record)
            stream = b"\n\n".join(records)
        pause_at = None
        if release is not None:
            marker = b"event: content_block_start"
            offset = stream.index(marker, stream.index(marker) + len(marker))
            pause_at = (offset, release)
        self.response.put_nowait(Response(200, stream, "text/event-stream", pause_at))

    @contextlib.contextmanager
    def streaming_text(self, first, last, *, usage=None, chunk_size=None):
        release = threading.Event()
        self.reply([{"type": "text", "text": first}, {"type": "text", "text": last}], usage, release, chunk_size)
        try:
            yield
        finally:
            release.set()

    def text(self, text, *, usage=None):
        self.reply([{"type": "text", "text": text}], usage)

    def tool(self, name, call_id, **inputs):
        self.reply([{"type": "tool_use", "id": call_id, "name": name, "input": inputs}])

    def fail(self):
        self.response.put_nowait(json_response({
            "type": "error", "error": {"type": "invalid_request_error", "message": "fixture provider failure"},
        }, 400))


def csv_review(client, workspace, call):
    (workspace / "example.csv").write_text('group,value\n"core, io",4\n', encoding="utf-8")
    call.tool("read_file", "read-marker", path="example.csv", brief="读取 CSV 样例")
    call = client.provider.next("read-marker")
    reply = "读取结果\n\n| 测试 | 结果 |\n| --- | --- |\n| 基础记录 | 通过 |\n| 引号字段 | 失败 |"
    call.text(reply)
    return reply


class DenseTranscript:
    prompt = "review sample workspace"
    draft = "draft survives the review"
    heading = "工作区核对结果"
    source = "fn verified_total"
    end = "原始报告校验完成"
    read_group = "Read 12 files · ok"
    missing = "samples/missing-proof.txt"
    read_target = "samples/read-12.txt"
    partial = (
        f"## {heading}\n\n"
        "这次检查把读取、搜索和修改分开记录。每个样例文件只提供一条标记，"
        "所以即使多个调用同时完成，也能根据文件名和正文核对结果。"
        "预期缺失的文件单独报告，后续步骤继续使用已验证的样例。\n\n"
        "写入生成文件后，再更新第一份样例的标记。检查命令等待读者确认，"
        "释放后把完整输出留在会话中。下次打开终端时也可以查看当时的输出。\n\n"
        "```rust\n"
        "fn verified_total(values: &[u32]) -> u32 {\n"
        "    let retained_sample_values = values.iter().copied().filter(|value| *value > 0);\n"
        "    retained_sample_values.sum()\n"
        "}\n"
    )
    tail = (
        "```\n\n"
        "### 检查结论\n\n"
        "1. 已读取十二份样例，保留各文件的独立结果。\n"
        "2. 缺失文件有明确错误，生成文件与检查命令结果均可复查。\n\n"
        "| 验证项目 | 结果 |\n"
        "| --- | --- |\n"
        "| 样例读取 | 十二项通过 |\n"
        "| 缺失路径 | 已记录错误 |\n"
        "| 命令检查 | 完整输出已保存 |\n\n"
        "```fixture-log\n"
        "dense-literal verification saved\n"
        "```\n\n"
        f"{end}"
    )
    reply = partial + tail

    def __init__(self, client, workspace):
        self.client = client
        self.workspace = workspace
        samples = workspace / "samples"
        samples.mkdir()
        self.reads = []
        for index in range(1, 13):
            filename = f"read-{index:02d}.txt"
            (samples / filename).write_text(f"sample-marker {index:02d}\n", encoding="utf-8")
            self.reads.append(("read_file", f"dense-read-{index:02d}", {"path": f"samples/{filename}", "limit": 1}))
        self.other_tools = [
            ("find_files", "dense-find-first", {"path": "samples", "pattern": "read-0*.txt"}),
            ("find_files", "dense-find-last", {"path": "samples", "pattern": "read-1*.txt"}),
            ("find_files", "dense-find-all", {"path": "samples", "pattern": "*.txt"}),
            ("search_text", "dense-search-markers", {"path": "samples", "pattern": "sample-marker", "view": "count"}),
            ("search_text", "dense-search-first", {"path": "samples", "pattern": "sample-marker 01", "view": "lines"}),
            ("write_file", "dense-write", {"path": "generated.txt", "content": "dense review verified\n", "brief": "Write audit result"}),
            ("edit_file", "dense-edit", {"path": "samples/read-01.txt", "old_text": "sample-marker 01",
                                          "new_text": "sample-reviewed 01", "brief": "Update reviewed sample"}),
            ("read_file", "dense-missing", {"path": self.missing}),
        ]
        self.session = None

    @contextlib.contextmanager
    def running_tool(self, *, executable=None):
        provider = self.client.provider
        call = provider.next(self.prompt)
        for batch in (self.reads, self.other_tools):
            call.reply([{"type": "tool_use", "id": identity, "name": tool, "input": args}
                        for tool, identity, args in batch])
            call = provider.next(batch[-1][1])
        self.client.gated_tool(self.workspace, call, "dense-gate", executable=executable)
        self.session = self.client.session(self.workspace)
        try:
            yield
        finally:
            self.client.release_tool(self.workspace)

    def streaming_reply(self):
        return self.client.provider.next("dense-gate").streaming_text(self.partial, self.tail, chunk_size=8)

    def desktop(self, type_text, enter, capture_widths, copy_reply, *, gate_executable=None):
        type_text(self.prompt)
        enter()
        with self.running_tool(executable=gate_executable):
            type_text(self.draft)
            screens = {"running_tools": capture_widths("dense-running-tools")}
        with self.streaming_reply():
            screens["open_code"] = capture_widths("dense-open-code")
        verification = self.verify()
        screens["complete_report"] = capture_widths("dense-complete-report")
        copy_reply(self.reply)
        self.client.provider.assert_idle()
        return {**verification, "clipboard_matches_reply": True, "screens": screens}

    def verify(self):
        client, workspace, session = self.client, self.workspace, self.session
        snapshot = eventually("the complete Chinese report to persist",
                              lambda: self.settled_snapshot())
        assert snapshot["pending_inputs"] == 0, "The draft should remain local"
        client.verify_tool(workspace)
        assert (workspace / "generated.txt").read_text(encoding="utf-8") == "dense review verified\n"
        assert (workspace / "samples" / "read-01.txt").read_text(encoding="utf-8") == "sample-reviewed 01\n"
        assert not (workspace / self.missing).exists()
        query = urlencode({"workspace": workspace, "agent": "main", "limit": 64})
        entries = client.request("GET", f"/sessions/{session}/history?{query}").as_json()["entries"]
        calls = {entry["tool_call_id"]: entry for entry in entries if entry["type"] == "tool_call"}
        results = {entry["tool_call_id"]: entry for entry in entries if entry["type"] == "tool_result"}
        expected = {identity: (tool, args) for tool, identity, args in self.reads + self.other_tools}
        assert len(expected) + 1 >= 20
        for identity, (tool, args) in expected.items():
            assert calls[identity]["tool"] == tool and calls[identity]["args"] == args
            expected_status = "error" if identity == "dense-missing" else "ok"
            assert results[identity]["status"] == expected_status, (identity, results[identity]["summary"])
        assert self.missing in results["dense-missing"]["model_content"]
        assert "sample-marker 12" in results["dense-read-12"]["model_content"]
        gate = results["dense-gate"]
        assert calls["dense-gate"]["tool"] == "run_command" and gate["status"] == "ok"
        output_id = quote(gate["output_id"], safe="")
        query = urlencode({"workspace": workspace, "offset": 0, "max_bytes": 4096})
        saved = client.request("GET", f"/sessions/{session}/outputs/{output_id}?{query}").as_json()["output"]
        assert saved["complete"] and not saved["capture_truncated"] and not saved["has_more"]
        assert "".join(segment["text"] for segment in saved["segments"]).splitlines() == [TOOL_OUTPUT]
        client.provider.assert_idle()
        return {"tool_calls": len(expected) + 1, "failed_calls": 1, "reply_bytes": len(self.reply.encode()),
                "reply_sha256": hashlib.sha256(self.reply.encode()).hexdigest(), "complete_saved_output": True}

    def settled_snapshot(self):
        snapshot = self.client.snapshot(self.workspace, self.session)
        return snapshot if snapshot["status"] == "idle" and snapshot["last_output"] == self.reply else None


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

    def next(self, expected=None):
        try:
            call = self.calls.get(timeout=TIMEOUT)
        except queue.Empty as error:
            raise AssertionError(f"No provider call for {expected!r}") from error
        return call if expected is None else call.expect(expected)

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
        self.press("\x1bOPquit\r")

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

    @contextlib.contextmanager
    def full_footer(self):
        path = self.home / "tui.toml"
        fields = ["connection", "execution", "agent", "workspace", "model", "context",
                  "tokens_in", "tokens_out", "cache", "tps"]
        assert not path.exists()
        path.write_text("[statusline]\nfields = " + json.dumps(fields) + "\n", encoding="utf-8")
        try:
            yield
        finally:
            path.unlink()

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
        timed_out = False
        try:
            yield process
        except subprocess.TimeoutExpired:
            timed_out = True
            raise
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
            # Windows communicate readers can still own the pipes until the test service is stopped.
            if not timed_out:
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

    def gated_tool(self, workspace, call, call_id, *, executable=None):
        if executable is None:
            fixture = workspace / f"fixture-command{EXE}"
            shutil.copy2(self.bridge, fixture)
            command = fixture.name if os.name == "nt" else f"./{fixture.name}"
        else:
            assert os.name != "nt", "Use the workspace-local fixture executable on Windows"
            command = shlex.quote(str(executable))
        self.tool_workspaces.append(workspace)
        call.tool("run_command", call_id, command=f"{command} --gated-tool", timeout=95, brief=TOOL_BRIEF)
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


def install(archive, home, env):
    platform = "windows-x86_64" if os.name == "nt" else (
        "darwin-aarch64" if sys.platform == "darwin" else "linux-x86_64")
    with archive.open("rb") as source:
        digest = hashlib.sha256()
        for block in iter(lambda: source.read(1 << 20), b""):
            digest.update(block)
    sha256 = digest.hexdigest()

    class Release(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            if self.path == "/latest.json":
                body = json.dumps({"version": "acceptance", "platforms": {platform: {
                    "url": f"http://127.0.0.1:{self.server.server_port}/package", "sha256": sha256,
                }}}).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            elif self.path == "/package":
                self.send_response(200)
                self.send_header("Content-Length", str(archive.stat().st_size))
                self.end_headers()
                with archive.open("rb") as handle:
                    shutil.copyfileobj(handle, self.wfile)
            else:
                self.send_error(404)

    scripts = Path(__file__).resolve().parents[1]
    env = {**env, "KUKU_HOME": str(home), "PATH": str(home / "bin") + os.pathsep + env["PATH"]}
    with serve(Release) as address:
        cmd = (["powershell", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                "-File", str(scripts / "install.ps1"), "-Force", "-NoModifyPath", "-Source"]
               if os.name == "nt" else ["sh", str(scripts / "install.sh"), "--force", "--source"])
        subprocess.run([*cmd, address + "/latest.json"], cwd=home, env=env, check=True,
                       stdout=subprocess.DEVNULL, timeout=90)
