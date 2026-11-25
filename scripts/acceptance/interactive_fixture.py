"""Local, deterministic boundaries for testing an installed interactive client."""

import contextlib
from dataclasses import dataclass, field
import hashlib
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from itertools import count
import json
import os
from pathlib import Path, PureWindowsPath
from platform import machine
import queue
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import traceback
from urllib.parse import quote, urlencode, urlsplit

from evidence import FailureSummary


CONFIG = Path(__file__).resolve().with_name("fixtures") / "config.toml"
TIMEOUT = 30
TERMINAL_ROWS = 32
TERMINAL_COLS = 140
TOOL_OUTPUT = "fixture-tool-output-42"
EXE = ".exe" if os.name == "nt" else ""
TOOL_PROGRAM = f"./fixture-command{EXE}"


def is_composer_heading(line):
    activity = r"(?:[\u2800-\u28ff]{2}|[↑…◇↓↳])\s+\S"
    return bool(re.match(r"^\s*[│┃]\s+" + activity, line))


def cleanup_path(path):
    if not isinstance(path, PureWindowsPath):
        return path
    absolute = str(path)
    if absolute.startswith("\\\\?\\"):
        return absolute
    if absolute.startswith("\\\\"):
        return "\\\\?\\UNC\\" + absolute[2:]
    return "\\\\?\\" + absolute


@contextlib.contextmanager
def scratch_directory(base, report, *, prefix, retain=False):
    scratch = Path(tempfile.mkdtemp(prefix=prefix, dir=base)).resolve()
    failed = False
    try:
        yield scratch
    except BaseException:
        failed = True
        raise
    finally:
        if retain:
            report["scratch_path"] = str(scratch)
        else:
            try:
                shutil.rmtree(cleanup_path(scratch))
            except OSError as error:
                report["scratch_cleanup_error"] = f"{error.__class__.__name__}: {error}"
                report["scratch_path"] = str(scratch)
                if not failed:
                    raise


def eventually(description, probe, timeout=TIMEOUT):
    deadline = time.monotonic() + timeout
    while True:
        if result := probe():
            return result
        if time.monotonic() >= deadline:
            raise AssertionError(f"Timed out waiting for {description}")
        time.sleep(0.04)


def run_cases(cases):
    failed = []
    details = bytearray()
    for name, operation in cases:
        started = time.monotonic()
        try:
            operation()
        except Exception:
            failed.append(name)
            context = f"\n{name}:\n{traceback.format_exc()}".encode("utf-8")
            remaining = FailureSummary.LIMIT - len(details)
            details.extend(context[:remaining])
            print(f"Acceptance case failed: {name}", file=sys.stderr, flush=True)
            traceback.print_exc()
        finally:
            print(f"Acceptance case finished: {name} ({time.monotonic() - started:.3f}s)", flush=True)
    if failed:
        context = details.decode("utf-8", errors="ignore")
        if len(details) == FailureSummary.LIMIT:
            context += "\n[Failure details reached the retained byte limit]"
        raise AssertionError(f"Acceptance cases failed: {', '.join(failed)}\n{context}")


@dataclass
class StreamPause:
    arrived: threading.Event = field(default_factory=threading.Event)
    release: threading.Event = field(default_factory=threading.Event)
    flushed_at_ns: int = 0


@dataclass(frozen=True)
class Response:
    status: int
    body: bytes
    content_type: str = "application/json"
    pauses: tuple[tuple[int, StreamPause], ...] = ()

    def as_json(self, status=200):
        assert self.status == status, f"Expected HTTP {status}, got {self.status}"
        return json.loads(self.body)


def rendered_running_transcript(screen, height, markers):
    rows = screen.splitlines()
    return (
        len(rows) == height
        and any(is_composer_heading(row) and any(label in row for label in ("Tools", "Running"))
                for row in rows[-6:-3])
        and all(marker in screen for marker in markers)
    )


def page_up_after_rendered_resize(terminal, read_while, height, markers):
    screen = read_while(
        terminal,
        lambda value: rendered_running_transcript(value, height, markers),
        f"the rendered {height}-row running transcript before PageUp",
    )
    terminal.press("\x1b[5~")
    return screen


