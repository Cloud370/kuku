"""Installed-terminal checks for settings, defaults and retained rewind history."""

import difflib
import json
import os
import re
import threading
import time
from urllib.parse import urlencode

from interactive_fixture import is_composer_heading, InstalledClient, Response, TOOL_OUTPUT, TOOL_PROGRAM, Terminal, eventually, json_response, run_cases
from interactive_history import paginated_rewind, rewind_files
from interactive_model_recovery import assert_live_elapsed, assert_testing_navigation, assert_wide_setup, recovery_cases
from interactive_rendering import has_selected_queued_input, queued_input_click_position


def lifecycle_checks(binary, bridge, directory):
    def configured(name, operation):
        with InstalledClient(binary, bridge, directory / name) as client:
            operation(client)

    cases = [("first-run", lambda: first_run_credential_repair(binary, bridge, directory / "settings"))]
    cases.extend((name, lambda name=name, operation=operation: configured(name, operation))
                 for name, operation in (
                     ("background-job", background_job_recovery),
                     ("defaults", persistent_model_default),
                     ("keyboard-queue", keyboard_queue_withdrawal),
                     ("rewind", readonly_rewind),
                     ("rewind-history", paginated_rewind),
                     ("rewind-files", rewind_files),
                 ))
    cases.append(("model-setup", lambda: shared_model_editor(binary, bridge, directory / "model-setup")))
    cases.append(("setup-probe-progress", lambda: setup_probe_progress(binary, bridge, directory / "probe-progress")))
    cases.append(("setup-recovery", lambda: recovery_cases(binary, bridge, directory / "setup-recovery")))
    cases.append(("auth-repair", lambda: runtime_credential_repair(binary, bridge, directory / "auth-repair")))
    return cases


def verify_restored_tool(terminal, job, history):
    def titles(screen):
        return [line for line in screen.splitlines() if line.lstrip(" ╭─│").startswith(TOOL_PROGRAM)]

    if job is not None:
        terminal.press("\x0f")
        terminal.expect("Ctrl+O show")
        terminal.press("\x1b[5~")
        screen = terminal.expect(TOOL_PROGRAM, also=("started in background", "Background job",
                                                 f"{job['id']} · Completed"))["screen"]
        assert len(titles(screen)) == 1 and screen.count("Background job") == 1, screen
        terminal.press("\x1b[6~")
        terminal.press("\x0f")
        terminal.expect(TOOL_OUTPUT)
        return
    screen = terminal.expect(TOOL_PROGRAM)["screen"]
    assert len(titles(screen)) == 1 and "· ok · exit 0" in screen, f"{screen}\nCommitted tool: {history}"
    assert "--gated-tool" in screen and TOOL_OUTPUT in screen
    terminal.press("\x0f")
    collapsed = terminal.expect("Ctrl+O show")["screen"]
    assert "View output" not in collapsed and "--gated-tool" in collapsed, collapsed
    terminal.press("\x0f")
    terminal.expect(TOOL_PROGRAM, also=(TOOL_OUTPUT,))


def background_job_recovery(client, *, explicit=False):
    workspace = client.workspace("work-background")
    prompt, final = "Keep a background command across client exit", "Background command completed once"
    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.submit(prompt)
        client.gated_tool(workspace, client.provider.next(prompt), "background-proof", background=explicit)
        session = client.session(workspace)
        query = urlencode({"workspace": workspace, "agent": "main"})
        eventually("durable background Job registration", lambda: client.request(
            "GET", f"/sessions/{session}/jobs?{query}").as_json()["jobs"])
        terminal.expect("started in background")
        terminal.exit_cleanly()
    client.release_tool(workspace)
    completed = client.completed_tool(workspace, "background-proof")
    assert completed.completed_job is not None
    completed.text(final)
    session = client.session(workspace)
    eventually("background reply to commit", lambda: client.snapshot(workspace, session)["last_output"] == final)
    client.verify_tool(workspace)
    with client.terminal(workspace) as terminal:
        terminal.resume(prompt)
        terminal.expect(final)
        verify_restored_tool(terminal, completed.completed_job, None)
        terminal.exit_cleanly()
    client.provider.assert_idle()


def focus_action(terminal, action):
    def focused():
        return next((line.strip() for line in terminal.request("screen")["screen"].splitlines()
                     if "▶ " in line), None)

    for _ in range(10):
        before = focused()
        if before and f"▶ {action}" in before:
            return
        terminal.press("\x1bOQ")
        try:
            eventually("F2 to focus the next visible action", lambda: (
                after if (after := focused()) is not None and after != before else None
            ))
        except AssertionError as error:
            screen = terminal.request("screen")["screen"]
            raise AssertionError(f"{error}\nLast terminal screen:\n{screen}") from error
    raise AssertionError(f"No visible contextual action {action!r}")


