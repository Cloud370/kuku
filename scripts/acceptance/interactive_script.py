"""Exercise the installed no-TTY CLI and managed configuration through a provider fixture."""

import json
from pathlib import Path
from urllib.parse import urlencode

from interactive_fixture import eventually


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
    patch_config_through_agent(client, workspace)


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
