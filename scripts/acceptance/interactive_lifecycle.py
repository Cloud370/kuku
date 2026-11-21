"""Installed-terminal checks for settings, defaults and retained rewind history."""

import difflib
import json
import re
import threading
import time
from urllib.parse import urlencode

from interactive_fixture import InstalledClient, Response, TOOL_BRIEF, TOOL_OUTPUT, eventually, json_response, run_cases
from interactive_rendering import has_selected_queued_input


def lifecycle_cases(binary, bridge, directory):
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
                 ))
    cases.append(("model-setup", lambda: shared_model_editor(binary, bridge, directory / "model-setup")))
    cases.append(("setup-probe-progress", lambda: setup_probe_progress(binary, bridge, directory / "probe-progress")))
    cases.append(("auth-repair", lambda: runtime_credential_repair(binary, bridge, directory / "auth-repair")))
    run_cases(cases)


def verify_restored_tool(terminal, job, history):
    if job is not None:
        terminal.press("\x0f")
        terminal.expect("Ctrl+O show")
        terminal.press("\x1b[5~")
        screen = terminal.expect(TOOL_BRIEF, also=("started in background", "Background job",
                                                 f"{job['id']} · Completed"))["screen"]
        assert screen.count(f"╭─ {TOOL_BRIEF}") == 1 and screen.count("Background job") == 1, screen
        terminal.press("\x1b[6~")
        terminal.press("\x0f")
        terminal.expect(TOOL_OUTPUT)
        return
    screen = terminal.expect(TOOL_BRIEF)["screen"]
    assert screen.count(f"╭─ {TOOL_BRIEF}") == 1 and "╰─ ok" in screen, f"{screen}\nCommitted tool: {history}"
    assert "--gated-tool" in screen and TOOL_OUTPUT in screen
    terminal.press("\x0f")
    collapsed = terminal.expect("Ctrl+O show")["screen"]
    assert "Saved output" not in collapsed and "--gated-tool" not in collapsed
    terminal.press("\x0f")
    terminal.expect(TOOL_BRIEF, also=(TOOL_OUTPUT,))


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
        config.write_text(config.read_text().replace(
            'api_key = "test-key"', 'api_key = "$KUKU_ACCEPTANCE_MISSING_KEY"'
        ))
        workspace = client.workspace("work-first-run")
        prompt = "Verify the repaired first-run settings"
        with client.terminal(workspace) as terminal:
            terminal.expect("Find and test models", also=("may cost money",))
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
            terminal.expect("Save and continue", also=("2/2 finished", "1 to check"))
            terminal.press("\x1b[B\r")
            terminal.expect(
                "Image not verified",
                also=("hit output limit", "Text reply received", "Cost · Unknown"),
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
        assert key not in config.read_text()
        credential_files = list((client.home / "credentials").iterdir())
        assert len(credential_files) == 1
        assert credential_files[0].read_text() == key
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
        endpoint = re.search(r'base_url = "(http://127\.0\.0\.1:[0-9]+)"', config.read_text())[1]
        config.write_text("")
        workspace = client.workspace("work-model-setup")
        with client.terminal(workspace) as terminal:
            terminal.request("resize", rows=32, cols=110)
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
                "Save setup", also=("Default", "beta", "API URL", "Settings", "› Save and continue"),
                absent=("Model capabilities", "Save and exit"),
            )["screen"]
            terminal.press("\x1b[B\x1b[B\r")
            settings_review = terminal.expect(
                "Model capabilities", also=("Provider default", "Costs", "Save and exit"),
                absent=("API URL",),
            )["screen"]
            terminal.press("\x1b")
            terminal.expect("› Save and continue")
            terminal.press("\r")
            terminal.expect("New session", state="Ready", absent=("Connected", "F2 Restart"))
            terminal.request("resize", rows=32, cols=110)
            terminal.submit("Use the first saved default")
            first = client.provider.next("Use the first saved default")
            assert first.request["model"] == "beta"
            assert "thinking" not in first.request and "output_config" not in first.request
            first.text("Saved default sent a text turn.")
            terminal.expect("Saved default sent a text turn.", state="Finished")

            draft = "keep the unsent draft"
            terminal.type(draft)
            terminal.press("\x0c")
            terminal.expect("Add model or connection")
            terminal.press("\r")
            terminal.expect("Set up a model")
            terminal.press("\r")
            terminal.expect("Existing · custom")
            terminal.press("\r")
            terminal.expect("Choose models")
            terminal.press("\x1b")
            terminal.expect("Enter model ID")
            terminal.press("\x1b[B\r")
            terminal.expect("Model ID")
            terminal.type("gamma")
            terminal.press("\r")
            terminal.expect("Save models", also=("API URL", "gamma"))
            terminal.press("\r")
            terminal.expect("Tier · gamma")
            terminal.press("gamma")
            match = terminal.expect("/ gamma", also=("› Tier · gamma",))["screen"]
            terminal.click_text(match, "Tier · gamma")
            terminal.expect("Use in this Session", also=("Edit", "Set as default"))
            terminal.press("\x1b[B\r")
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
            terminal.expect("Save models", also=("API URL", "delta"))
            terminal.press("\r")
            terminal.expect("Tier · gamma", also=("delta",))
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
            terminal.expect("Tier · gamma", also=("delta",))
            terminal.press("gamma")
            match = terminal.expect("/ gamma", also=("› Tier · gamma",))["screen"]
            terminal.click_text(match, "Tier · gamma")
            terminal.expect("Use in this Session")
            terminal.press("\r")
            terminal.expect(draft, also=("delta",), absent=("Model route", "Model settings"))
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
    slow_catalog_started = threading.Event()
    beta_started = threading.Event()
    release_beta = threading.Event()
    runtime_requests = []
    release = threading.Event()
    trials = []

    def dispatch(method, path, headers, body):
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
        trials.append((model, effort, time.monotonic()))
        if model == "beta" and effort is None:
            beta_started.set()
            release_beta.wait(20)
        if effort == "__kuku_unknown_effort__":
            return json_response({"error": {"param": "reasoning_effort", "allowed_values": ["none", "high"]}}, 400)
        if model == "alpha" and effort == "none" and sum(row[:2] == ("alpha", "none") for row in trials) == 1:
            assert beta_started.wait(8)
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
    try:
        with instance as client:
            config = client.home / "config.toml"
            endpoint = re.search(r'base_url = "(http://127\.0\.0\.1:[0-9]+)"', config.read_text())[1]
            config.write_text("")
            with client.terminal(client.workspace("work-probe-progress")) as terminal:
                terminal.expect("Find and test models", also=("Edit config file", "may cost money"))
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
                discovery = terminal.expect("Choose models", also=("alpha", "beta"))["screen"]
                assert slow_catalog_started.is_set()
                assert not trials
                terminal.press("alpha \x01beta \r")
                terminal.expect("Choose the default model")
                terminal.press("\r")
                terminal.expect("Save setup", also=(f"{endpoint}/v1/chat/completions",))
                terminal.press("\x1b[B\r")
                progress = terminal.expect("Retry in", also=("none · alpha", "Waiting for response", "API listed 2 values", "Esc previous step"))["screen"]
                assert not any(row[:2] == ("beta", "low") for row in trials)
                release_beta.set()
                finished = terminal.expect("6/6 finished", also=("Usable · none · alpha · Thinking seen", "Rejected · high · alpha", "Usable · low · beta"))["screen"]
                terminal.press("\x1b")
                terminal.expect("Save setup", also=("› Save and continue",))
                terminal.press("\x1b[B\x1b[B\r")
                terminal.expect("› Thinking")
                terminal.press("\r")
                terminal.expect("1 usable", also=("none · send to provider",), absent=("high",))
                terminal.press("\x1b[B\r")
                terminal.expect("Model capabilities")
                terminal.press("\x1b")
                terminal.expect("› Save and continue")
                terminal.press("\r")
                terminal.expect("New session", state="Ready")
                terminal.submit("Verify the saved thinking setting")
                terminal.expect("Saved effort sent.", state="Finished")
                assert runtime_requests == [("alpha", "none")]
                assert sum(row[:2] == ("alpha", None) for row in trials) == 1
                retry_times = [stamp for model, effort, stamp in trials if (model, effort) == ("alpha", "none")]
                assert len(retry_times) == 2 and retry_times[1] - retry_times[0] >= 2
                terminal.exit_cleanly()
            report = {"discovery": discovery, "countdown": progress, "results": finished, "requests": [(m, e) for m, e, _ in trials], "runtime": runtime_requests}
            (client.directory / "probe-progress.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    finally:
        release.set()
        release_beta.set()


def persistent_model_default(client):
    workspace = client.workspace("work-model-default")
    chosen = "fixture-saved-model"
    config_path = client.home / "config.toml"
    config_path.write_text(config_path.read_text().replace(
        '[model.light]\nprovider = "test-anthropic"\nmodel = "claude-sonnet-4-6"',
        f'[model.light]\nprovider = "test-anthropic"\nmodel = "{chosen}"',
    ))
    prompt = "Verify the separately chosen Session model"
    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.press("/model\r")
        models = terminal.expect("Tier · light", also=(chosen,))["screen"]
        initial_models = models
        tier_click = terminal.click_text(models, "Tier · light")
        tier_selected = terminal.expect("Use in this Session", also=(chosen,))["screen"]
        terminal.press("\r")
        tier_return = terminal.expect("New session", state="Ready", also=(chosen,),
                                      absent=("Model route",))["screen"]
        terminal.submit(prompt)
        call = client.provider.next(prompt)
        assert call.request["model"] == chosen, call.request["model"]
        call.text("The Session model is selected.")
        terminal.expect("The Session model is selected.", state="Finished")
        session = client.session(workspace)
        before = client.snapshot(workspace, session)["config"]
        config_before_default = config_path.read_text()

        def open_default_confirmation():
            terminal.press("/model\r")
            models = terminal.expect("Model route", also=(
                "Set current model as default", "Tier · light", chosen,
            ))["screen"]
            clicked = terminal.click_text(models, "Set current model as default")
            try:
                confirmation = terminal.expect("Save as default", also=(chosen, "new sessions"))["screen"]
            except AssertionError:
                (client.directory / "model-default-failure.json").write_text(json.dumps({
                    "initial_models_screen": initial_models, "tier_click": tier_click,
                    "screen_after_tier_enter": tier_return, "models_before_default_click": models,
                    "default_click": clicked, "last_screen": terminal.request("screen")["screen"],
                    "session_config_before": before,
                    "session_config_after": client.snapshot(workspace, session)["config"],
                    "saved_default_is_light": 'default_model = "light"' in config_path.read_text(),
                }, ensure_ascii=False, indent=2), encoding="utf-8")
                raise
            return models, clicked, confirmation

        cancel_models, default_click, _ = open_default_confirmation()
        cancel_selected = terminal.expect("› Cancel", also=("Save as default", chosen))["screen"]
        terminal.press("\r")
        cancel_return = terminal.expect("The Session model is selected.", state="Finished",
                                       absent=("Save as default", "Model route"))["screen"]
        assert config_path.read_text() == config_before_default
        assert client.snapshot(workspace, session)["config"] == before
        models, save_click, confirmation = open_default_confirmation()
        terminal.expect("› Cancel", also=(chosen, "new sessions"))
        terminal.press("\x1b[A")
        save_selected = terminal.expect("› Save as default", also=(chosen, "new sessions"))["screen"]
        terminal.press("\r")
        terminal.expect("Saved as default")
        assert client.snapshot(workspace, session)["config"] == before
        terminal.exit_cleanly()
    config = (client.home / "config.toml").read_text()
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
        "initial_models_screen": initial_models, "tier_click": tier_click,
        "selected_tier_screen": tier_selected,
        "screen_after_tier_enter": tier_return, "models_before_default_click": cancel_models,
        "models_before_save_click": models, "default_click": default_click, "save_click": save_click,
        "cancel_selected_screen": cancel_selected, "cancel_return_screen": cancel_return,
        "cancel_preserved_config": True, "save_selected_screen": save_selected,
        "confirmation_screen": confirmation,
        "unchanged_session_config": before,
        "new_session_model": created.request["model"],
        "reopened_session_model": reopened.request["model"],
    }, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"Persistent default {chosen} was used by new and reopened Sessions; saving kept the current config unchanged.")


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
        terminal.press("\x1b")
        terminal.expect("Press Esc again")
        terminal.press("\x1b")
        terminal.expect("Rewind to message", also=(discarded_prompt,))
        terminal.press("\r")
        terminal.expect("Enter commits", also=(discarded_prompt,))
        terminal.press("\r")
        terminal.expect("Rewound content · read-only", also=(discarded_prompt,))
        terminal.press("\x03")
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
            terminal.expect("Keyboard queue check in progress", state="Streaming")
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
            terminal.press("\x1b[A")

            def selected_input():
                value = terminal.request("screen")["screen"]
                return value if has_selected_queued_input(value, withdrawn) else None

            selected = eventually("Up to select the newest exact queued input", selected_input)
            terminal.press("\x1b[3~")
            restored = terminal.expect(withdrawn, state="Draft", also=(kept, "Input withdrawn"))["screen"]
            def remaining_input():
                value = pending_inputs()
                return value if [entry["receipt"]["command_id"] for entry in value] == [ids[kept]] else None

            remaining = eventually("keyboard withdrawal to leave the other input queued", remaining_input)
            composer = restored[restored.index("To main"):]
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


