"""Native terminal checks for resolving a failed setup text request."""

from collections import Counter
import json
import re
import threading

from interactive_fixture import InstalledClient, eventually, json_response, run_cases


def recovery_cases(binary, bridge, directory):
    run_cases([
        ("change default, retry one model and skip another", lambda: recover_models(binary, bridge, directory / "models")),
        ("repair a rejected key", lambda: repair_connection(binary, bridge, directory / "key", "key")),
        ("repair a missing URL", lambda: repair_connection(binary, bridge, directory / "url", "url")),
        ("confirm saving early", lambda: save_early(binary, bridge, directory / "save-early")),
    ])


def setup_address(client):
    config = client.home / "config.toml"
    endpoint = re.search(r'base_url = "(http://127\.0\.0\.1:[0-9]+)"', config.read_text())[1]
    config.write_text("")
    return endpoint


def choose(terminal, down, label):
    if down:
        terminal.press("\x1b[B" * down)
    terminal.expect(f"› {label}")
    terminal.press("\r")


def paste_key(terminal, key):
    terminal.press(f"\x1b[200~{key}\x1b[201~\r")


def assert_testing_navigation(terminal, config):
    terminal.press("\r")
    terminal.expect("Model tests", also=("6 running",))
    terminal.press("\x1b")
    terminal.expect("› View requests")
    choose(terminal, 2, "Save early")
    terminal.expect("Save before tests finish?", also=("› Keep testing", "Stop tests and save"))
    terminal.press("\r")
    terminal.expect("› View requests", also=("6 running",))
    terminal.press("\x1b")
    terminal.expect("Go back before tests finish?", also=("› Keep testing", "Stop tests and go back"))
    terminal.press("\x1b")
    terminal.expect("› View requests", also=("6 running",))
    assert config.read_text() == ""


def assert_wide_setup(terminal):
    def overview(rows, cols):
        terminal.request("resize", rows=rows, cols=cols)

        def resized_frame():
            screen = terminal.request("screen")["screen"]
            lines = screen.splitlines()
            if len(lines) == rows and len(lines[0]) == cols and all(
                label in screen for label in ("25%", "Try none, high", "› View requests")
            ):
                return screen

        screen = eventually(f"setup at {cols}×{rows}", resized_frame)
        lines = screen.splitlines()
        header = next(line for line in lines if "Model" in line and "Image" in line)
        divider = next(line for line in lines if "┼" in line)
        assert len(divider.rstrip(" │")) <= 92, screen
        return screen, header.index("Image")

    _, standard_column = overview(24, 100)
    wide, wide_column = overview(25, 280)
    assert wide_column == standard_column < 90, wide
    terminal.press("\r")
    details = terminal.expect("Model tests", rows=25)["screen"]
    receipt = next(line for line in details.splitlines() if "API type" in line)
    elapsed = re.search(r"\b\d+\.\d+s\b", receipt)
    assert elapsed and elapsed.start() < 90, details
    terminal.press("\x1b")
    terminal.expect("› View requests", rows=25)
    terminal.request("resize", rows=20, cols=60)
    terminal.expect("none+1", rows=20)
    return wide


def assert_live_elapsed(terminal, initial):
    def elapsed_time(screen):
        row = next((line for line in screen.splitlines() if line.strip(" │").startswith("beta ")), "")
        match = re.search(r"\bTesting (\d+)s", row)
        return int(match[1]) if match else -1

    before = elapsed_time(initial)
    assert before >= 0, initial

    def ticking():
        screen = terminal.request("screen")["screen"]
        if "25%" in screen and "2 done" in screen and elapsed_time(screen) > before:
            return screen

    return eventually("request time advancing before another result arrives", ticking)


def open_focused_results(terminal):
    terminal.expect("› Model tests")
    terminal.press("\r")
    terminal.expect("Model tests", also=("Enter returns",))
    terminal.press("\x1b")
    terminal.expect("Save setup")


def start_custom(terminal, address, key):
    terminal.expect("Find and test models")
    terminal.press("\r")
    choose(terminal, 0, "New custom connection")
    terminal.expect("API URL")
    terminal.type(address)
    terminal.press("\r")
    terminal.expect("Use a proxy?")
    choose(terminal, 1, "No proxy")
    terminal.expect("Paste your API key")
    paste_key(terminal, key)


def catalog(*models):
    return json_response({"data": [{"id": model, "input_modalities": ["text"], "reasoning_efforts": ["low"]}
                                   for model in models]})


def text_reply():
    return json_response({"choices": [{"message": {"content": "377"}, "finish_reason": "stop"}]})


def save_early(binary, bridge, directory):
    instance = InstalledClient(binary, bridge, directory)
    started = threading.Event()
    release = threading.Event()

    def dispatch(method, path, headers, body):
        if method == "GET":
            return catalog("fixture-model")
        if json.loads(body).get("reasoning_effort") == "low":
            started.set()
            release.wait(20)
        return text_reply()

    instance.provider.dispatch = dispatch
    try:
        with instance as client:
            endpoint = setup_address(client)
            with client.terminal(client.workspace("save-early"), "init") as terminal:
                start_custom(terminal, f"{endpoint}/v1/chat/completions", "fixture-save-early-key")
                terminal.expect("Choose models", also=("fixture-model",))
                terminal.press("\r")
                terminal.expect("› View requests", also=("Save early", "50%", "Try low"))
                assert started.wait(5)
                choose(terminal, 2, "Save early")
                terminal.expect("Save before tests finish?", also=("› Keep testing",))
                config = client.home / "config.toml"
                assert config.read_text() == ""
                terminal.press("\x1b[B")
                terminal.expect("› Stop tests and save")
                terminal.exit_cleanly("\r")
                assert f'provider = "{endpoint.removeprefix("http://")}"' in config.read_text()
    finally:
        release.set()