def first_run_credential_repair(binary, bridge, directory):
    instance = InstalledClient(binary, bridge, directory)
    login = directory / "login"
    project = login / "project"
    project.mkdir(parents=True)
    environment = {**instance.env, "HOME": str(login)}
    environment.pop("OPENROUTER_API_KEY", None)
    if os.name == "nt":
        environment["KUKU_HOME"] = str(login / ".kuku")
    else:
        environment.pop("KUKU_HOME")
    for workspace in (login, project):
        with Terminal(bridge, binary, workspace, environment) as terminal:
            terminal.expect("New session")
            terminal.request("resize", rows=20, cols=60)
            terminal.expect("Set up a model", rows=20)
            terminal.open_first_model_settings()
            terminal.press("\r")
            terminal.expect("Choose a provider")
            terminal.press("\x1b[A")
            terminal.expect("› Preset · ChatGPT subscription")
            terminal.press("\x1b[A")
            terminal.expect("› Preset · OpenRouter")
            terminal.press("\r\r\r")
            terminal.expect("Paste your API key")
            terminal.press("\x03")
            terminal.expect("New session", also=("/model", "Set up a model"))
            terminal.exit_cleanly()
    assert not (login / ".kuku" / "config.toml").exists()

    key = "fixture-repaired-credential"
    authenticated = []
    original_dispatch = instance.provider.dispatch

    def dispatch(method, path, headers, body):
        assert headers.get("x-api-key") == key, "Repaired credential did not reach the provider"
        if method == "GET":
            return json_response({}, 404)
        request = json.loads(body)
        if not request.get("stream"):
            assert request.get("thinking") == {"type": "disabled"}
            assert request["max_tokens"] == 4096, request["max_tokens"]
            if any(part["type"] == "image" for part in request["messages"][0]["content"]):
                return json_response({"content": [], "stop_reason": "max_tokens"})
            return json_response({"content": [{"type": "text", "text": "377"}]})
        authenticated.append(True)
        return original_dispatch(method, path, headers, body)

    instance.provider.dispatch = dispatch
    with instance as client:
        config = client.home / "config.toml"
        config.write_text(config.read_text(encoding="utf-8").replace(
            'api_key = "test-key"', 'api_key = "$KUKU_ACCEPTANCE_MISSING_KEY"'
        ), encoding="utf-8")
        workspace = client.directory
        prompt = "Verify the repaired first-run settings"
        with client.terminal(workspace) as terminal:
            terminal.open_first_model_settings()
            terminal.expect("Find and test models")
            client.provider.assert_idle()
            terminal.press("\r")
            terminal.expect("Choose a provider", also=("Existing · test-anthropic",))
            terminal.press("\r\r\r")
            terminal.expect("Paste your API key")
            terminal.press("\x1b")
            missing = terminal.expect("API key source", also=("not detected", "› Paste API key"))["screen"]
            client.provider.assert_idle()
            terminal.press("\r")
            terminal.expect("Paste your API key")
            terminal.press(f"\x1b[200~{key}\x1b[201~")
            masked = terminal.expect(f"{key[:7]}{'•' * (len(key) - 12)}{key[-5:]}")["screen"]
            assert key not in masked
            terminal.press("\r")
            terminal.expect("HTTP 404")
            terminal.press("\x1b[B\r")
            terminal.expect("Model ID")
            terminal.press("\r")
            terminal.expect("› Model tests", also=("2/2 finished", "Ready", "Image unverified", "Save and continue"), absent=("Fix failed models",))
            terminal.press("\r")
            terminal.expect(
                "Not verified · output limit",
                also=("› Image input", "✓ Received", "Cost · Unknown"),
            )
            terminal.press("\r")
            terminal.expect("› Save and continue")
            terminal.press("\r")
            terminal.expect("New session", state="Ready")
            client.provider.assert_idle()
            terminal.submit(prompt)
            call = client.provider.next(prompt)
            assert call.request["model"] == "claude-sonnet-4-6", call.request["model"]
            call.text("First reply used the repaired settings.")
            reply = terminal.expect("First reply used the repaired settings.", state="Finished")["screen"]
            terminal.exit_cleanly()
        assert authenticated == [True]
        assert client.session(workspace)
        with client.terminal(workspace) as terminal:
            terminal.resume(prompt)
            terminal.expect("First reply used the repaired settings.")
            terminal.exit_cleanly()
        with client.terminal(client.workspace("another-workspace")) as terminal:
            terminal.expect("claude-sonnet-4-6", state="Ready")
            terminal.submit("Use the configured model in another workspace")
            shared = client.provider.next("Use the configured model in another workspace")
            assert shared.request["model"] == call.request["model"]
            shared.text("Same model and credential.")
            terminal.expect("Same model and credential.", state="Finished")
            terminal.exit_cleanly()
        assert authenticated == [True, True]
        assert key not in config.read_text(encoding="utf-8")
        credential_files = list((client.home / "credentials").iterdir())
        assert len(credential_files) == 1
        assert credential_files[0].read_text(encoding="utf-8") == key
        (client.directory / "first-run.json").write_text(json.dumps({
            "missing_credential_screen": missing,
            "masked_credential_screen": masked,
            "first_reply_screen": reply,
            "authenticated_requests": len(authenticated),
            "model": call.request["model"],
        }, ensure_ascii=False, indent=2), encoding="utf-8")
        print("First-run credential repair kept the key masked and used it for the first reply.")


