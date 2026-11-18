#!/usr/bin/env python3
"""Exercise installed command, terminal, and browser clients against a real service."""

import argparse
import contextlib
import json
import os
from pathlib import Path
import re
import sys
from urllib.parse import urlencode, urlsplit

from interactive_fixture import (
    DenseTranscript, InstalledClient, TOOL_BRIEF, TOOL_OUTPUT, eventually, http_fixture, install,
    delegate_reviewer, json_response, local_request,
)


def installed_upgrade(client, previous, current):
    from run import config_record

    prefix = client.binary.parent.parent
    workspace = client.workspace("work-upgrade")
    prompt = "finish the tool across a package upgrade"
    output = "upgrade tool finished once"
    target_hash = config_record(client.binary)["sha256"]
    install(previous, prefix, client.env)
    assert config_record(client.binary)["sha256"] != target_hash, "Upgrade needs two distinct executables"
    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.submit(prompt)
        client.gated_tool(workspace, client.provider.next(prompt), "upgrade-gated-tool")
        session = client.session(workspace)
        original = client.connection()
        original_version = client.status()["version"]
        terminal.exit_cleanly()

    install(current, prefix, client.env)
    assert config_record(client.binary)["sha256"] == target_hash
    version, _ = client.cli(workspace, "--version")
    stdout, _ = client.cli(workspace, "server", "restart", success=False)
    deferred = json.loads(stdout)
    assert deferred["state"] == "deferred" and deferred["service"]["busy"]
    assert deferred["service"]["version"] == original_version
    client.assert_current_service(original)
    client.release_tool(workspace)
    client.provider.next("upgrade-gated-tool").text(output)
    eventually("old service to complete the tool", lambda: client.snapshot(workspace, session)["last_output"] == output)
    eventually("old service to become idle", lambda: not client.status()["busy"])
    client.verify_tool(workspace)
    client.cli(workspace, "server", "restart")
    assert client.connection()["instance_id"] != original["instance_id"]
    assert version.strip() == "kuku " + client.status()["version"]
    with client.terminal(workspace) as terminal:
        terminal.resume(prompt)
        terminal.expect(output)
        terminal.exit_cleanly()
    client.verify_tool(workspace)
    client.provider.assert_idle()
    print(f"Upgraded {original_version} to {version.strip()} across a busy tool and restored Session {session}.")


@contextlib.contextmanager
def substitute_address(client, target):
    original = client.discovery.read_bytes()
    discovery = json.loads(original)
    discovery["address"] = urlsplit(target).netloc
    client.discovery.write_text(json.dumps(discovery), encoding="utf-8")
    try:
        yield
    finally:
        client.discovery.write_bytes(original)


def busy_protocol_upgrade(client, workspace, connection):
    protocol = client.status(connection)["protocol_version"]
    mismatch = protocol + 100
    restart_codes = []

    def route(method, path, headers, body):
        authorized = {name: headers[name] for name in ("Authorization", "Content-Type") if name in headers}
        response = local_request(connection["address"], method, path,
                                 body=body or None, headers=authorized)
        if path in ("/health", "/service/status") and response.status == 200:
            return json_response({**response.as_json(), "protocol_version": mismatch})
        if path == "/service/restart":
            restart_codes.append(response.status)
        return response

    with http_fixture(route) as proxy, substitute_address(client, proxy):
        stdout, _ = client.cli(workspace, "server", "status")
        status = json.loads(stdout)
        assert status["instance_id"] == connection["instance_id"] and status["protocol_version"] == mismatch
        stdout, stderr = client.cli(workspace, "run", "--json", "protocol mismatch probe", success=False)
        error = stdout + stderr
        assert "protocol" in error.lower(), "Client must explain the incompatible Session protocol"
        for version in (protocol, mismatch):
            assert re.search(rf"\b{version}\b", error), "Compatibility error must show both versions"
        with client.terminal(workspace) as incompatible:
            incompatible.expect(f"protocol {mismatch}")
            incompatible.exit_cleanly()
        stdout, _ = client.cli(workspace, "server", "restart", success=False)
        result = json.loads(stdout)
        assert result["state"] == "deferred" and result["service"]["busy"], (
            "Deferred restart must identify the busy work"
        )
        assert restart_codes == [409], "Busy service must reject the version-upgrade restart"
    client.assert_current_service(connection)
    client.provider.assert_idle()


