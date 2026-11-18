#!/usr/bin/env python3
"""Exercise installed command, terminal, and browser clients against a real service."""

import argparse
import contextlib
import json
import os
from pathlib import Path
import re
from urllib.parse import urlencode, urlsplit

from interactive_fixture import (
    InstalledClient, TOOL_OUTPUT, eventually, http_fixture, install, json_response, local_request,
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
        with client.terminal(other) as right:
            left.expect("New session", state="Ready")
            right.expect("New session", state="Ready")
            original = client.connection()
            status = client.status(original)
            assert status["instance_id"] == original["instance_id"] and status["pid"] == original["pid"]
            assert not client.sessions(workspace) and not client.sessions(other), (
                "Opening a blank terminal must not create a Session"
            )
            client.provider.assert_idle()

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
            right.request("resize", rows=32, cols=140)
            right.expect("Context 89% used")
            print(status.strip())
            assert (other / "right-marker.txt").read_text(encoding="utf-8") == "right workspace\n"
            assert not (workspace / "right-marker.txt").exists(), "Work crossed the terminal workspace boundary"
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
        assert screen.count("run_command") == 1, "Restored terminal must show one tool result"
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


def main():
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
        preserve_draft_after_interrupt(client)
        editable_multiline_paste(client)
        wait_restart_and_recover_crash(client, workspace, session, title)
        remote_authorization_and_script_json(client, client.workspace("work-script"))
        recover_from_provider_failure(client)
        lost_first_receipt(client)
        if options.browser:
            from interactive_browser import browser_handoff
            browser_handoff(client)


if __name__ == "__main__":
    main()