def shared_model_editor(binary, bridge, directory):
    key = "fixture-shared-model-key"
    instance = InstalledClient(binary, bridge, directory)
    original_dispatch = instance.provider.dispatch
    addresses = []

    def dispatch(method, path, headers, body):
        assert headers.get("x-api-key") == key
        addresses.append((method, path))
        if method == "GET" and path == "/gateway/models":
            return json_response({"data": [{"id": "beta"}, {"id": "alpha"}, {"id": "beta"}]})
        if not json.loads(body).get("stream"):
            return json_response({"content": [{"type": "text", "text": "377"}]})
        return original_dispatch(method, "/v1/messages" if path == "/gateway/messages" else path, headers, body)

    instance.provider.dispatch = dispatch
    with instance as client:
        config = client.home / "config.toml"
        endpoint = re.search(r'base_url = "(http://127\.0\.0\.1:[0-9]+)"', config.read_text(encoding="utf-8"))[1]
        config.write_text("", encoding="utf-8")
        workspace = client.workspace("work-model-setup")
        with client.terminal(workspace) as terminal:
            terminal.request("resize", rows=32, cols=110)
            terminal.open_first_model_settings()
            terminal.expect("Set up a model", also=("Esc exit",))
            terminal.press("\r")
            providers = terminal.expect("› New custom connection", also=("Esc previous step",))["screen"]
            assert providers.index("New custom connection") < providers.index("Preset")
            terminal.press("\r")
            terminal.expect("API URL", also=("Esc previous step",))
            terminal.press("\x1b")
            terminal.expect("› New custom connection")
            terminal.press("\r")
            terminal.expect("API URL")
            terminal.type(f"{endpoint}/gateway/messages")
            terminal.expect(f"Request · {endpoint}/gateway/messages")
            terminal.press("\r")
            terminal.expect("Use a proxy?")
            terminal.press("\r")
            terminal.expect("Paste your API key")
            terminal.press(f"\x1b[200~{key}\x1b[201~")
            terminal.expect(f"{key[:7]}{'•' * (len(key) - 12)}{key[-5:]}", absent=(key,))
            assert addresses == []
            terminal.press("\r")
            catalog = terminal.expect("Choose models", also=("beta", "alpha"))["screen"]
            assert catalog.index("beta") < catalog.index("alpha") and catalog.count("beta") == 1
            terminal.press("alpha \x01beta \r")
            defaults = terminal.expect("Choose the default model", also=("› beta", "alpha"))["screen"]
            assert defaults.index("beta") < defaults.index("alpha")
            terminal.press("\x1b")
            terminal.expect("Choose models", also=("2 selected", "Esc previous step"))
            terminal.press("\r")
            terminal.expect("Choose the default model")
            terminal.press("\r")
            terminal.request("resize", rows=20, cols=60)
            save_review = terminal.expect(
                "Save setup", also=("Default", "beta", "API URL", "Settings", "› Model tests"),
                absent=("Model capabilities", "Save and exit"),
            )["screen"]
            terminal.press("\x1b[B\r")
            settings_review = terminal.expect(
                "Model capabilities", also=("medium", "Costs", "Save and exit"),
                absent=("API URL",),
            )["screen"]
            terminal.press("\x1b")
            terminal.expect("› Model tests")
            terminal.press("\x1b[A")
            terminal.expect("› Save and continue")
            terminal.request("resize", rows=32, cols=110)
            terminal.expect("› Save and continue", also=("└" + "─" * 108 + "┘",), rows=32)
            terminal.press("\r")
            terminal.expect("New session", state="Ready", absent=("Connected", "F2 Restart"))
            terminal.submit("Use the first saved default")
            first = client.provider.next("Use the first saved default")
            assert first.request["model"] == "beta"
            assert first.request["thinking"] == {"type": "adaptive"}
            assert first.request["output_config"]["effort"] == "medium"
            first.text("Saved default sent a text turn.")
            terminal.expect("Saved default sent a text turn.", state="Finished")

            draft = "keep the unsent draft"
            terminal.type(draft)
            terminal.press("\x0c")
            terminal.expect("Add model or connection")
            terminal.press("/Add model or connection\r")
            terminal.expect("Set up a model")
            terminal.press("\r")
            terminal.expect(f"Existing · {endpoint.removeprefix('http://')}", absent=("Existing · custom",))
            terminal.press("\r")
            terminal.expect("Choose models")
            terminal.press("\x1b")
            terminal.expect("Enter model ID")
            terminal.press("\x1b[B\r")
            terminal.expect("Model ID")
            terminal.type("gamma")
            terminal.press("\r")
            terminal.expect("› Model tests", also=("Save models", "API URL", "gamma"))
            terminal.press("\x1b[A\r")
            terminal.expect("gamma", state="Model route")
            terminal.press("/gamma")
            terminal.expect("/ gamma", also=("›gamma",))
            terminal.press("\x1b")
            terminal.expect("e Edit")
            terminal.press("e")
            terminal.expect("Set up a model")
            terminal.press("\r")
            terminal.expect("API URL")
            terminal.press("\r\r")
            terminal.expect("Leave blank to keep your saved key.")
            terminal.press("\r")
            terminal.expect("Keep current model")
            terminal.press("\x1b[B\r")
            terminal.expect("Model ID", also=("gamma",))
            terminal.press("\x01")
            terminal.type("delta")
            terminal.press("\r")
            terminal.expect("› Model tests", also=("Save models", "API URL", "delta"))
            terminal.press("\x1b[A\r")
            terminal.expect("gamma", state="Model route", also=("delta",))
            terminal.press("\x1b")
            terminal.expect(draft, absent=("Connected", "F2 Restart", "Model route"))
            assert len(list((client.home / "credentials").iterdir())) == 1
            catalog = client.request("GET", "/models").as_json()
            assert catalog["default_tier"] == "default"
            for item in catalog["tiers"]:
                assert item["capabilities"]["reasoning_source"] is None
                assert item["capabilities"]["input_modalities"] is None
                assert item["capabilities"]["output_modalities"] is None
            assert next(item for item in catalog["tiers"] if item["name"] == "default")["model"] == "beta"
            assert next(item for item in catalog["tiers"] if item["name"] == "gamma")["model"] == "delta"

            terminal.press("\x0c")
            terminal.expect("gamma", state="Model route", also=("delta",))
            terminal.press("/gamma")
            match = terminal.expect("/ gamma", also=("›gamma",))["screen"]
            terminal.click_text(match, "›gamma")
            terminal.expect(draft, also=("delta",), absent=("Model route", "Model settings"))
            terminal.press("\r")
            terminal.expect("Model route changed", also=("beta → delta", "Enter Send", draft))
            client.provider.assert_idle()
            terminal.press("\r")
            active = client.provider.next(draft)
            assert active.request["model"] == "delta"
            draft = "draft across busy and allowed restart"
            terminal.type(draft)
            previous = client.connection()
            terminal.press("\x10/restart\r")
            terminal.expect("Restart local service", also=("› Cancel",))
            terminal.press("\x1b[A\r")
            terminal.expect("Restart deferred", also=(draft,))
            client.assert_current_service(previous)
            active.text("Edited route completed.")
            terminal.expect("Edited route completed.", state="Finished")

            terminal.press("\x10/restart\r")
            terminal.expect("Restart local service", also=("› Cancel",))
            terminal.press("\r")
            terminal.expect(draft, absent=("Restart local service",))
            client.assert_current_service(previous)
            terminal.press("\x10/restart\r")
            terminal.expect("Restart local service", also=("› Cancel",))
            terminal.press("\x1b[A\r")
            def replaced_service():
                try:
                    return client.connection()["instance_id"] != previous["instance_id"]
                except FileNotFoundError:
                    return False

            eventually("a replacement kuku service", replaced_service)
            terminal.expect(draft, state="Finished", absent=("Connected", "F2 Restart"))
            terminal.exit_cleanly("\x03\x03")
        assert ("GET", "/gateway/models") in addresses and ("POST", "/gateway/messages") in addresses
        (client.directory / "review-layout.json").write_text(
            json.dumps({"save": save_review, "settings": settings_review}, indent=2, ensure_ascii=False),
            encoding="utf-8",
        )