def has_standalone_assistant_rail(frame):
    assistant_row = next(
        (index for index, line in enumerate(frame)
         if line.strip() == "Assistant" or line.lstrip().startswith("╭─ Assistant ")),
        None,
    )
    start = assistant_row + 1 if assistant_row is not None else 0
    return any(line.strip() in {"│", "┃"} for line in frame[start:])


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
                start = 0
                for offset, pause in response.pauses:
                    self.wfile.write(response.body[start:offset])
                    self.wfile.flush()
                    pause.flushed_at_ns = time.perf_counter_ns()
                    pause.arrived.set()
                    if not pause.release.wait(TIMEOUT):
                        return
                    start = offset
                self.wfile.write(response.body[start:])

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
        self.completed_job = None

    def expect(self, text):
        assert text in json.dumps(self.request["messages"], ensure_ascii=False), (
            f"Provider request missing fixture message or tool ID {text!r}"
        )
        return self

    def tool_result(self, call_id, *, error=False):
        results = [
            block for message in self.request["messages"]
            if isinstance(message.get("content"), list) for block in message["content"]
            if block.get("type") == "tool_result" and block.get("tool_use_id") == call_id
        ]
        assert len(results) == 1, f"Expected one tool result for {call_id}, got {len(results)}"
        result = results[0]
        if error:
            assert result.get("is_error", False), f"Tool {call_id} should have failed: {result}"
        else:
            assert not result.get("is_error", False), f"Tool {call_id} failed: {result}"
        return result

    def expect_tool_output(self, call_id, output):
        result = self.tool_result(call_id)
        assert output in json.dumps(result["content"], ensure_ascii=False), (
            f"Tool {call_id} did not return {output!r}: {result}"
        )
        return self

    def reply(self, content, usage=None, *, pauses=(), chunk_size=None):
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
        offsets = ()
        if pauses:
            starts = list(re.finditer(rb"^event: content_block_start\n", stream, re.MULTILINE))
            assert len(starts) == len(pauses) + 1, "Each streamed fixture segment needs an SSE content block"
            offsets = tuple((start.start(), pause) for start, pause in zip(starts[1:], pauses))
        self.response.put_nowait(Response(200, stream, "text/event-stream", offsets))

    @contextlib.contextmanager
    def streaming_text(self, *parts, usage=None, chunk_size=None):
        assert len(parts) >= 2
        pauses = tuple(StreamPause() for _ in parts[1:])
        self.reply([{"type": "text", "text": part} for part in parts], usage, pauses=pauses, chunk_size=chunk_size)
        try:
            yield pauses
        finally:
            for pause in pauses:
                pause.release.set()

    def text(self, text, *, usage=None):
        self.reply([{"type": "text", "text": text}], usage)

    def tool(self, tool_name, call_id, **inputs):
        self.reply([{"type": "tool_use", "id": call_id, "name": tool_name, "input": inputs}])

    def fail(self):
        self.response.put_nowait(json_response({
            "type": "error", "error": {"type": "invalid_request_error", "message": "fixture provider failure"},
        }, 400))


def desktop_wheel(client, workspace, type_text, wheel_up, latest, enter, capture):
    prompt = "keep this wheel draft"
    type_text(prompt)
    before = capture("wheel-latest")
    wheel_up()
    reading = capture("wheel-reading")
    latest()
    enter()
    call = client.provider.next(prompt)
    assert any(block.get("text") == prompt for block in call.request["messages"][-1]["content"])
    reply = "Wheel draft preserved"
    call.text(reply)
    session = client.session(workspace)
    eventually("wheel test to settle", lambda: client.snapshot(workspace, session)["last_output"] == reply)
    return {"submitted_draft": prompt, "screenshots": [before, reading]}


def csv_review(client, workspace, call):
    (workspace / "example.csv").write_text('group,value\n"core, io",4\n', encoding="utf-8")
    call.tool("read_file", "read-marker", path="example.csv", brief="读取 CSV 样例")
    call = client.provider.next("read-marker")
    reply = "读取结果\n\n| 测试 | 结果 |\n| --- | --- |\n| 基础记录 | 通过 |\n| 引号字段 | 失败 |"
    call.text(reply)
    return reply


def delegate_reviewer(provider, prompt, instructions, task):
    provider.next(prompt).tool(
        "create_agent", "create-reviewer", address="reviewer",
        instructions=instructions,
    )
    provider.next("create-reviewer").tool(
        "send_message", "send-reviewer", to="reviewer", message=task,
    )
    first, second = provider.next(), provider.next()
    if "send-reviewer" in json.dumps(first.request["messages"]):
        parent, child = first, second
    else:
        parent, child = second, first
    parent.expect("send-reviewer")
    child.expect(task)
    child_input = json.dumps(child.request["messages"])
    assert prompt not in child_input and "send-reviewer" not in child_input, (
        "The reviewer must receive the delegated input in its own context"
    )
    return parent, child


