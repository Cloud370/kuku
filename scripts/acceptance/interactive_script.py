"""Exercise the installed no-TTY CLI and managed configuration through a provider fixture."""

import http.client
import json
import os
import signal
import threading
from pathlib import Path
from urllib.parse import urlencode

from interactive_fixture import Response, eventually, http_fixture, local_request


def remote_authorization_and_script_json(client, workspace):
    stdout, stderr = client.cli(workspace, success=False)
    assert "kuku run" in stdout + stderr, "Non-interactive help must identify the script command"
    client.cli(workspace, "server", "restart")
    connection = client.connection()
    address = f"http://{connection['address']}"
    assert client.request("GET", f"/sessions?{urlencode({'workspace': workspace})}",
                          authenticated=False).status == 401
    args = ("run", "--remote", address, "--workspace", str(workspace), "--json")
    client.cli(workspace, *args, "unauthenticated prompt", success=False)
    client.cli(workspace, *args, "invalid token prompt", success=False, token="invalid-fixture-token")
    client.assert_current_service(connection)
    client.provider.assert_idle()

    prompt, answer = "script json contract\nKeep the entire second line.\n第三行", "script final output"
    source_file = workspace / "task input.txt"
    source_file.write_bytes(prompt.encode("utf-8"))

    def provider_input():
        call = client.provider.next()
        last = call.request["messages"][-1]
        assert last["role"] == "user"
        assert prompt in [block["text"] for block in last["content"] if block["type"] == "text"]
        return call

    with client.command(workspace, *args, "--prompt-file", str(source_file), token=connection["token"]) as process:
        provider_input().text(answer)
        stdout, _ = client.finish(process)
    result = json.loads(stdout)
    assert len(stdout.splitlines()) == 1 and result["type"] == "session" and result["event"] == "completed"
    assert result["status"] == "completed" and result["agent"] == "main" and result["response"] == answer
    session = result["session_id"]
    stdout, _ = client.cli(workspace, "show", session, "--json")
    assert json.loads(stdout)["last_output"] == answer

    streamed_answer = "script streamed output"
    with source_file.open("rb") as stdin, client.command(
        workspace, "run", "--stream-json", "--session", session, "--prompt-file", "-", stdin=stdin,
    ) as process:
        provider_input().text(streamed_answer)
        stdout, _ = client.finish(process)
    progress = [json.loads(line) for line in stdout.splitlines()]
    assert len(progress) >= 2 and progress[0]["event"] == "started"
    assert progress[-1]["status"] == "completed" and progress[-1]["response"] == streamed_answer
    assert progress[-1]["session_id"] == session
    assert client.snapshot(workspace, session)["last_output"] == streamed_answer
    query = urlencode({"workspace": workspace, "agent": "main"})
    history = client.request("GET", f"/sessions/{session}/history?{query}").as_json()["entries"]
    assert [entry["text"] for entry in history if entry["type"] == "user"] == [prompt, prompt]

    client.provider.assert_idle()
    print(f"Remote authentication, file and stdin input, and complete JSON and JSONL verified in Session {session}.")
    concurrent_script_receipts(client, workspace)
    if os.name != "nt":
        late_reply_interrupt(client, workspace)
        native_permission_continuations(client)
    patch_config_through_agent(client, workspace)