def setup_probe_progress(binary, bridge, directory):
    key = "fixture-probe-only-key"
    instance = InstalledClient(binary, bridge, directory)
    api_checks = threading.Barrier(3, timeout=5)
    slow_catalog_started = threading.Event()
    held_requests = {pair: threading.Event() for pair in (
        ("beta", None), ("gamma", None), ("beta", "low"),
        ("gamma", "medium"), ("alpha", "high"),
    )}
    release_requests = threading.Event()
    release_beta_text = threading.Event()
    release_beta_effort = threading.Event()
    rate_request_started = threading.Event()
    allow_rate_limit = threading.Event()
    runtime_requests = []
    release = threading.Event()
    trials = []

    def dispatch(method, path, headers, body):
        if method == "POST" and path in ("/v1/responses", "/v1/messages"):
            assert json.loads(body)["model"] == "alpha"
            api_checks.wait()
            return json_response({}, 404)
        if method == "GET" and "x-api-key" in headers:
            return json_response({}, 401)
        assert headers.get("authorization") == f"Bearer {key}"
        if method == "GET" and path == "/models":
            slow_catalog_started.set()
            release.wait(20)
            return json_response({}, 404)
        if method == "GET" and path == "/v1/models":
            assert slow_catalog_started.wait(8)
            return json_response({"data": [
                {"id": "alpha", "input_modalities": ["text"]},
                {"id": "beta", "input_modalities": ["text"], "reasoning_efforts": ["low"]},
                {"id": "gamma", "input_modalities": ["text"], "reasoning_efforts": ["medium"]},
            ]})
        if method == "GET" and path == "/api/v1/models":
            return json_response({}, 404)
        assert (method, path) == ("POST", "/v1/chat/completions")
        body = json.loads(body)
        model, effort = body["model"], body.get("reasoning_effort")
        if body.get("stream"):
            runtime_requests.append((model, effort))
            chunks = [
                {"choices": [{"index": 0, "delta": {"content": "Saved effort sent."}}]},
                {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
            ]
            return Response(200, ("".join(f"data: {json.dumps(chunk)}\n\n" for chunk in chunks) + "data: [DONE]\n\n").encode(), "text/event-stream")
        assert body["max_tokens"] == 4096
        if not trials:
            api_checks.wait()
        trials.append((model, effort, time.monotonic()))
        if (model, effort) in held_requests:
            held_requests[model, effort].set()
            completion_gate = {("beta", None): release_beta_text, ("beta", "low"): release_beta_effort}.get((model, effort), release_requests)
            completion_gate.wait(20)
        if effort == "__kuku_unknown_effort__":
            return json_response({"error": {"param": "reasoning_effort", "allowed_values": ["none", "high", "max"]}}, 400)
        if model == "alpha" and effort == "none" and sum(row[:2] == ("alpha", "none") for row in trials) == 1:
            rate_request_started.set()
            allow_rate_limit.wait(20)
            return json_response({"error": {"type": "rate_limit_error", "message": "fixture rate limit"}}, 429)
        if effort == "high":
            return json_response({"error": {"param": "reasoning_effort", "message": "high is unsupported"}}, 422)
        message = {"content": "377"}
        if effort == "none":
            message["reasoning_content"] = "fixture reasoning"
        return json_response({
            "choices": [{"message": message, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 20, "completion_tokens": 2, "cost": 0.000008},
        })

    instance.provider.dispatch = dispatch

    def overview_layout(screen):
        lines = screen.splitlines()
        heading = next(index for index, line in enumerate(lines)
                       if all(label in line for label in ("Model", "Text", "Thinking", "Image")))
        columns = tuple(lines[heading].index(label) for label in ("Text", "Thinking", "Image"))
        model_rows = {model: next(index for index, line in enumerate(lines)
                                  if line.strip(" │").startswith(model + " "))
                      for model in ("alpha", "beta", "gamma")}
        menu = next(index for index, line in enumerate(lines) if any(
            label in line for label in ("› View requests", "› Save and continue")))
        return heading, columns, model_rows, menu

    def model_cells(screen, model):
        _, columns, model_rows, _ = overview_layout(screen)
        line = screen.splitlines()[model_rows[model]]
        return tuple(line[left:right].strip(" │") for left, right in zip(columns, (*columns[1:], len(line))))

    def overall(screen, percent, filled):
        bar = next(line for line in screen.splitlines() if "Overall" in line)
        assert f"] {percent}%" in bar and bar.count("━") == filled, screen

    try:
        with instance as client:
            config = client.home / "config.toml"
            endpoint = re.search(r'base_url = "(http://127\.0\.0\.1:[0-9]+)"', config.read_text(encoding="utf-8"))[1]
            config.write_text("", encoding="utf-8")
            with client.terminal(client.workspace("work-probe-progress")) as terminal:
                terminal.open_first_model_settings()
                terminal.expect("Find and test models", also=("Edit config file",))
                terminal.press("\x1b[B\r")
                terminal.expect("Manual configuration", also=("config.toml",))
                assert not slow_catalog_started.is_set() and not trials
                terminal.press("\x1b")
                terminal.expect("Set up a model")
                terminal.press("\r")
                terminal.expect("› New custom connection")
                terminal.press("\r")
                terminal.expect("API URL", also=("API type · Auto",))
                terminal.type(endpoint)
                terminal.press("\r\x1b[B\r")
                terminal.expect("Paste your API key")
                terminal.press(f"\x1b[200~{key}\x1b[201~\r")
                discovery = terminal.expect("Choose models", also=("alpha", "beta", "gamma"))["screen"]
                assert slow_catalog_started.is_set()
                assert not trials
                terminal.press("alpha \x01beta \x01gamma \r")
                terminal.request("resize", rows=20, cols=60)
                terminal.expect("Choose the default model", rows=20)
                terminal.press("\r")
                save_activity = terminal.expect(
                    "Testing models", rows=20,
                    also=("Overall", "25%", "3 selected", "6 running", "1 queued", "┼",
                          "› View requests", "Settings", "Save early", "Esc previous step"),
                    absent=("Save and continue", "models done"),
                )["screen"]
                assert all(event.wait(5) for event in held_requests.values()), trials
                assert rate_request_started.wait(5), trials
                text, thinking, image = model_cells(save_activity, "alpha")
                assert text == "Ready" and re.fullmatch(r"none\+1 \d+s", thinking) and image == "Not tested", save_activity
                for model, effort in (("beta", "low"), ("gamma", "medium")):
                    text, thinking, image = model_cells(save_activity, model)
                    assert text.startswith("Testing ") and re.search(rf"{effort} \d+s$", thinking) and image == "Not tested", save_activity
                assert save_activity.splitlines()[-2].startswith("│Esc previous step")
                overall(save_activity, 25, 6)
                ticking = assert_live_elapsed(terminal, save_activity)
                wide = assert_wide_setup(terminal)
                assert_testing_navigation(terminal, config)
                allow_rate_limit.set()
                save_retry = terminal.expect("Retry ", also=("5 running", "2 waiting"), rows=20)["screen"]
                release_beta_effort.set()
                partial = terminal.expect("Testing models", also=("low", "4 running", "2 waiting"), rows=20)["screen"]
                text, thinking, image = model_cells(partial, "beta")
                assert text.startswith("Testing ") and thinking == "low" and image == "Not tested", partial
                assert model_cells(save_retry, "alpha")[1].startswith("Retry "), save_retry
                overall(partial, 41, 10)
                assert overview_layout(save_activity) == overview_layout(save_retry) == overview_layout(partial)
                terminal.press("\r")
                progress = terminal.expect(
                    "Retry in", also=("Thinking none", "Text reply", "Testing", "API listed 3 values", "Esc previous step"), rows=20,
                )["screen"]
                assert progress.splitlines()[-2].startswith("│Esc previous step")
                assert not any(row[:2] == ("alpha", "max") for row in trials)
                release_beta_text.set()
                terminal.press("\x1b")
                after_text = terminal.expect("58%", also=("› View requests",), rows=20)["screen"]
                overall(after_text, 58, 14)
                terminal.press("\r")
                terminal.expect("Model tests")
                release_requests.set()
                finished = terminal.expect(
                    "9/9 finished", also=("Usable · Thinking seen", "Rejected", "Thinking low", "Cost"), rows=20,
                )["screen"]
                assert finished.count("alpha") == 1 and finished.count("beta") == 1, finished
                result_rows = [next(line for line in finished.splitlines() if label in line)
                               for label in ("Thinking none", "Thinking high", "Thinking low")]
                assert len({line.index(result) for line, result in zip(result_rows, ("Usable", "Rejected", "Usable"))}) == 1, finished
                terminal.press("\x1b[B\x1b[B")
                terminal.expect("Thinking medium", also=("gamma",))
                terminal.press("\x1b")
                save_results = terminal.expect(
                    "Save setup", also=("› Save and continue", "100%", "none, max", "low", "medium"),
                    absent=("Retry in", "Save early"), rows=20,
                )["screen"]
                overall(save_results, 100, 24)
                assert overview_layout(save_activity) == overview_layout(save_results)
                for model, levels in (("alpha", "none, max"), ("beta", "low"), ("gamma", "medium")):
                    assert model_cells(save_results, model) == ("Ready", levels, "Not tested"), save_results
                assert save_results.splitlines()[-2].startswith("│Esc previous step")
                terminal.press("\x1b[B\x1b[B\r")
                terminal.expect("› Thinking")
                terminal.press("\r")
                terminal.expect("2 usable", also=("› none · send to provider", "max"), absent=("high",))
                terminal.press("\x1b")
                terminal.expect("Model capabilities")
                terminal.press("\x1b")
                terminal.expect("› Save and continue")
                terminal.press("\r")
                terminal.expect("New session", state="Ready")
                saved_models = client.request("GET", "/models").as_json()["tiers"]
                assert {item["model"]: item["settings"]["reasoning_effort"] for item in saved_models} == {
                    "alpha": "none", "beta": "low", "gamma": "medium",
                }
                terminal.submit("Verify the saved thinking setting")
                terminal.expect("Saved effort sent.", state="Finished")
                assert runtime_requests == [("alpha", "none")]
                assert sum(row[:2] == ("alpha", None) for row in trials) == 1
                retry_times = [stamp for model, effort, stamp in trials if (model, effort) == ("alpha", "none")]
                assert len(retry_times) == 2 and retry_times[1] - retry_times[0] >= 2
                terminal.exit_cleanly()
            report = {"discovery": discovery, "save_activity": save_activity, "ticking": ticking, "wide": wide, "save_retry": save_retry, "save_partial": partial, "save_results": save_results, "countdown": progress, "results": finished, "confirmed_in_flight": 6, "requests": [(m, e) for m, e, _ in trials], "runtime": runtime_requests}
            (client.directory / "probe-progress.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    finally:
        release.set()
        allow_rate_limit.set()
        release_beta_text.set()
        release_beta_effort.set()
        release_requests.set()


def persistent_model_default(client):
    workspace = client.workspace("work-model-default")
    chosen = "fixture-saved-model"
    config_path = client.home / "config.toml"
    config_path.write_text(config_path.read_text(encoding="utf-8").replace(
        '[model.light]\nprovider = "test-anthropic"\nmodel = "claude-sonnet-4-6"',
        f'[model.light]\nprovider = "test-anthropic"\nmodel = "{chosen}"',
    ), encoding="utf-8")
    prompt = "Verify the separately chosen Session model"
    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.press("/model\r")
        models = terminal.expect("light", also=(chosen,))["screen"]
        initial_models = models
        terminal.click_text(models, "light")
        tier_return = terminal.expect("New session", state="Ready", also=(chosen,),
                                      absent=("Model route",))["screen"]
        terminal.submit(prompt)
        call = client.provider.next(prompt)
        assert call.request["model"] == chosen, call.request["model"]
        call.text("The Session model is selected.")
        terminal.expect("The Session model is selected.", state="Finished")
        session = client.session(workspace)

        terminal.press("/model\r")
        terminal.expect("balanced", also=("Model route",))
        terminal.press("/balanced\r")
        terminal.expect("The Session model is selected.", absent=("Model route",))
        eventually("the current Session uses balanced",
                   lambda: client.snapshot(workspace, session)["config"].get("tier") == "balanced")

        terminal.press("/model\r")
        terminal.press("/light")
        terminal.expect("/ light", also=("›light",))
        terminal.press("\x1b")
        models = terminal.expect("d Default + switch", also=(chosen,))["screen"]
        terminal.press("d")
        eventually("the service loads light as the default model",
                   lambda: client.request("GET", "/models").as_json()["default_tier"] == "light")
        saved = terminal.expect("Settings saved for the next safe model boundary", also=(chosen,))["screen"]
        eventually("the current Session uses light",
                   lambda: client.snapshot(workspace, session)["config"].get("tier") == "light")
        terminal.exit_cleanly()
    config = (client.home / "config.toml").read_text(encoding="utf-8")
    assert 'default_model = "light"' in config and chosen in config
    client.cli(workspace, "server", "restart")
    new_workspace = client.workspace("work-new-default")
    with client.terminal(new_workspace) as terminal:
        terminal.expect("New session", state="Ready", also=(chosen,))
        terminal.submit("Verify the new Session default")
        created = client.provider.next("Verify the new Session default")
        assert created.request["model"] == chosen, created.request["model"]
        created.text("The saved default opened a new Session.")
        terminal.expect("The saved default opened a new Session.", state="Finished")
        terminal.exit_cleanly()
    client.cli(workspace, "server", "restart")
    with client.terminal(new_workspace) as terminal:
        terminal.resume("Verify the new Session default")
        terminal.expect("The saved default opened a new Session.")
        terminal.submit("Verify the reopened Session model")
        reopened = client.provider.next("Verify the reopened Session model")
        assert reopened.request["model"] == chosen, reopened.request["model"]
        reopened.text("The reopened Session kept its model.")
        terminal.expect("The reopened Session kept its model.", state="Finished")
        terminal.exit_cleanly()
    client.provider.assert_idle()
    (client.directory / "model-default.json").write_text(json.dumps({
        "initial_models_screen": initial_models,
        "screen_after_tier_click": tier_return,
        "models_before_save": models,
        "saved_default_screen": saved,
        "current_session_model": chosen,
        "new_session_model": created.request["model"],
        "reopened_session_model": reopened.request["model"],
    }, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"Persistent default {chosen} was used by current, new, and reopened Sessions.")


def readonly_rewind(client):
    workspace = client.workspace("work-readonly-rewind")
    kept_prompt = "Keep the active context anchor"
    kept_reply = "ACTIVE-CONTEXT-ANCHOR"
    discarded_prompt = "Remove this later message from context"
    discarded_reply = "EXCLUDED-READONLY-REPLY"
    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        for prompt, reply in ((kept_prompt, kept_reply), (discarded_prompt, discarded_reply)):
            terminal.submit(prompt)
            client.provider.next(prompt).text(reply)
            terminal.expect(reply, state="Finished")
        session = client.session(workspace)
        query = urlencode({"workspace": workspace, "agent": "main"})
        history = client.request("GET", f"/sessions/{session}/history?{query}").as_json()
        discarded = next(entry for entry in history["entries"] if entry.get("text") == discarded_prompt)
        rewind = client.request("POST", f"/sessions/{session}/commands", payload={
            "type": "rewind_message", "workspace": str(workspace), "agent": "main",
            "command_id": "readonly-fixture-rewind", "target_event_id": discarded["id"],
            "text": discarded_prompt,
        }).as_json()
        assert rewind["ok"], rewind
        terminal.expect("Rewound content · read-only", absent=(discarded_reply,))
        terminal.type("Continue from the active anchor")
        screen = terminal.expect("Rewound content · read-only")["screen"]
        terminal.click_text(screen, "Messages" if "Messages" in screen else "Msg")
        messages = terminal.expect("Rewound · read-only")["screen"]
        assert discarded_reply not in messages
        terminal.click_text(messages, "Expand group")
        readonly = terminal.expect(discarded_reply, also=("excluded from model context", "Esc input"))["screen"]
        terminal.press("\x1b")
        return_screen = readonly
        enter_sent = False

        def composer_returned():
            nonlocal return_screen
            return_screen = terminal.request("screen")["screen"]
            return return_screen if (
                "Continue from the active anchor" in return_screen
                and "Reading history · Editing" in return_screen
                and "Esc input" not in return_screen
            ) else None

        try:
            eventually("Esc to restore composer focus before Enter", composer_returned)
            terminal.press("\r")
            enter_sent = True
            continued = client.provider.next("Continue from the active anchor")
        except AssertionError as error:
            screen = terminal.request("screen")["screen"]
            query = urlencode({"workspace": workspace, "agent": "main", "progress": "true"})
            snapshot = client.request("GET", f"/sessions/{session}/snapshot?{query}").as_json()
            diagnostic = {
                "expanded_screen": readonly, "screen_after_escape": return_screen,
                "screen_after_enter": screen, "agent_status": snapshot["agent"]["status"],
                "pending_inputs": snapshot["pending"], "enter_sent": enter_sent,
            }
            (client.directory / "readonly-rewind-failure.json").write_text(json.dumps(
                diagnostic, ensure_ascii=False, indent=2,
            ), encoding="utf-8")
            raise AssertionError(f"{error}\nLast terminal screen:\n{screen}\n"
                                 f"Agent: {diagnostic['agent_status']} · pending: {len(snapshot['pending'])}") from error
        context = json.dumps(continued.request["messages"])
        assert kept_reply in context
        assert discarded_prompt not in context and discarded_reply not in context
        continued.text("Continued from the active context only.")
        terminal.press("\x1b[1;5F")
        terminal.expect("Continued from the active context only.", state="Finished")
        terminal.exit_cleanly()
    client.cli(workspace, "server", "restart")
    with client.terminal(workspace) as terminal:
        terminal.resume(kept_prompt)
        screen = terminal.expect("Rewound content · read-only")["screen"]
        terminal.click_text(screen, "Messages" if "Messages" in screen else "Msg")
        messages = terminal.expect("Expand group")["screen"]
        terminal.click_text(messages, "Expand group")
        terminal.expect(discarded_reply, also=("excluded from model context",))
        terminal.exit_cleanly()
    query = urlencode({"workspace": workspace, "agent": "main"})
    history = client.request("GET", f"/sessions/{session}/history?{query}").as_json()
    assert history["rewind_groups"]
    client.provider.assert_idle()
    (client.directory / "readonly-rewind.json").write_text(json.dumps({
        "session": session,
        "rewind_groups": history["rewind_groups"],
        "readonly_screen": readonly,
        "screen_after_escape": return_screen,
        "composer_return_observed": "Esc input" not in return_screen and "Editing" in return_screen,
        "active_anchor_in_next_request": kept_reply in context,
        "discarded_content_in_next_request": discarded_reply in context,
    }, ensure_ascii=False, indent=2), encoding="utf-8")
    print("Rewound history stayed readable after restart and was absent from the next provider context.")


def keyboard_queue_withdrawal(client):
    workspace = client.workspace("work-keyboard-queue")
    prompt = "Hold a reply while queued inputs are reviewed"
    kept = "Keep this exact queued instruction"
    withdrawn = "Withdraw this exact queued instruction"
    draft = "independent draft survives queue processing"
    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.submit(prompt)
        response = client.provider.next(prompt)
        with response.streaming_text("Keyboard queue check in progress", "\n\nKeyboard queue check finished"):
            terminal.expect("Keyboard queue check in progress", state="↓  Reply")
            for text in (kept, withdrawn):
                terminal.type(text)
                terminal.press("\x1b\r")
                terminal.expect(text, state="Queued · next Turn")
            session = client.session(workspace)
            query = urlencode({"workspace": workspace, "agent": "main"})

            def pending_inputs():
                return client.request("GET", f"/sessions/{session}/snapshot?{query}").as_json()["pending"]

            pending = eventually("both exact queued inputs", lambda: (
                value if len(value := pending_inputs()) == 2 else None
            ))
            ids = {entry["input"]["text"]: entry["receipt"]["command_id"] for entry in pending}
            assert set(ids) == {kept, withdrawn}, pending

            def both_queue_rows_visible():
                screen = terminal.request("screen")["screen"]
                return all(queued_input_click_position(screen, text) is not None
                           for text in (kept, withdrawn))

            eventually("both exact inputs to appear in the pending-input panel", both_queue_rows_visible)
            terminal.press("\x1b[A")

            def selected_input():
                value = terminal.request("screen")["screen"]
                return value if has_selected_queued_input(value, withdrawn) else None

            try:
                selected = eventually("Up to select the newest exact queued input", selected_input)
            except AssertionError as error:
                raise AssertionError(
                    f"{error}\nLast terminal screen:\n{terminal.request('screen')['screen']}"
                ) from error
            terminal.press("\x1b[3~")
            restored = terminal.expect(withdrawn, state="Draft", also=(kept, "Removed from queue"))["screen"]
            def remaining_input():
                value = pending_inputs()
                return value if [entry["receipt"]["command_id"] for entry in value] == [ids[kept]] else None

            remaining = eventually("keyboard withdrawal to leave the other input queued", remaining_input)
            rows = restored.splitlines()
            heading = next(index for index, row in enumerate(rows) if is_composer_heading(row))
            composer = "\n".join(rows[heading:])
            assert withdrawn in composer, restored
            terminal.press("\x03")
            terminal.type(draft)
        continued = client.provider.next(kept)
        messages = json.dumps(continued.request["messages"])
        assert messages.count(kept) == 1 and withdrawn not in messages
        continued.text("The remaining queued instruction ran once.")
        terminal.expect("The remaining queued instruction ran once.", state="Finished", also=(draft,))
        terminal.press("\x03")
        terminal.exit_cleanly()
    client.provider.assert_idle()
    (client.directory / "keyboard-queue-withdrawal.json").write_text(json.dumps({
        "session": session, "queued_ids": ids, "remaining": remaining,
        "selected_screen": selected, "restored_draft_screen": restored,
        "kept_input_occurrences_in_request": messages.count(kept),
        "withdrawn_input_in_request": withdrawn in messages,
    }, ensure_ascii=False, indent=2), encoding="utf-8")
    print("Keyboard Up/Delete withdrew the exact input, restored its draft and kept the other input queued.")


def runtime_credential_repair(binary, bridge, directory):
    instance = InstalledClient(binary, bridge, directory)
    repaired_key = "fixture-repaired-key"
    attempts = []
    original_dispatch = instance.provider.dispatch

    def dispatch(method, path, headers, body):
        if method == "POST" and path == "/v1/messages":
            key = headers.get("x-api-key")
            if key == "test-key":
                attempts.append("original")
                return json_response({"type": "error", "error": {
                    "type": "authentication_error", "message": "fixture credential rejected",
                }}, 401)
            assert key == repaired_key, "Unexpected credential reached the fixture provider"
            attempts.append("repaired")
        return original_dispatch(method, path, headers, body)

    instance.provider.dispatch = dispatch
    with instance as client:
        workspace = client.workspace("work-auth-repair")
        prompt = "AUTH-REPAIR-ORIGINAL-INPUT-481"
        draft = "An independent draft survives credential repair"
        config_path = client.home / "config.toml"
        client.env["KUKU_ACCEPTANCE_REJECTED_KEY"] = "test-key"
        config_path.write_text(config_path.read_text(encoding="utf-8").replace(
            'api_key = "test-key"', 'api_key = "$KUKU_ACCEPTANCE_REJECTED_KEY"',
        ), encoding="utf-8")
        with client.terminal(workspace) as terminal:
            terminal.expect("New session", state="Ready")
            terminal.submit(prompt)
            failure = terminal.expect("Authentication error", state="Interrupted", also=("No automatic retry",))["screen"]
            session = client.session(workspace)
            original = client.connection()
            original_config = config_path.read_text(encoding="utf-8")
            terminal.type(draft)
            focus_action(terminal, "Repair credentials")
            terminal.press("\r")
            terminal.expect("Repair provider credentials", also=("Paste",))
            terminal.press(f"\x1b[200~{repaired_key}\x1b[201~")
            masked = terminal.expect("•" * len(repaired_key))["screen"]
            assert repaired_key not in masked
            terminal.press("\r")
            review = terminal.expect("Save credentials and restart local service")["screen"]
            assert attempts == ["original"], attempts
            terminal.press("\r")
            applied = terminal.expect("No retry has been sent.")["screen"]
            restarted = client.connection()
            assert restarted["instance_id"] != original["instance_id"]
            assert restarted["config_path"] == original["config_path"]
            assert attempts == ["original"], "Repair or restart sent a model request"
            assert client.session(workspace) == session
            terminal.press("\r")
            terminal.expect(draft, also=("Continue",))
            focus_action(terminal, "Continue")
            terminal.press("\r")
            retried = client.provider.next(prompt)
            context = json.dumps(retried.request["messages"])
            assert context.count(prompt) == 1
            retried.text("The repaired credential retried the retained input once.")
            terminal.expect("The repaired credential retried the retained input once.", state="Finished", also=(draft,))
            assert attempts == ["original", "repaired"], attempts
            query = urlencode({"workspace": workspace, "agent": "main", "limit": 64})
            entries = client.request("GET", f"/sessions/{session}/history?{query}").as_json()["entries"]
            retained = [entry for entry in entries if entry["type"] == "user" and entry["text"] == prompt]
            assert len(retained) == 1
            terminal.press("\x03")
            terminal.exit_cleanly()
        saved_text = config_path.read_text(encoding="utf-8")
        assert repaired_key not in saved_text
        assert re.search(r'''(?m)^api_key = (["'])file:[^\r\n]+\1$''', saved_text)
        normalize = lambda text: re.subn(r"(?m)^api_key = .*?$", "api_key = <credential>", text)
        saved_config, saved_keys = normalize(saved_text)
        unchanged_config, original_keys = normalize(original_config)
        assert saved_keys == original_keys == 1
        assert saved_config == unchanged_config, "Repair changed unrelated provider or model settings\n" + "\n".join(
            difflib.unified_diff(unchanged_config.splitlines(), saved_config.splitlines())
        )
        assert client.env["KUKU_ACCEPTANCE_REJECTED_KEY"] == "test-key"
        credential_files = list((client.home / "credentials").iterdir())
        assert len(credential_files) == 1 and credential_files[0].read_text(encoding="utf-8") == repaired_key
        client.provider.assert_idle()
        (client.directory / "auth-repair.json").write_text(json.dumps({
            "session": session, "failed_screen": failure, "masked_screen": masked,
            "review_screen": review, "applied_screen": applied,
            "provider_attempts": attempts, "service_instance_changed": True,
            "config_path_unchanged": True, "unrelated_config_preserved": True,
            "retained_user_messages": len(retained), "retry_input_occurrences": context.count(prompt),
            "private_credential_files": len(credential_files),
        }, ensure_ascii=False, indent=2), encoding="utf-8")
    print("Authentication repair saved a private credential, safely restarted and retried the same input only after Continue.")