class DenseTranscript:
    prompt = "review sample workspace"
    draft = "draft survives the review"
    heading = "工作区核对结果"
    source = "fn verified_total"
    end = "原始报告校验完成"
    activity_group = "12 file reads · 2 file changes"
    missing = "samples/missing-proof.txt"
    read_target = "samples/read-12.txt"
    file_cards = (
        "Added    generated.txt (+1 -0)", "Edited   samples/read-01.txt (+1 -1)",
    )
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
        return self.client.completed_tool(self.workspace, "dense-gate").streaming_text(self.partial, self.tail, chunk_size=8)

    def desktop(self, type_text, enter, capture_widths, copy_reply, toggle_details, *, gate_executable=None):
        type_text(self.prompt)
        enter()
        with self.running_tool(executable=gate_executable):
            type_text(self.draft)
            screens = {"running_tools": capture_widths("dense-running-tools")}
            toggle_details()
            screens["expanded_activity"] = capture_widths("dense-expanded-activity")
            toggle_details()
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
        return {"tool_calls": len(calls), "failed_calls": 1, "reply_bytes": len(self.reply.encode()),
                "reply_sha256": hashlib.sha256(self.reply.encode()).hexdigest(), "complete_saved_output": True}

    def settled_snapshot(self):
        snapshot = self.client.snapshot(self.workspace, self.session)
        return snapshot if snapshot["status"] == "idle" and snapshot["last_output"] == self.reply else None