def two_workspaces_and_disconnect(client):
    workspace = client.workspace("work-left")
    other = client.workspace("work-right")
    prompt = "left terminal gated work"
    final = "left tool completed once"
    tool_id = "native-gated-command"
    with client.terminal(workspace) as left:
        left.expect("New session", state="Ready")
        with client.full_footer(), client.terminal(other) as right:
            right.expect("New session", state="Ready")
            original = client.connection()
            status = client.status(original)
            assert status["instance_id"] == original["instance_id"] and status["pid"] == original["pid"]
            assert not client.sessions(workspace) and not client.sessions(other), (
                "Opening a blank terminal must not create a Session"
            )
            client.provider.assert_idle()

            right.request("resize", rows=32, cols=180)
            right.submit("right workspace check")
            client.provider.next("right workspace check").tool(
                "write_file", "write-right-marker", path="right-marker.txt",
                content="right workspace\n", brief="Write right workspace marker",
            )
            reply = "right workspace verified\n\n| 检查项 | 结果 |\n| --- | --- |\n| 基础测试 | 通过 |\n| 引号字段 | 失败 |"
            with client.provider.next("write-right-marker").streaming_text(reply, "\n\nSummary saved.", usage={
                "input_tokens": 44_500, "cache_read_input_tokens": 133_500,
                "cache_creation_input_tokens": 0, "output_tokens": 205,
            }):
                partial = right.expect("基础测试", state="Streaming")["screen"]
                assert "Summary saved." not in partial and "| ---" not in partial, partial
                assert any("基础测试" in row and "通过" in row for row in partial.splitlines()), partial
            right.expect("Summary saved.")
            footer = right.expect("Context 89% used")["screen"]
            assert "178K in" in footer and "210 out" in footer and "Cache hit 75%" in footer, footer
            assert "Agent main" in footer and "work-right" in footer, footer
            assert re.search(r"\b[0-9]+(?:\.[0-9]+)?[KMB]? TPS\b", footer), footer
            status = footer.splitlines()[-1]
            assert "Agent main" in status and "TPS" in status and "work-right" in status, status
            assert any("基础测试" in row and "通过" in row and "引号字段" not in row for row in footer.splitlines())
            right.type("draft stability")
            assert right.expect("draft stability")["screen"].splitlines()[-1] == status
            right.press("\x03")
            right.request("resize", rows=24, cols=80)
            eventually("a single compact footer on resize", lambda: (
                len(screen := right.request("screen")["screen"].splitlines()) == 24
                and "main" in screen[-1] and any("引号字段" in row for row in screen)
            ))
            right.request("resize", rows=42, cols=140)
            right.expect("Context 89% used")
            print(status.strip())
            assert (other / "right-marker.txt").read_text(encoding="utf-8") == "right workspace\n"
            assert not (workspace / "right-marker.txt").exists(), "Work crossed the terminal workspace boundary"
            right.submit("Summarize the result")
            client.provider.next("Summarize the result").text("Summary confirmed.")
            turns = right.expect("Summary confirmed.", state="Turn 2")["screen"]
            assert "Turn 1" in turns and "Turn 2" in turns, turns
            right.exit_cleanly()
        client.assert_current_service(original)
        assert client.session(other)

        left.type(prompt)
        left.expect("Draft", state="Connected")
        left.press("\r")
        call = client.provider.next(prompt)
        left.expect("Waiting for model", state="Connected")
        client.gated_tool(workspace, call, tool_id)
        left.expect("Running tools", state="Connected")
        session = client.session(workspace)
        assert session != client.session(other)
        assert client.status()["busy"], "Running tool is absent from service status"
        busy_protocol_upgrade(client, workspace, original)

    client.assert_current_service(original)
    client.release_tool(workspace)
    client.provider.next(tool_id).text(final)
    eventually("tool and response to commit without a client",
               lambda: client.snapshot(workspace, session)["last_output"] == final)
    client.verify_tool(workspace)
    with client.terminal(workspace) as reopened:
        reopened.resume(prompt)
        screen = reopened.expect(final)["screen"]
        assert screen.count(final) == 1, "Restored terminal must show one durable response"
        assert screen.count("Run command · ok") == 1, "Restored terminal must show one tool result"
        compact = reopened.expect("TPS")["screen"].splitlines()[-1]
        assert "main" in compact and "Context" in compact and "Cache hit" not in compact, compact
        assert not re.search(r"\d(?:[.\d]*[KMB])? (?:in|out)\b", compact), compact
        details = reopened.expect("Saved output")["screen"]
        assert "--gated-tool" in details
        reopened.press("\x0f")
        collapsed = reopened.expect("Ctrl+O show")["screen"]
        assert "Saved output" not in collapsed and "--gated-tool" not in collapsed
        reopened.press("\x0f")
        reopened.expect("Saved output")
        reopened.exit_cleanly()
    client.assert_current_service(original)
    client.provider.assert_idle(duration=0.2)
    print("Two native workspaces shared a service. The terminal distinguished a draft, model wait, and running tool.")
    print("Busy restart was deferred. The resumed terminal showed saved tool details and toggled their preview.")
    return workspace, session, prompt