def concurrent_script_receipts(client, workspace):
    first_prompt = "Hold the first Turn while another client edits queued input"
    first_answer = "Only the first Turn produced this answer."
    with client.command(workspace, "run", "--stream-json", "--command-id", "first", first_prompt) as first:
        started = json.loads(first.stdout.readline())
        session = started["session_id"]
        call = client.provider.next(first_prompt)
        path = f"/sessions/{session}/commands"

        def submit(command_id, action, **fields):
            response = client.request("POST", path, payload={
                "workspace": str(workspace), "agent": "main", "command_id": command_id,
                "type": action, **fields,
            })
            assert response.status == 200
            return response.as_json()

        for action in ("withdraw", "replace_queued"):
            queued_id = f"queued-{action}"
            with client.command(workspace, "run", "--stream-json", "--session", session,
                                "--command-id", queued_id, "This input should never be admitted") as queued:
                receipt = json.loads(queued.stdout.readline())["receipt"]
                assert receipt["command_id"] == queued_id
                if action == "withdraw":
                    assert submit("remove", action, input_command_id=queued_id)["status"] == "withdrawn"
                else:
                    submit("replacement", action, input_command_id=queued_id, text="Replacement draft")
                    assert submit("remove-replacement", "withdraw", input_command_id="replacement")["status"] == "withdrawn"
                stdout, _ = client.finish(queued, success=False)
                result = json.loads(stdout.splitlines()[-1])
                assert queued.returncode == 130 and result["status"] == "cancelled" and result["receipt"] == receipt
                assert result["input_status"] == "withdrawn" and result.get("response") is None

        if os.name == "nt":
            call.text(first_answer)
        else:
            # Delay just this observer while the same Agent completes two distinct Turns.
            os.kill(first.pid, signal.SIGSTOP)
            try:
                call.text(first_answer)
                eventually("first Turn to settle", lambda: client.snapshot(workspace, session)["last_output"] == first_answer)
                second_prompt, second_answer = "Run a separate later Turn", "Only the later Turn produced this answer."
                with client.command(workspace, "run", "--json", "--session", session,
                                    "--command-id", "later", second_prompt) as later:
                    client.provider.next(second_prompt).tool("read_file", "later-read", path="task input.txt")
                    client.provider.next("later-read").text(second_answer)
                    stdout, _ = client.finish(later)
                assert json.loads(stdout)["response"] == second_answer
            finally:
                os.kill(first.pid, signal.SIGCONT)
        stdout, _ = client.finish(first)
        results = [json.loads(line) for line in stdout.splitlines()]
        assert results[-1]["receipt"] == started["receipt"] and results[-1]["response"] == first_answer
        assert results[-1]["usage"]["model_requests"] == 1
        assert "Only the later Turn" not in stdout and "later-read" not in stdout
    with client.command(workspace, "run", "--json", "Start a Turn with a tool call") as original:
        opening = client.provider.next("Start a Turn with a tool call")
        session = next(item["session_id"] for item in client.sessions(workspace)
                       if item["title"] == "Start a Turn with a tool call")
        prompt = "Admit this steering input at the next request boundary"
        with client.command(workspace, "run", "--stream-json", "--session", session, prompt) as steering:
            assert json.loads(steering.stdout.readline())["event"] == "started"
            opening.tool("read_file", "read-before-admission", path="task input.txt")
            client.provider.next(prompt).text("The steered Turn completed.")
            stdout, _ = client.finish(steering)
        original_stdout, _ = client.finish(original)
        joined = json.loads(stdout.splitlines()[-1])
        assert joined["response"] == "The steered Turn completed." and "read-before-admission" not in stdout
        assert joined["turns"] == json.loads(original_stdout)["turns"]

    client.provider.assert_idle()
    print("Verified queued withdrawal, replacement, steering and Turn-scoped script results.")


def late_reply_interrupt(client, workspace):
    prompt, answer, output = "Ask me to choose a color", "color=green", "Green recorded."
    with client.command(workspace, "run", "--json", prompt) as process:
        client.provider.next(prompt).tool("ask_user", "ask-color", question="Which color?")
        stdout, _ = client.finish(process, success=False)
    waiting = json.loads(stdout)
    assert process.returncode == 3 and waiting["question"]["id"]
    session, request = waiting["session_id"], waiting["question"]["id"]
    service = client.connection()
    snapshot_path = f"/sessions/{session}/snapshot"
    snapshot_query = urlencode({"workspace": workspace, "agent": "main"})
    arrived, release = threading.Event(), threading.Event()

    def gated_reply(method, path, headers, body):
        upstream_headers = {"Authorization": headers["Authorization"], "Content-Type": "application/json"}
        if path.startswith(snapshot_path + "?") and not arrived.is_set():
            arrived.set()
            assert release.wait(10), "The reply's snapshot was never released"
        if "/observe?" in path:
            # Forward the real journal through the later Turn's stable cursor. Live events are unnecessary here.
            cursor = client.request("GET", f"{snapshot_path}?{snapshot_query}").as_json()["cursor"]
            upstream = http.client.HTTPConnection(service["address"], timeout=5)
            try:
                upstream.request(method, path, headers=upstream_headers)
                response = upstream.getresponse()
                assert response.status == 200
                frames = []
                for line in response:
                    frames.append(line)
                    frame = json.loads(line)
                    if frame.get("type") == "committed" and frame["id"] >= cursor:
                        break
                return Response(200, b"".join(frames), "application/x-ndjson")
            finally:
                upstream.close()
        return local_request(service["address"], method, path, body=body, headers=upstream_headers)

    with http_fixture(gated_reply) as remote, client.command(
        workspace, "--remote", remote, "--workspace", str(workspace), "reply", session, request,
        "--answer", answer, "--command-id", "answer-green", token=service["token"],
    ) as reply:
        try:
            continuation = client.provider.next(answer)
            assert arrived.wait(5), "The reply did not start observing its continuation"
            continuation.text(output)
            eventually("answer to settle", lambda: client.snapshot(workspace, session)["last_output"] == output)
            original_turn = client.snapshot(workspace, session)["turn"]
            later_prompt = "Run independently after the question has been answered"
            with client.command(workspace, "run", "--json", "--session", session, later_prompt) as later:
                call = client.provider.next(later_prompt)
                os.kill(reply.pid, signal.SIGINT)
                release.set()
                stdout, _ = client.finish(reply)
                result = json.loads(stdout)
                assert result["status"] == "idle" and result["turn"] == original_turn
                assert result["last_output"] == output and result["usage"]["model_requests"] == 1
                assert client.snapshot(workspace, session)["status"] == "running"
                call.text("Independent later reply.")
                stdout, _ = client.finish(later)
                assert json.loads(stdout)["response"] == "Independent later reply."
        finally:
            release.set()
    print("A late interrupt returned the answered Turn and left the next client's Turn running.")


