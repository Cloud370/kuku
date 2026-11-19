"""Run the installed TUI, WebUI and CLI on one budgeted DeepSeek development task."""

import argparse
import http.client
from http.server import BaseHTTPRequestHandler
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import threading
from urllib.parse import quote, urlencode

from interactive_browser import open_session
from interactive_fixture import InstalledClient, eventually, install, serve
from playwright.sync_api import sync_playwright

from run import Budget, ROOT, load_secrets, write_json

MODEL = "deepseek-flash"
MAX_REQUESTS = 12
MAX_BYTES = 48_000
MAX_OUTPUT = 2048
RESERVATION_USD = 0.30
PROVIDER_HOST = "api.deepseek.com"

PROMPT = (
    "Fix quoted CSV totals in report.py. Read the source and tests, then use the file editing "
    "tool to fix the implementation. Keep the public function and tests intact. "
    f"Run {'python' if os.name == 'nt' else 'python3'} check.py and git diff -- report.py after editing. "
    "After a passing check, end with the plain text CSV totals verified."
)
IMPLEMENTATION = '''def totals(lines):
    amounts = {}
    for line in lines:
        group, amount = line.rstrip().split(",")
        amounts[group] = amounts.get(group, 0) + int(amount)
    return amounts
'''
TESTS = '''import unittest

from report import totals


class TotalsTest(unittest.TestCase):
    def test_simple_records(self):
        self.assertEqual({"core": 5, "ui": 1}, totals(["core,2", "ui,1", "core,3"]))

    def test_quoted_groups(self):
        rows = ['"core, io",4', '"code ""review""",2', '"core, io",-1']
        self.assertEqual({"core, io": 3, 'code "review"': 2}, totals(rows))
'''
CHECK = '''import pathlib
import sys
import time
import unittest

pathlib.Path(".check-started").touch()
deadline = time.monotonic() + 75
while not pathlib.Path(".review-ready").exists():
    if time.monotonic() >= deadline:
        sys.exit("review handoff timed out")
    time.sleep(.05)
suite = unittest.defaultTestLoader.discover(".")
ok = unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful()
print("CSV regression checks " + ("passed" if ok else "failed"))
sys.exit(0 if ok else 1)
'''