def fast_markdown_stream(client):
    workspace = client.workspace("work-fast-stream")
    prompt = "Render the streaming report"
    first = "## Progress\n\n" + "".join(f"Row {i:04d}: 一项完成。\n" for i in range(160))
    last = "\n\nFinal marker"
    draft = "draft kept during burst"
    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.submit(prompt)
        with client.provider.next(prompt).streaming_text(first, last, chunk_size=2):
            terminal.expect("Row 0159", state="Streaming")
            terminal.type(draft)
        terminal.expect("Final marker", state="Finished")
        assert draft in terminal.request("screen")["screen"]
        snapshot = client.snapshot(workspace, client.session(workspace))
        assert snapshot["last_output"].strip() == (first + last).strip()
        terminal.press("\x03")
        terminal.exit_cleanly()
    print(f"Fast stream retained {len(first + last)} characters and the editable draft.")


def configure_statusline(client):
    workspace = client.workspace("work-statusline")
    settings = client.home / "tui.toml"
    initial = '# Terminal preferences\n[statusline]\nfields = ["agent", "workspace"]\n'
    settings.write_text(initial, encoding="utf-8")

    def footer(terminal, predicate):
        last = ""

        def read():
            nonlocal last
            rows = terminal.request("screen")["screen"].splitlines()
            last = rows[-1].strip() if rows else ""
            return last if predicate(last) else None

        try:
            return eventually("expected status line preview", read)
        except AssertionError as error:
            raise AssertionError(f"{error}. Last footer {last!r}") from error

    def open_settings(terminal):
        terminal.press("/statusline\r")
        terminal.expect("Status line settings")

    try:
        with client.terminal(workspace) as terminal:
            original = footer(terminal, lambda line: line.startswith("Agent main") and "work-statusline" in line)
            sdk_settings = (client.home / "config.toml").read_bytes()
            open_settings(terminal)
            terminal.press(" ")
            footer(terminal, lambda line: "Agent main" not in line and "work-statusline" in line)
            terminal.press("\x1b")
            footer(terminal, lambda line: line == original)
            assert settings.read_text(encoding="utf-8") == initial
            open_settings(terminal)
            terminal.press("\x1b[C")
            reordered = footer(terminal, lambda line: "work-statusline" in line and line.endswith("Agent main"))
            terminal.press("\r")
            terminal.expect("Status line saved")
            assert settings.read_text(encoding="utf-8") != initial
            terminal.exit_cleanly()

        with client.terminal(workspace) as restored:
            footer(restored, lambda line: line == reordered)
            open_settings(restored)
            restored.press("r")
            footer(restored, lambda line: "Connected" in line)
            restored.press("\r")
            restored.expect("Status line saved")
            assert "fields" not in settings.read_text(encoding="utf-8")
            restored.exit_cleanly()
        assert (client.home / "config.toml").read_bytes() == sdk_settings
        client.provider.assert_idle()
    finally:
        settings.unlink(missing_ok=True)
    print("Status line preview, cancel, reorder, save, reopen and restore preserved SDK settings.")


def preserve_draft_after_interrupt(client):
    workspace = client.workspace("work-interrupt")
    initial = "hold this response before Esc"
    draft = "keep this draft through Esc"
    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.submit(initial)
        cancelled = client.provider.next(initial)
        session = client.session(workspace)
        original = client.snapshot(workspace, session)
        assert original["status"] == "running"

        terminal.type(draft)
        terminal.press("\x1b")
        terminal.expect(draft, state="Stopped")
        stopped = client.snapshot(workspace, session)
        assert stopped["status"] == "stopped"
        assert stopped["pending_inputs"] == 0, "Esc or draft editing must leave the draft unsent"
        assert original["turn"] == stopped["turn"]
        client.provider.assert_idle(duration=0.2)

        cancelled.text("Late cancelled text")
        terminal.press("\r")
        sent = client.provider.next(draft)
        assert json.dumps(sent.request["messages"], ensure_ascii=False).count(draft) == 1
        sent.text("Recovered from Esc")
        screen = terminal.expect("Recovered from Esc")["screen"]
        assert "Late cancelled text" not in screen
        def committed():
            snapshot = client.snapshot(workspace, session)
            if snapshot["last_output"] == "Recovered from Esc" and snapshot["status"] == "idle":
                return snapshot

        eventually("post-interrupt result to commit", committed)
        terminal.exit_cleanly()
    print("Esc interrupted the observed work. Enter sent the preserved draft and exit restored the terminal.")