def native_permission_continuations(client):
    workspace = client.workspace("script-native-permissions", permissions="ask")
    prompt = "Write two files with separate approval requests"
    with client.command(workspace, "run", "--json", prompt) as process:
        client.provider.next(prompt).tool("write_file", "native-first", path="first.txt", content="first")
        stdout, _ = client.finish(process, success=False)
    waiting = json.loads(stdout)
    assert process.returncode == 3 and waiting["permission"]["tool"] == "write_file"
    session, first_request = waiting["session_id"], waiting["permission"]["id"]
    with client.command(workspace, "reply", session, first_request, "--allow") as first:
        continuation = client.provider.next("native-first")
        os.kill(first.pid, signal.SIGSTOP)
        try:
            continuation.tool("write_file", "native-second", path="second.txt", content="second")

            def second_permission():
                request = client.snapshot(workspace, session).get("permission")
                return request if request and request["id"] != first_request else None

            request = eventually("the second native permission", second_permission)
            with client.command(workspace, "reply", session, request["id"], "--allow") as second:
                final = client.provider.next("native-second")
                os.kill(first.pid, signal.SIGINT)
                os.kill(first.pid, signal.SIGCONT)
                stdout, _ = client.finish(first)
                result = json.loads(stdout.splitlines()[-1])
                assert result["status"] == "waiting" and result["permission"]["id"] == request["id"]
                assert result["last_output"] is None and result["usage"] is None
                assert client.snapshot(workspace, session)["status"] == "running"
                final.text("Both files were approved.")
                stdout, _ = client.finish(second)
                assert json.loads(stdout.splitlines()[-1])["last_output"] == "Both files were approved."
        finally:
            if first.poll() is None:
                os.kill(first.pid, signal.SIGCONT)
    assert (workspace / "first.txt").read_text() == "first" and (workspace / "second.txt").read_text() == "second"
    print("A delayed native permission reply observed its own wait and left the next approval's execution running.")


def patch_config_through_agent(client, workspace):
    default_config = client.home / "config.toml"
    active_config = (workspace / "service config.toml").resolve()
    original = default_config.read_bytes()
    active_original = b"# Custom managed fixture configuration\n" + original
    active_config.write_bytes(active_original)
    stdout, _ = client.cli(workspace, "--config", str(active_config), "server", "restart")
    assert Path(json.loads(stdout)["service"]["config_path"]).resolve() == active_config
    stdout, _ = client.cli(workspace, "config", "show", "--json")
    assert Path(json.loads(stdout)["path"]).resolve() == active_config
    stdout, _ = client.cli(workspace, "--config", str(default_config), "config", "show", "--json")
    explicit = json.loads(stdout)
    assert Path(explicit["path"]).resolve() == default_config.resolve()
    assert explicit["service"]["state"] == "different_path"
    prompt = "Change the managed service default through the installed CLI"
    model = "fixture-patched-model"
    patch = f'''default_model = {{ connection = "test-anthropic", model = "{model}" }}

[model.test-anthropic."{model}"]
think = "off"
max_output_tokens = 2048
'''
    with client.command(workspace, "run", "--json", prompt) as process:
        call = client.provider.next(prompt)
        assert call.request["model"] == "claude-sonnet-4-6"
        call.tool("run_command", "patch-active-config", program=str(client.binary),
                  args=["config", "set", "--patch", patch])
        patched = client.provider.next("patch-active-config")
        tool_result = patched.tool_result("patch-active-config")
        assert active_config.name in str(tool_result["content"])
        patched.text("The active configuration was updated.")
        stdout, _ = client.finish(process)
    assert json.loads(stdout)["status"] == "completed"
    assert default_config.read_bytes() == original
    assert active_config.read_bytes() != active_original

    def active_config_loaded():
        stdout, _ = client.cli(workspace, "config", "validate")
        result = json.loads(stdout)
        assert Path(result["path"]).resolve() == active_config
        return result["service"]["state"] == "loaded"

    eventually("the service to load its patched configuration", active_config_loaded)
    catalog = client.request("GET", "/models").as_json()
    assert catalog["default_model"] == {"connection": "test-anthropic", "model": model}
    with client.command(workspace, "run", "--json", "Use the newly configured default") as process:
        call = client.provider.next("Use the newly configured default")
        assert call.request["model"] == model and call.request["max_tokens"] == 2048
        call.text("Patched model confirmed.")
        stdout, _ = client.finish(process)
    assert json.loads(stdout)["response"] == "Patched model confirmed."
    client.provider.assert_idle()
    print(f"An Agent updated {active_config.name} through config set and a new Session used {model}.")