def provider_boundary(attempts):
    """Apply hard spending limits before each upstream request, including transport retries."""
    lock = threading.Lock()

    class Forward(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            length = int(self.headers.get("Content-Length", 0))
            if self.path != "/chat/completions" or not 0 < length <= MAX_BYTES:
                return self.send_error(400, "outside acceptance request limit")
            body = self.rfile.read(length)
            request = json.loads(body)
            output = request.get("max_tokens", request.get("max_completion_tokens"))
            if request.get("model") != MODEL or type(output) is not int or not 0 < output <= MAX_OUTPUT:
                return self.send_error(400, "outside acceptance model or output limit")
            with lock:
                if len(attempts) >= MAX_REQUESTS:
                    return self.send_error(429, "acceptance request limit reached")
                entry = {"request_bytes": length, "max_tokens": output}
                attempts.append(entry)
            upstream = http.client.HTTPSConnection(PROVIDER_HOST, timeout=80)
            try:
                upstream.request("POST", self.path, body, headers={
                    "Authorization": self.headers["Authorization"],
                    "Content-Type": "application/json",
                    "Accept": "text/event-stream",
                })
                response = upstream.getresponse()
                entry["status"] = response.status
                self.send_response(response.status)
                self.send_header("Content-Type", response.getheader("Content-Type"))
                self.end_headers()
                while line := response.readline():
                    if line.startswith(b"data:") and b'"usage"' in line:
                        try:
                            usage = json.loads(line[5:]).get("usage") or {}
                            if usage:
                                entry["usage"] = {key: value for key, value in usage.items()
                                                  if key.endswith("_tokens") and type(value) is int}
                        except ValueError:
                            pass
                    self.wfile.write(line)
                    self.wfile.flush()
            finally:
                upstream.close()

    return Forward


def setup_workspace(workspace):
    for name, content in (("report.py", IMPLEMENTATION), ("test_report.py", TESTS), ("check.py", CHECK)):
        (workspace / name).write_text(content, encoding="utf-8")
    subprocess.run(["git", "init", "-q"], cwd=workspace, check=True)
    subprocess.run(["git", "add", "report.py", "test_report.py", "check.py"], cwd=workspace, check=True)
    subprocess.run(["git", "-c", "user.name=Acceptance", "-c", "user.email=check@localhost",
                    "commit", "-qm", "Initial CSV totals"], cwd=workspace, check=True)
    result = subprocess.run([sys.executable, "-m", "unittest", "-v"], cwd=workspace, capture_output=True)
    assert result.returncode != 0 and b"test_quoted_groups" in result.stderr


def config(endpoint):
    tiers = "".join(f'''
[model.{tier}]
provider = "deepseek-chat"
model = "{MODEL}"
think = "low"
reasoning_effort = "low"
context_window = 32768
max_output_tokens = {MAX_OUTPUT}
''' for tier in ("balanced", "strong", "light"))
    return f'''default_model = "balanced"
{tiers}
[provider.deepseek-chat]
format = "openai-chat"
profile = "deepseek"
base_url = "{endpoint}"
api_key = "$KUKU_TEST_DEEPSEEK_API_KEY"
'''


def handoff(client, workspace, browser, args, result):
    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        tiers = client.request("GET", "/models").as_json()["tiers"]
        assert all("error" not in tier for tier in tiers), "Server cannot resolve its model configuration"
        Budget(args.ledger).reserve(
            args.run_id, RESERVATION_USD,
            "One installed-client development task. At most 12 upstream requests, 48000 serialized bytes "
            "and 2048 output tokens per request, including retries. DeepSeek Flash peak pricing.",
        )
        terminal.press("\x1b[200~" + PROMPT + "\x1b[201~")
        terminal.expect("git diff")
        terminal.press("\r")
        eventually("model executing the project check", lambda: (workspace / ".check-started").exists(), 200)
        session = client.session(workspace)
        original = client.connection()
        result["session_id"] = session
        result["agent"] = "main"
        terminal.exit_cleanly()

    with open_session(browser, client, workspace, session) as page:
        page.get_by_text(PROMPT, exact=False).first.wait_for(state="visible")
        assert client.snapshot(workspace, session)["status"] == "running"
        result["browser"] = {"same_session": session, "running_when_opened": True}
        page.screenshot(path=str(args.evidence / "browser-running.png"))
        page.get_by_role("button", name="New agent", exact=True).click()
        page.get_by_role("textbox", name="Agent address").fill("reviewer")
        page.get_by_role("button", name="Create agent", exact=True).click()
        picker = page.get_by_role("combobox", name="Agent", exact=True)
        picker.select_option("reviewer")
        page.wait_for_function("new URLSearchParams(location.search).get('agent') === 'reviewer'")
        picker.select_option("main")
        page.get_by_text(PROMPT, exact=False).first.wait_for(state="visible")
        result["browser"]["previewed_agents"] = ["reviewer", "main"]
        install(args.archive, client.home, client.env)
        stdout, _ = client.cli(workspace, "server", "restart", success=False)
        assert json.loads(stdout)["state"] == "deferred"
        client.assert_current_service(original)
        (workspace / ".review-ready").touch()
        final = eventually("model final output", lambda: final_output(client, workspace, session), 180)
        page.get_by_text("CSV totals verified", exact=False).last.wait_for(state="visible")
        result["browser"]["observed_completion"] = True

    with client.terminal(workspace) as terminal:
        terminal.resume("Fix quoted CSV totals")
        screen = terminal.expect("CSV totals verified")["screen"]
        (args.evidence / "terminal-final.txt").write_text(screen, encoding="utf-8")
        names, saved_commands = verify_history(client, workspace, session)
        result["tools"] = names
        review_saved_commands(terminal, saved_commands, args.evidence)
        terminal.exit_cleanly()
    client.assert_current_service(original)
    stdout, _ = client.cli(workspace, "show", session, "--json")
    snapshot = json.loads(stdout)
    assert snapshot["last_output"] == final
    files = subprocess.check_output(["git", "diff", "--name-only"], cwd=workspace, text=True).splitlines()
    assert files == ["report.py"], f"Unexpected changed project files: {files}"
    (args.evidence / "project.diff").write_bytes(subprocess.check_output(["git", "diff"], cwd=workspace))
    checks = subprocess.run([sys.executable, "-m", "unittest", "-v"], cwd=workspace, capture_output=True)
    (args.evidence / "project-checks.txt").write_bytes(checks.stdout + checks.stderr)
    assert checks.returncode == 0
    result.update(passed=True, changed_files=files, check_exit=0, upgrade_during_tool=True)


def saved_output_rows(screen):
    rows = screen.splitlines()
    for index, line in enumerate(rows):
        footer = re.search(r"\d+–\d+ / \d+ bytes · PgUp/PgDn", line)
        if footer:
            column = footer.start()
            title = next(i for i, row in enumerate(rows[:index]) if row.startswith("run_command", column))
            return [row[column:] for row in rows[title + 1:index]]
    raise AssertionError("Missing saved command output footer")


def review_saved_commands(terminal, count, evidence):
    wanted = {"diff --git a/report.py": "terminal-diff.txt",
              "CSV regression checks passed": "terminal-check.txt"}
    terminal.request("resize", rows=35, cols=90)
    for index in range(count):
        screen = terminal.expect("run_command") ["screen"]
        terminal.click_text(screen, "Messages" if "Messages" in screen else "Msg")
        terminal.press("\r")
        terminal.press("\x1b[B" * index + "\r")
        terminal.press("\t\t\r")
        screen = terminal.expect("bytes · PgUp/PgDn")["screen"]
        for page in range(4):
            output = saved_output_rows(screen)
            for marker in list(wanted):
                if any(line.startswith(marker) for line in output):
                    (evidence / wanted.pop(marker)).write_text(screen, encoding="utf-8")
            if not wanted:
                terminal.press("\x1b")
                return
            if page < 3:
                terminal.press("\x1b[6~")
                def next_page():
                    current = terminal.request("screen")["screen"]
                    return current if current != screen else None

                try:
                    screen = eventually("a different saved output page", next_page, timeout=1)
                except AssertionError:
                    break
        terminal.press("\x1b")
        terminal.expect("CSV totals verified")
    raise AssertionError(f"Saved tool output in the resumed terminal lacked {list(wanted)}")


def final_output(client, workspace, session):
    agent = client.snapshot(workspace, session)
    if agent["status"] in {"failed", "interrupted", "waiting"}:
        raise AssertionError(f"Model stopped with {agent['status']}")
    return agent["last_output"] if agent["status"] in {"idle", "done"} else None


def verify_history(client, workspace, session):
    query = urlencode({"workspace": workspace, "agent": "main", "limit": 64})
    entries = client.request("GET", f"/sessions/{session}/history?{query}").as_json()["entries"]
    calls = {entry["tool_call_id"]: entry["tool"] for entry in entries if entry["type"] == "tool_call"}
    tools = [entry for entry in entries if entry["type"] == "tool_result"]
    names = [calls[entry["tool_call_id"]] for entry in tools]
    saved = []
    saved_commands = 0
    for entry, name in zip(tools, names):
        if name != "run_command" or not entry.get("output_id"):
            continue
        saved_commands += 1
        query = urlencode({"workspace": workspace, "offset": 0, "max_bytes": 16384})
        output = client.request("GET", f"/sessions/{session}/outputs/{quote(entry['output_id'], safe='')}?{query}").as_json()["output"]
        assert output["complete"] and not output["capture_truncated"] and not output["has_more"]
        saved.extend(segment["text"] for segment in output["segments"])
    output = "".join(saved)
    assert "CSV regression checks passed" in output and "diff --git" in output
    assert "read_file" in names and ("edit_file" in names or "write_file" in names) and "run_command" in names
    return names, saved_commands


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("archive", "bridge", "directory", "evidence", "ledger", "secrets"):
        parser.add_argument("--" + arg, type=Path, required=True)
    parser.add_argument("--run-id", required=True)
    args = parser.parse_args()
    directory = args.directory.resolve()
    if directory.is_relative_to(ROOT) or directory.exists() and any(directory.iterdir()):
        raise ValueError("Live installation requires a fresh directory outside the source tree")
    args.evidence.mkdir(parents=True, exist_ok=True)
    binary = directory / "data" / "bin" / ("kuku.exe" if os.name == "nt" else "kuku")
    client = InstalledClient(binary, args.bridge, directory)
    key = load_secrets(args.secrets).get("KUKU_TEST_DEEPSEEK_API_KEY")
    if not key:
        raise ValueError("DeepSeek acceptance credential is missing")
    client.env.update(KUKU_TEST_DEEPSEEK_API_KEY=key,
                      PATH=str(binary.parent) + os.pathsep + client.env["PATH"])
    install(args.archive, client.home, client.env)
    workspace = client.workspace("csv-project")
    setup_workspace(workspace)
    attempts = []
    result = {"passed": False, "provider": PROVIDER_HOST, "model": MODEL,
              "reasoning_effort": "low", "reservation_usd": RESERVATION_USD, "requests": attempts}
    try:
        with sync_playwright() as playwright, serve(provider_boundary(attempts)) as endpoint:
            browser = playwright.chromium.launch()
            try:
                (client.home / "config.toml").write_text(config(endpoint), encoding="utf-8")
                handoff(client, workspace, browser, args, result)
            finally:
                (workspace / ".review-ready").touch()
                client.stop()
                browser.close()
    finally:
        write_json(args.evidence / "live-result.json", result)
    print("Installed TUI, browser and CLI completed the budgeted real-provider task")


if __name__ == "__main__":
    main()