def editable_multiline_paste(client):
    workspace = client.workspace("work-paste")
    paste = "原生终端草稿 e\u0301 👩‍💻\r\nsecond line\rthird line\nfourth lineX"
    prompt = "原生终端草稿 e\u0301 👩‍💻\nsecond line\nthird line\nfourth line"
    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        discarded = "draft to clear with Ctrl+C"
        terminal.type(discarded)
        terminal.press("\x03\x03")
        view = terminal.expect("Type a prompt")
        assert view["exit_code"] is None and discarded not in view["screen"]

        terminal.press("/qui\t")

        def completed_quit():
            view = terminal.request("screen")
            assert view["exit_code"] is None, "Tab must leave the client open"
            return view["screen"].count("/quit") == 2

        eventually("Tab to complete /quit in the open actions menu", completed_quit)
        terminal.press("\x03")
        terminal.expect("Search…")

        assert terminal.request("screen")["bracketed_paste"], "The client must request bracketed paste"
        framed = f"\x1b[200~{paste}\x1b[201~"
        if os.name == "nt":
            terminal.press("\x1b" + framed)
        else:
            terminal.press("\x1b")
            eventually("the action picker to close", lambda: "Search…" not in terminal.request("screen")["screen"])
            terminal.press(framed)
        draft = terminal.expect("fourth lineX")["screen"]
        assert "Search…" not in draft, "Escape must have closed the action picker"
        assert client.sessions(workspace) == [], "Pasting a line break must leave the draft unsent"
        client.provider.assert_idle(duration=0.25)

        terminal.press("\x7f\r")
        call = client.provider.next("fourth line")
        message = call.request["messages"][-1]
        texts = [block["text"] for block in message["content"] if block["type"] == "text"]
        assert message["role"] == "user" and prompt in texts, f"The edited paste did not match: {texts!r}"
        call.text("multiline paste verified")
        terminal.expect("multiline paste verified")
        assert client.session(workspace)
        client.provider.assert_idle(duration=0.25)
        terminal.exit_cleanly()
    print("Ctrl+C cleared the focused input, Tab completed /quit, and multiline paste stayed editable until Enter.")


def wait_restart_and_recover_crash(client, workspace, session, title):
    question_prompt = "persist a pending question"
    question = "Which durable answer should I use?"
    with client.terminal(workspace) as terminal:
        terminal.resume(title)
        terminal.submit(question_prompt)
        client.provider.next(question_prompt).tool("ask_user", "waiting-fixture", question=question)
        terminal.expect(question)
        terminal.exit_cleanly()
    waiting = client.snapshot(workspace, session)
    assert waiting["status"] == "waiting"
    request_id = waiting["question"]["id"]
    original = client.connection()
    client.cli(workspace, "server", "restart")
    restarted = client.connection()
    assert restarted["instance_id"] != original["instance_id"]
    assert client.snapshot(workspace, session)["question"]["id"] == request_id
    client.provider.assert_idle(duration=0.2)

    with client.command(workspace, "reply", session, request_id, "--answer", "answer after restart") as process:
        client.provider.next("answer after restart").text("durable answer restored")
        client.finish(process)

    started = "hold request for service crash"
    with client.terminal(workspace) as terminal:
        terminal.resume(title)
        terminal.submit(started)
        crashed_call = client.provider.next(started)
        terminal.expect("Waiting for model", state="Connected")
        client.kill(restarted)
        terminal.expect("Reconnecting")
    crashed_call.text("Uncommitted response after crash")
    with client.terminal(workspace) as terminal:
        terminal.resume(title)
        terminal.expect("Interrupted")
        assert client.connection()["instance_id"] != restarted["instance_id"]
        snapshot = client.snapshot(workspace, session)
        assert snapshot["status"] == "interrupted"
        assert snapshot["last_output"] != "Uncommitted response after crash"
        client.provider.assert_idle(duration=0.2)
        terminal.exit_cleanly()
    print("Restart preserved a pending request. The open terminal showed Reconnecting after the service stopped.")
    print("After recovery, the terminal showed the interrupted work.")