class MarkdownDocument:
    prompt = "Show the complete Markdown document"
    draft = "keep this Markdown review draft"
    example = (
        "这份核对报告可以原样复制。\n\n````md\n"
        + DenseTranscript.reply
        + "\n\n```python\n"
        "def review_total(values: list[int]) -> int:\n"
        "    return sum(value for value in values if value > 0)\n"
        "```\n````\n\n报告之外的说明保留。"
    )

    def __init__(self, source=None):
        self.source = self.example if source is None else source
        opening = re.search(r"(?m)^`{4,}(markdown|md)[ \t]*\r?\n", self.source)
        assert opening, "The fixture needs an explicit Markdown document fence"
        python = self.source.index("\n```python", opening.end()) + 1
        closing = self.source.index("\n```", python) + 1
        lines = self.source[python:closing].splitlines(keepends=True)
        pause = python + len("".join(lines[:8]))
        self.parts = (self.source[:opening.end()], self.source[opening.end():pause], self.source[pause:])
        self.fence = opening.group().strip()
        self.label = opening[1]
        self.intro = self.parts[0].splitlines()[0][:24]
        self.heading = next(line for line in self.parts[1].splitlines() if line.startswith("# ") or line.startswith("## "))
        self.list_item = re.search(r"(?m)^1\. (.+)$", self.parts[1])[1]
        self.table = next(line for line in self.parts[1].splitlines() if line.startswith("|"))
        self.columns = [cell.strip() for cell in self.table.strip("|").split("|")]
        self.code_anchor = self.parts[1].rstrip().splitlines()[-1].strip()
        self.trailer = self.source.rstrip().splitlines()[-1][:24]

    def languages(self, complete):
        body = self.source if complete else self.parts[1]
        return re.findall(r"(?m)^```([\w-]+)\s*$", body)

    def stream(self, provider):
        return provider.next(self.prompt).streaming_text(*self.parts, chunk_size=8)

    def wait_for_body(self, client, workspace):
        session = client.session(workspace)
        params = urlencode({"workspace": workspace, "agent": "main", "progress": "true"})
        path = f"/sessions/{session}/snapshot?{params}"
        expected = "".join(self.parts[:2])
        eventually("the held Markdown prefix in the server's live reply",
                   lambda: (client.request("GET", path).as_json().get("reply_preview") or {}).get("text") == expected)

    def verify(self, client, workspace):
        session = client.session(workspace)

        def complete():
            snapshot = client.snapshot(workspace, session)
            return snapshot if snapshot["status"] == "idle" and snapshot["last_output"] == self.source else None

        snapshot = eventually("the original Markdown document in the durable reply", complete)
        assert snapshot["pending_inputs"] == 0
        raw = self.source.encode("utf-8")
        return {"reply_bytes": len(raw), "reply_sha256": hashlib.sha256(raw).hexdigest()}


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
    def __init__(self, bridge, binary, workspace, env, args=(), *, confirm_permissions=True):
        self.rows = TERMINAL_ROWS
        self.cols = TERMINAL_COLS
        self.confirm_permissions = confirm_permissions
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
        response = self.receive()
        if operation == "resize":
            self.rows = response["rows"]
            self.cols = response["cols"]
        elif operation == "screen":
            response["rows"] = self.rows
            response["cols"] = self.cols
        return response

    def press(self, text):
        self.request("write", text=text)

    def click_text(self, screen, text, cell_offset=0):
        row, line = next((row, line) for row, line in enumerate(screen.splitlines(), 1) if text in line)
        prefix = line[:line.index(text)]
        column = len(prefix) + 1 + cell_offset
        self.press(f"\x1b[<0;{column};{row}M\x1b[<0;{column};{row}m")
        return {"row": row, "column": column}

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

    def expect(self, text, state=None, *, also=(), absent=(), rows=None):
        viewport = {}
        required = (text, *also) if state is None else (text, state, *also)

        def contains_text():
            viewport.update(self.request("screen"))
            screen = viewport["screen"]
            if (self.confirm_permissions and "Workspace permissions" in screen
                    and "› Full access" in screen and "· current" not in screen):
                self.press("\r")
                return None
            if rows is not None and screen.count("\n") + 1 != rows:
                return None
            return viewport if (all(item in screen for item in required)
                                and not any(item in screen for item in absent)) else None

        try:
            return eventually(f"terminal text {required!r}, absent {absent!r}, at {rows or 'any'} rows", contains_text)
        except AssertionError as error:
            raise AssertionError(f"{error}\nLast terminal screen:\n{viewport.get('screen', '')}") from error

    def expect_first_model_settings(self):
        self.expect("Set up a model", also=("Find and test models", "Edit config file"),
                    absent=("kuku service offline", "F2 Reconnect"))

    def exit_cleanly(self, keys="/quit\r"):
        self.press(keys)

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
    def __init__(self, binary, bridge, directory, *, reopen=False):
        self.binary = binary
        self.bridge = bridge
        self.directory = directory
        self.home = directory / "data"
        if reopen:
            if not (self.home / "config.toml").is_file():
                raise ValueError(f"No saved acceptance Session data in {directory}")
        else:
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

    def workspace(self, name, *, permissions=None):
        workspace = self.directory / name
        workspace.mkdir()
        if permissions is not None:
            self.cli(workspace, "list")  # Start the managed service without opening a Session.
            query = urlencode({"workspace": workspace})
            response = self.request("PUT", f"/workspaces/permissions?{query}", payload=permissions)
            assert response.as_json() == permissions
        return workspace

    def terminal(self, workspace, *args, color=False, confirm_permissions=True):
        env = self.env
        if color:
            env = {key: value for key, value in self.env.items() if key not in ("NO_COLOR", "CLICOLOR")}
            env["COLORTERM"] = "truecolor"
        return Terminal(self.bridge, self.binary, workspace, env, args,
                        confirm_permissions=confirm_permissions)

    @contextlib.contextmanager
    def command(self, workspace, *args, token=None, stdin=subprocess.DEVNULL):
        env = self.env if token is None else {**self.env, "KUKU_TOKEN": token}
        process = subprocess.Popen(
            [self.binary, *args], cwd=workspace, env=env, stdin=stdin,
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

    def snapshot(self, workspace, session, agent="main"):
        query = urlencode({"workspace": workspace, "agent": agent})
        return self.request("GET", f"/sessions/{session}/snapshot?{query}").as_json()["agent"]

    def assert_current_service(self, connection):
        current = self.connection()
        assert current["pid"] == connection["pid"], "Managed service PID changed"
        assert current["instance_id"] == connection["instance_id"], "Managed service identity changed"

    def gated_tool(self, workspace, call, call_id, *, executable=None, background=False):
        if executable is None:
            fixture = workspace / f"fixture-command{EXE}"
            shutil.copy2(self.bridge, fixture)
            program = f"./{fixture.name}"
        else:
            program = str(executable)
        self.tool_workspaces.append(workspace)
        call.tool("run_command", call_id, program=program, args=["--gated-tool"], background=background)
        eventually("gated tool dispatch", lambda: (workspace / "tool-attempts.txt").exists())

    def completed_tool(self, workspace, call_id):
        call = self.provider.next(call_id)
        content = call.tool_result(call_id)["content"]
        handle = json.loads(content) if content.startswith("{") else {}
        if handle.get("kind") != "job":
            return call.expect_tool_output(call_id, TOOL_OUTPUT)
        job_id = handle["job_id"]
        wait_id = f"{call_id}-wait"
        call.tool("wait_job", wait_id, job_id=job_id)
        call = self.provider.next(wait_id).expect_tool_output(wait_id, TOOL_OUTPUT)
        query = urlencode({"workspace": workspace, "agent": "main"})
        session = self.session(workspace)
        job = self.request("GET", f"/sessions/{session}/jobs/{job_id}?{query}").as_json()
        assert job["tool_call_id"] == call_id and job["status"] == "completed", job
        assert job["result"]["status"] == "ok", job
        facts = job["result"]["structured"]
        assert facts["exit_code"] == 0 and facts["capture_complete"] and not facts["cleanup_incomplete"], job
        assert TOOL_OUTPUT in facts["output_preview"], job
        call.completed_job = job
        return call

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
    if sys.platform == "linux" and machine() in ("aarch64", "arm64"):
        platform = "linux-aarch64"
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