def recover_models(binary, bridge, directory):
    instance = InstalledClient(binary, bridge, directory)
    requests = []

    def dispatch(method, path, headers, body):
        assert headers.get("authorization") == "Bearer fixture-recovery-key"
        if method == "GET":
            assert path == "/v1/models"
            return catalog("alpha", "beta", "gamma")
        assert path == "/v1/chat/completions"
        body = json.loads(body)
        model, effort = body["model"], body.get("reasoning_effort")
        requests.append((model, effort))
        if model == "alpha" and effort is None and requests.count(("alpha", None)) == 1:
            return json_response({"error": {"message": "fixture unavailable"}}, 503)
        if model == "gamma":
            return json_response({"error": {"message": "fixture route missing"}}, 404)
        return text_reply()

    instance.provider.dispatch = dispatch
    with instance as client:
        endpoint = setup_address(client)
        with client.terminal(client.workspace("recover-models")) as terminal:
            terminal.request("resize", rows=20, cols=60)
            start_custom(terminal, f"{endpoint}/v1/chat/completions", "fixture-recovery-key")
            terminal.expect("Choose models", also=("alpha", "beta", "gamma"))
            terminal.press("alpha \x01beta \x01gamma \r")
            terminal.expect("Choose the default model")
            choose(terminal, 0, "alpha")
            first = terminal.expect("Save setup", also=("› Model tests", "Save and exit", "Keep unverified settings",
                "Use another default", "Fix failed models", "Text failed", "URL/model absent"))["screen"]
            assert "Save and continue" not in first
            initial_requests = Counter(requests)
            open_focused_results(terminal)
            choose(terminal, 3, "Use another default")
            terminal.expect("Use another default", also=("› beta",), absent=("alpha", "gamma"))
            choose(terminal, 0, "beta")
            open_focused_results(terminal)
            assert Counter(requests) == initial_requests

            choose(terminal, 3, "Fix failed models")
            terminal.expect("Choose a model to fix", also=("alpha · Text failed", "gamma · URL/model absent"))
            choose(terminal, 0, "alpha")
            terminal.expect("Fix model", also=("› Retry this model", "Skip this model"))
            choose(terminal, 0, "Retry this model")
            retried = terminal.expect("Save setup", also=("› Model tests", "Fix failed models"))["screen"]
            assert re.search(r"alpha\s+│\s+Ready", retried), retried
            final_requests = Counter(requests)
            assert final_requests[("alpha", None)] == 2, requests
            assert all(final_requests[item] == count for item, count in initial_requests.items() if item[0] != "alpha"), requests
            open_focused_results(terminal)
            choose(terminal, 3, "Fix failed models")
            terminal.expect("Fix model", also=("gamma", "Change API URL", "Skip this model"))
            choose(terminal, 2, "Skip this model")
            terminal.expect("Save setup · 2 models", also=("› Save and continue", "Ready"), absent=("gamma", "Fix failed models"))
            choose(terminal, 0, "Save and continue")
            terminal.expect("New session", state="Ready")
            stored = client.request("GET", "/models").as_json()
            assert {tier["model"] for tier in stored["tiers"]} == {"alpha", "beta"}
            assert next(tier["model"] for tier in stored["tiers"] if tier["name"] == stored["default_tier"]) == "beta"
            terminal.exit_cleanly()


def repair_connection(binary, bridge, directory, issue):
    instance = InstalledClient(binary, bridge, directory)
    requests = []

    def dispatch(method, path, headers, body):
        if method == "GET":
            return catalog("fixture-model")
        key = headers.get("authorization")
        requests.append((path, key))
        if key == "Bearer fixture-rejected-key":
            return json_response({"error": {"message": "fixture key invalid"}}, 401)
        if path.startswith("/missing/"):
            return json_response({"error": {"message": "fixture URL missing"}}, 404)
        assert path == "/v1/chat/completions" and key == "Bearer fixture-repair-key"
        return text_reply()

    instance.provider.dispatch = dispatch
    with instance as client:
        endpoint = setup_address(client)
        with client.terminal(client.workspace(f"recover-{issue}")) as terminal:
            fixed = f"{endpoint}/v1/chat/completions"
            address = endpoint if issue == "key" else f"{endpoint}/missing/v1/chat/completions"
            key = "fixture-rejected-key" if issue == "key" else "fixture-repair-key"
            start_custom(terminal, address, key)
            terminal.expect("Choose models", also=("fixture-model",))
            terminal.press("\r")
            reason = "Key rejected" if issue == "key" else "URL/model absent"
            terminal.expect("› Model tests", also=("Save and exit", reason, "Fix failed models"))
            open_focused_results(terminal)
            choose(terminal, 3, "Fix failed models")
            if issue == "key":
                choose(terminal, 0, "Change API key")
                terminal.expect("API key source")
                terminal.press("\r")
                terminal.expect("Paste your API key")
                terminal.press("\x01")
                paste_key(terminal, "fixture-repair-key")
            else:
                choose(terminal, 0, "Change API URL")
                terminal.expect("API URL")
                terminal.press("\x01")
                terminal.type(fixed)
                terminal.press("\r")
            terminal.expect("Save setup", also=("Save and continue", "fixture-model", "Ready"), absent=("Fix failed models",))
            assert ("/v1/chat/completions", "Bearer fixture-repair-key") in requests
            terminal.exit_cleanly("\x03")