def remote_authorization_and_script_json(client, workspace):
    stdout, stderr = client.cli(workspace, success=False)
    assert "kuku run" in stdout + stderr, "Non-interactive help must identify the script command"
    connection = client.connection()
    address = f"http://{connection['address']}"
    assert client.request("GET", f"/sessions?{urlencode({'workspace': workspace})}",
                          authenticated=False).status == 401
    args = ("run", "--remote", address, "--workspace", str(workspace), "--json")
    client.cli(workspace, *args, "unauthenticated prompt", success=False)
    client.cli(workspace, *args, "invalid token prompt", success=False, token="invalid-fixture-token")
    client.assert_current_service(connection)
    client.provider.assert_idle()

    prompt, answer = "script json contract", "script final output"
    with client.command(workspace, *args, prompt, token=connection["token"]) as process:
        client.provider.next(prompt).text(answer)
        stdout, _ = client.finish(process)
    result = json.loads(stdout)
    assert len(stdout.splitlines()) == 1 and result["type"] == "session" and result["event"] == "completed"
    assert result["agent"] == "main" and result["response"] == answer
    session = result["session_id"]
    stdout, _ = client.cli(workspace, "show", session, "--json")
    assert json.loads(stdout)["last_output"] == answer

    prompt = "script jsonl contract"
    with client.command(workspace, "run", "--stream-json", prompt) as process:
        client.provider.next(prompt).text("script streamed output")
        stdout, _ = client.finish(process)
    progress = [json.loads(line) for line in stdout.splitlines()]
    assert len(progress) >= 2 and progress[-1]["response"] == "script streamed output"

    client.provider.assert_idle()
    print("Remote requests required authentication and installed scripts produced JSON and JSONL.")


def recover_from_provider_failure(client):
    workspace = client.workspace("work-provider-error")
    prompt = "provider failure in scripted run"
    with client.command(workspace, "run", "--json", prompt) as process:
        client.provider.next(prompt).fail()
        stdout, stderr = client.finish(process, success=False)
    assert "fixture provider failure" in stdout + stderr
    failed = json.loads(stdout)
    assert failed["error"] and failed["status"] == "interrupted"
    session = failed["session_id"]
    recovery = "recover explicitly from provider failure"
    with client.terminal(workspace) as terminal:
        terminal.resume(prompt)
        terminal.expect("/continue", state="Interrupted")
        terminal.submit(recovery)
        eventually("recovery input to queue before explicit continuation",
                   lambda: client.snapshot(workspace, session)["pending_inputs"] == 1)
        client.provider.assert_idle(duration=0.2)
        terminal.press("/continue\r")
        client.provider.next(recovery).text("provider recovered")
        terminal.expect("provider recovered")
        terminal.exit_cleanly()
    client.provider.assert_idle()
    print("The failed script returned an error and the terminal continued only after an explicit recovery action.")


def lost_first_receipt(client):
    workspace = client.workspace("work-receipt")
    connection = client.connection()
    command_id, prompt = "first-receipt-id", "send once despite missing first receipt"
    committed = []
    partitioned = True

    def route(method, path, headers, body):
        if partitioned and committed:
            return None
        authorized = {name: headers[name] for name in ("Authorization", "Content-Type") if name in headers}
        response = local_request(connection["address"], method, path,
                                 body=body or None, headers=authorized)
        if method == "POST" and urlsplit(path).path.endswith("/commands") and partitioned:
            committed.append(response.as_json())
            return None
        return response

    with http_fixture(route) as proxy:
        args = ("run", "--remote", proxy, "--workspace", str(workspace), "--command-id", command_id, "--json", prompt)
        with client.command(workspace, *args, token=connection["token"]) as process:
            call = client.provider.next(prompt)
            eventually("durable receipt that is lost to the client", lambda: committed)
            client.gated_tool(workspace, call, "lost-ack-fixture-tool")
            client.release_tool(workspace)
            client.provider.next("lost-ack-fixture-tool").text("first receipt executed once")
            client.finish(process, success=False)
        session = client.session(workspace)
        eventually("committed result before client relaunch",
                   lambda: client.snapshot(workspace, session)["last_output"] == "first receipt executed once")
        partitioned = False
        stdout, _ = client.cli(workspace, *args, token=connection["token"])
        replayed = json.loads(stdout)
        assert replayed["event"] == "acknowledged" and replayed["replayed"] is True
        assert replayed["receipt"] == committed[0]["receipt"]
        assert client.session(workspace) == session
        assert replayed["state"]["usage"]["model_requests"] == 2, (
            "Replayed receipt must keep the original request and its tool continuation"
        )
        client.verify_tool(workspace)
        client.provider.assert_idle(duration=0.2)
    print("Relaunched client recovered its lost first receipt and ran the tool just once")