def paginated_rewind(client):
    workspace = client.workspace("work-rewind-history")
    prompts = [f"HISTORY-PAGE-USER-{index:02d}" for index in range(1, 27)]
    replies = [f"HISTORY-PAGE-ANSWER-{index:02d}" for index in range(1, 27)]
    query = {"workspace": workspace, "agent": "main", "limit": 48}

    def history():
        return client.request("GET", f"/sessions/{session}/history?{urlencode(query)}").as_json()

    def oldest_message(terminal):
        terminal.press("\x1b")
        terminal.expect("Press Esc again")
        terminal.press("\x1b")
        screen = terminal.expect("Rewind to message", also=(
            prompts[-1], "↑ at the top loads earlier messages",
        ))["screen"]
        for steps in range(40):
            if re.search(r"› Turn 1(?:\s|$)", screen) and prompts[0] in screen:
                return steps, screen
            terminal.press("\x1b[5~")

            def moved():
                value = terminal.request("screen")["screen"]
                return value if value != screen and "Loading earlier messages…" not in value else None

            try:
                screen = eventually("rewind selection to advance toward the earliest message", moved)
            except AssertionError as error:
                screen = terminal.request("screen")["screen"]
                raise AssertionError(f"{error}\nLast terminal screen:\n{screen}") from error
        raise AssertionError(f"Could not select the earliest committed message:\n{screen}")

    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        first_id = None
        for prompt, reply in zip(prompts, replies):
            terminal.submit(prompt)
            client.provider.next(prompt).text(reply)
            terminal.expect(reply, state="Finished")
            if first_id is None:
                session = client.session(workspace)
                first_id = next(entry["id"] for entry in history()["entries"]
                                if entry["type"] == "user" and entry["text"] == prompt)
        terminal.exit_cleanly()
    before = history()
    assert before["next_before"] is not None
    assert all(entry["id"] != first_id for entry in before["entries"])
    with client.terminal(workspace) as terminal:
        terminal.resume(prompts[0])
        terminal.expect(replies[-1], state="Finished")
        cancelled_steps, cancelled = oldest_message(terminal)
        assert cancelled_steps > 0
        terminal.press("\x1b")
        terminal.expect("To main", state="Finished")
        assert history() == before, "Browsing and cancelling rewind changed durable history"
        client.provider.assert_idle()
        confirmed_steps, selected = oldest_message(terminal)
        terminal.press("\r")
        confirmation = terminal.expect("Rewind to message · Turn 1", also=(prompts[0], "Enter commits"))["screen"]
        terminal.press("\r")
        terminal.expect("Rewound content · read-only", also=(prompts[0],))
        rewound = history()
        assert [group["target_event_id"] for group in rewound["rewind_groups"]] == [first_id]
        assert not any(entry["type"] == "user" for entry in rewound["entries"])
        terminal.press("\x03")
        terminal.submit("Continue after the earliest paginated rewind")
        call = client.provider.next("Continue after the earliest paginated rewind")
        context = json.dumps(call.request["messages"])
        assert all(text not in context for text in prompts + replies)
        call.text("The earliest message was selected across history pages.")
        terminal.press("\x1b[1;5F")
        terminal.expect("The earliest message was selected across history pages.", state="Finished")
        terminal.exit_cleanly()
    client.provider.assert_idle()
    (client.directory / "rewind-history.json").write_text(json.dumps({
        "session": session, "committed_messages": len(prompts) + len(replies),
        "initial_next_before": before["next_before"], "target_event_id": first_id,
        "cancelled_page_steps": cancelled_steps, "confirmed_page_steps": confirmed_steps,
        "cancelled_selection_screen": cancelled, "confirmed_selection_screen": selected,
        "confirmation_screen": confirmation, "rewind_groups": rewound["rewind_groups"],
        "discarded_content_in_next_request": any(text in context for text in prompts + replies),
    }, ensure_ascii=False, indent=2), encoding="utf-8")
    print("Rewind loaded beyond 48 committed messages, cancelled safely and confirmed the exact earliest event.")


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
        config_path.write_text(config_path.read_text().replace(
            'api_key = "test-key"', 'api_key = "$KUKU_ACCEPTANCE_REJECTED_KEY"',
        ))
        with client.terminal(workspace) as terminal:
            terminal.expect("New session", state="Ready")
            terminal.submit(prompt)
            failure = terminal.expect("Authentication error", state="Interrupted", also=("No automatic retry",))["screen"]
            session = client.session(workspace)
            original = client.connection()
            original_config = config_path.read_text()
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
        saved_text = config_path.read_text()
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
        assert len(credential_files) == 1 and credential_files[0].read_text() == repaired_key
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