def open_reviewer(terminal):
    terminal.press("\x1b[1;3B")
    terminal.expect("Switch Agent")
    terminal.press("reviewer")
    menu = terminal.expect("› reviewer", state="Switch Agent")["screen"]
    terminal.press("\r")
    terminal.expect("To reviewer")
    return menu


def check_agent_reading_position(client, terminal):
    prompt = "Print numbered checkpoints for review"
    terminal.submit(prompt)
    markers = [f"Checkpoint {number:03}" for number in range(70)]
    client.provider.next(prompt).text("\n\n".join(markers))
    terminal.expect(markers[-1], state="Finished")
    terminal.press("\x1b[5~")
    screen = terminal.expect("Reading ·")["screen"]
    visible = [line for line in markers if line in screen]
    assert visible and markers[-1] not in visible, screen
    terminal.type("draft at checkpoint")
    open_reviewer(terminal)
    terminal.press("\x1b[1;3A")
    restored = terminal.expect("draft at checkpoint", state="Reading ·")["screen"]
    assert [line for line in markers if line in restored] == visible, restored
    terminal.press("\x03")
    client.provider.assert_idle()
    print(f"Returning to the Agent restored {visible[0]} through {visible[-1]} and its draft.")


def delegated_agent_cards(client):
    workspace = client.workspace("work-agent-cards")
    prompt = "Ask the reviewer to check the sample totals"
    instructions = "Review the selected material independently."
    task = "Check only the forwarded sample"
    initial = "Reviewer is checking the sample"
    conclusion = "Reviewer confirmed the totals"
    reply = f"{initial}\n\n{conclusion}"
    parent_reply = "Parent finished and left the review running"
    card = "Agent reviewer · Input delivered"
    screens = []

    def click_card(terminal, screen):
        row, column = next((row, line.index(card) + 1)
                           for row, line in enumerate(screen.splitlines(), 1) if card in line)
        terminal.press(f"\x1b[<0;{column};{row}M\x1b[<0;{column};{row}m")

    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.request("resize", rows=32, cols=140)
        terminal.submit(prompt)
        parent, child = delegate_reviewer(client.provider, prompt, instructions, task)
        session = client.session(workspace)

        def reviewer_snapshot():
            return client.snapshot(workspace, session, "reviewer")

        terminal.type("parent draft to keep")
        menu = open_reviewer(terminal)
        terminal.expect("From main", state=task)
        assert task in menu, menu
        screens.append(("Switching to the active reviewer with a parent draft", menu))
        terminal.type("reviewer draft to keep")
        terminal.press("\x1b[1;3A")
        original_draft = terminal.expect("parent draft to keep", state="To main")["screen"]
        assert "reviewer draft to keep" not in original_draft
        terminal.press("\x1b[1;3A")
        terminal.expect("reviewer draft to keep", state="To reviewer")
        terminal.press("\x1b[1;3A")
        terminal.expect("parent draft to keep", state="To main")
        terminal.press("\x03")
        client.provider.assert_idle()

        with child.streaming_text(initial, f"\n\n{conclusion}"):
            parent.text(parent_reply)
            eventually("parent to settle while reviewer is streaming", lambda: (
                (parent_state := client.snapshot(workspace, session))["status"] == "idle"
                and parent_state["last_output"] == parent_reply
                and reviewer_snapshot()["status"] == "running"
            ))
            terminal.expect(f"{card} · Agent now Streaming reply")
            open_reviewer(terminal)
            attached = terminal.expect(initial, state="Streaming reply")["screen"]
            assert attached.count(initial) == 1 and "reviewer draft to keep" in attached, attached
            screens.append(("Attaching to a reviewer mid-reply", attached))
            terminal.press("\x1b[1;3A")
            terminal.expect(f"{card} · Agent now Streaming reply", state="To main")
            terminal.press("\x0f")
            expanded = terminal.expect("Live Agent reply")["screen"]
            assert "Requested instructions" in expanded and "Requested task" in expanded and task in expanded, expanded
            assert initial in expanded and conclusion not in expanded, expanded
            screens.append(("Expanded streaming Agent card", expanded))

            def collapsed_card():
                screen = terminal.request("screen")["screen"]
                return screen if card in screen and initial in screen and "Requested task" not in screen else None

            click_card(terminal, expanded)
            collapsed = eventually("mouse to collapse the Agent reply", collapsed_card)
            assert "Requested instructions" in collapsed and "Live Agent reply" not in collapsed, collapsed
            screens.append(("Collapsed streaming Agent card", collapsed))
            click_card(terminal, collapsed)
            terminal.expect("Live Agent reply")
            terminal.press("\x0f")
            globally_collapsed = eventually("Ctrl+O to collapse Agent details", collapsed_card)
            assert "Requested instructions" not in globally_collapsed, globally_collapsed

        eventually("reviewer to settle independently", lambda: (
            (reviewer := reviewer_snapshot())["status"] == "idle" and reviewer["last_output"] == reply
        ))
        terminal.expect(f"{card} · Agent now Finished")
        terminal.exit_cleanly()

    with client.terminal(workspace) as reopened:
        reopened.request("resize", rows=32, cols=140)
        reopened.resume(prompt)
        reopened.expect(f"{card} · Agent now Finished")
        reopened.press("\x10")
        reopened.expect("Actions and Skills")
        reopened.press("\x1b")
        reopened.press("/details\r")
        reopened.expect("Transcript details")
        reopened.press("reviewer")

        def details_menu():
            screen = reopened.request("screen")["screen"]
            rows = screen.splitlines()
            selected = next((i for i, row in enumerate(rows) if "› Agent reviewer" in row), None)
            if selected is None:
                return None
            menu = "\n".join(rows[selected:selected + 4])
            if menu.count("reviewer") >= 2 and task in menu and instructions in menu:
                return screen, menu
            return None

        choices, menu = eventually("search to show both reviewer cards", details_menu)
        assert menu.index(task) < menu.index(instructions), menu
        screens.append(("Searching the two reviewer cards", choices))
        reopened.press("\x1b[B\r")
        focused = reopened.expect("Requested instructions")["screen"]
        assert "Agent reviewer · Created" in focused and "Requested task" not in focused, focused
        screens.append(("Earlier Agent card opened from the details menu", focused))
        click_card(reopened, focused)
        restored = reopened.expect("Latest Agent reply")["screen"]
        assert conclusion in restored and "Live Agent reply" not in restored, restored
        screens.append(("Restored completed Agent card", restored))
        check_agent_reading_position(client, reopened)
        reopened.exit_cleanly()
    client.provider.assert_idle()
    for caption, screen in screens:
        print(f"{caption}\n{screen.rstrip()}")


def dense_transcript(client, reference=False):
    workspace = client.workspace("dense-transcript")
    scenario = DenseTranscript(client, workspace)
    screens = []
    tool_heading = re.compile(
        r"(?:Read (?:file|12 files)|Find files|Search text|Write file|Edit file|Run command|Tool [a-z_]+)"
        r"(?: ×\d+)? · (?:ok|error|running)\b"
    )

    def row_of(screen, text):
        return next(row for row, line in enumerate(screen.splitlines(), 1) if text in line)

    def click(terminal, screen, text):
        row = row_of(screen, text)
        column = screen.splitlines()[row - 1].index(text) + 1
        terminal.press(f"\x1b[<0;{column};{row}M\x1b[<0;{column};{row}m")

    def read_while(terminal, predicate, description):
        screen = ""

        def read():
            nonlocal screen
            screen = terminal.request("screen")["screen"]
            return screen if predicate(screen) else None

        try:
            return eventually(description, read)
        except AssertionError as error:
            raise AssertionError(f"{error}\nLast terminal screen:\n{screen}") from error

    def measure_tools(terminal):
        height = 144
        terminal.request("resize", rows=height, cols=140)
        screen = read_while(terminal, lambda value: len(value.splitlines()) == height
                            and "Running tools" in value and scenario.draft in value,
                            "the full tool transcript in a tall terminal")
        lines = screen.splitlines()
        headings = [index for index, line in enumerate(lines) if tool_heading.search(line)]
        assert len(headings) == (21 if reference else 7), screen
        last = next(index for index, line in enumerate(lines) if TOOL_BRIEF in line)
        assert last >= headings[-1], screen
        return {"summary_rows": len(headings), "tool_rows": last - headings[0] + 1}

    def inspect_tools(terminal):
        terminal.press("\x1b[5~")
        collapsed = read_while(terminal, lambda value: len(value.splitlines()) == 32
                               and scenario.read_group in value and "Reading · Ctrl+End" in value,
                               "the original reading position before opening the group")
        assert scenario.read_target not in collapsed and scenario.draft in collapsed, collapsed
        anchor = row_of(collapsed, scenario.read_group)
        click(terminal, collapsed, scenario.read_group)
        opened = terminal.expect(scenario.read_target)["screen"]
        assert row_of(opened, scenario.read_group) == anchor and "sample-marker 12" not in opened, opened
        assert all(f"samples/read-{index:02d}.txt" in opened for index in range(1, 13)), opened
        screens.append(("Expanded tool group with twelve distinct targets", opened))

        terminal.press("\x10")
        terminal.expect("Actions and Skills")
        terminal.press("details\r")
        terminal.expect("Transcript details")
        terminal.press(scenario.read_target + "\r")
        details = terminal.expect("sample-marker 12")["screen"]
        assert "Requested arguments" in details and scenario.read_target in details and scenario.draft in details, details
        screens.append(("One file's requested arguments and result from the keyboard menu", details))
        click(terminal, details, "sample-marker 12")
        closed = read_while(terminal, lambda value: scenario.read_target in value and "sample-marker 12" not in value,
                            "the single file details to close")
        click(terminal, closed, scenario.read_group)
        restored = read_while(terminal, lambda value: scenario.read_group in value and scenario.read_target not in value,
                              "the group to return to the same reading position")
        assert row_of(restored, scenario.read_group) == anchor and scenario.draft in restored, restored

        click(terminal, restored, "Read file · error")
        error = terminal.expect("Requested arguments")["screen"]
        assert f"path does not exist: {scenario.missing}" in error and "Running tools" in error and scenario.draft in error, error
        screens.append(("Independent failed call with its exact error", error))
        click(terminal, error, "Read file · error")
        terminal.press("\x1b[1;5F")

    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.request("resize", rows=32, cols=140)
        terminal.submit(scenario.prompt)
        with scenario.running_tool():
            terminal.expect("Running tools")
            terminal.type(scenario.draft)
            measurements = measure_tools(terminal)
            terminal.request("resize", rows=32, cols=140)
            wide = read_while(terminal, lambda value: len(value.splitlines()) == 32
                              and scenario.missing in value and scenario.draft in value,
                              "the wide tool turn")
            screens.append(("Dense tool turn with a running command and local draft", wide))
            if not reference:
                assert scenario.read_group in wide, wide
            terminal.request("resize", rows=26, cols=74)
            narrow = terminal.expect(scenario.missing, state="Running tools")["screen"]
            assert scenario.draft in narrow and re.search(r"(?:Run command|Tool run_command) · running", narrow), narrow
            screens.append(("Narrow tool turn with the same draft", narrow))
            terminal.request("resize", rows=32, cols=140)
            if not reference:
                inspect_tools(terminal)
        with scenario.streaming_reply():
            wide = terminal.expect(scenario.source, state="Streaming")["screen"]
            assert scenario.heading in wide and scenario.draft in wide and scenario.end not in wide and "rust" in wide, wide
            screens.append(("Wide Chinese report while the Rust fence is open", wide))
            terminal.request("resize", rows=26, cols=74)
            narrow = terminal.expect(scenario.source, state="Streaming")["screen"]
            assert scenario.draft in narrow and scenario.end not in narrow, narrow
            screens.append(("Narrow Chinese report while the Rust fence is open", narrow))
        complete = terminal.expect(scenario.end, state="Finished")["screen"]
        assert all(text in complete for text in (scenario.draft, "验证项目", "十二项通过", "dense-literal verification saved")), complete
        screens.append(("Complete report and unchanged draft", complete))
        verification = scenario.verify()
        terminal.press("\x03")
        terminal.exit_cleanly()
    for label, screen in screens:
        print(label + "\n" + "\n".join(line.rstrip() for line in screen.splitlines()).rstrip())
    verification.update(measurements)
    print("Dense transcript: " + json.dumps(verification, ensure_ascii=False))
    return verification


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--bridge", required=True, type=Path)
    parser.add_argument("--directory", required=True, type=Path)
    parser.add_argument("--browser", action="store_true")
    parser.add_argument("--upgrade", nargs=2, type=Path, metavar=("OLD_ARCHIVE", "NEW_ARCHIVE"))
    options = parser.parse_args()
    with InstalledClient(options.binary, options.bridge, options.directory) as client:
        if options.upgrade:
            installed_upgrade(client, *options.upgrade)
        workspace, session, title = two_workspaces_and_disconnect(client)
        fast_markdown_stream(client)
        configure_statusline(client)
        preserve_draft_after_interrupt(client)
        editable_multiline_paste(client)
        wait_restart_and_recover_crash(client, workspace, session, title)
        remote_authorization_and_script_json(client, client.workspace("work-script"))
        recover_from_provider_failure(client)
        lost_first_receipt(client)
        delegated_agent_cards(client)
        dense_transcript(client)
        if options.browser:
            from interactive_browser import browser_handoff
            browser_handoff(client)


if __name__ == "__main__":
    main()
