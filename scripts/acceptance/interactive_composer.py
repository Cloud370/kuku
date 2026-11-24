"""Check composer actions with the installed client and its native terminal."""

import json
import os
from urllib.parse import urlencode

from interactive_fixture import eventually


def startup_info(client):
    workspace = client.workspace("work-startup-info")
    resources = workspace / ".claude"
    config = client.home / "config.toml"
    text = config.read_text(encoding="utf-8")
    path = json.dumps(str(resources), ensure_ascii=False)
    config.write_text(text.replace("[discovery]\n", f"[discovery]\nextra_project_paths = [{path}]\n"),
                      encoding="utf-8")

    def add_skill(name):
        folder = resources / "skills" / name
        folder.mkdir(parents=True)
        (folder / "SKILL.md").write_text(
            f"---\nname: {name}\ndescription: A startup discovery fixture.\n---\n\nUse this when requested.\n",
            encoding="utf-8",
        )

    add_skill("preview-review")
    with client.terminal(workspace) as terminal:
        terminal.request("resize", rows=28, cols=60)
        terminal.expect("preview-review", also=("Workspace", "work-startup-info", "Skills",
                                                  "New session", "claude-sonnet-4-6",
                                                  "/skill: Browse", "Ctrl+L Model", "Ctrl+T Think", "Ctrl+P Help"))
        terminal.request("resize", rows=32, cols=132)
        draft = "check the welcome column"
        terminal.type(draft)
        screen = terminal.expect(draft, also=("Workspace", "preview-review"), rows=32)["screen"]
        workspace_row = next(line for line in screen.splitlines() if line.strip().startswith("Workspace "))
        draft_row = next(line for line in screen.splitlines() if draft in line)
        assert workspace_row.index("Workspace") == draft_row.index(draft), screen
        terminal.press("\x03")
        terminal.expect("New session", absent=(draft,))
        add_skill("refresh-on-new")
        terminal.press("/new\r")
        terminal.expect("refresh-on-new", also=("preview-review", "New session"))
        terminal.press("/skill:")
        terminal.expect("/skill:refresh-on-new", state="Actions and Skills")
        terminal.press("\x1b")
        terminal.expect("New session", absent=("Actions and Skills",))
        sessions = client.request("GET", f"/sessions?{urlencode({'workspace': workspace})}").as_json()
        assert sessions["sessions"] == []
        terminal.exit_cleanly()


def choose_agent_entry(terminal, screen, label):
    if os.name != "nt":
        terminal.click_text(screen, label)
        return

    def selected(view):
        return next(((row, line) for row, line in enumerate(view.splitlines())
                     if line.lstrip(" │").startswith("›")), None)

    for _ in range(12):
        selection = selected(screen)
        assert selection is not None, screen
        row, line = selection
        if label in line:
            terminal.press("\r")
            return
        target = next(i for i, line in enumerate(screen.splitlines()) if label in line)
        terminal.press("\x1b[B" if target > row else "\x1b[A")

        def moved():
            current = terminal.request("screen")["screen"]
            current_selection = selected(current)
            return current if current_selection is not None and current_selection != selection else None

        screen = eventually(f"move the Agent picker selection toward {label}", moved)
    raise AssertionError(f"Could not select {label} in the Agent picker")


def agent_setup(client):
    workspace = client.workspace("work-agent-setup")
    config = client.home / "config.toml"
    text = config.read_text(encoding="utf-8")
    text = text.replace('reasoning_efforts = ["none"]',
                        'reasoning_efforts = ["none", "high"]\nservice_tiers = ["flex", "priority"]')
    text += '\n[model.test-anthropic."review-model"]\nthink = "off"\nmax_output_tokens = 4096\n'
    config.write_text(text, encoding="utf-8")

    with client.terminal(workspace) as terminal:
        terminal.request("resize", cols=60, rows=28)
        terminal.expect("New session", state="Ready", rows=28)
        terminal.submit("Start the Agent setup check")
        client.provider.next("Start the Agent setup check").text("Ready to create a reviewer.")
        terminal.expect("Ready to create a reviewer.")
        terminal.press("/agents\r")
        roster = terminal.expect("Create Agent", state="Switch Agent")["screen"]
        choose_agent_entry(terminal, roster, "Create Agent")
        terminal.expect("New agent")
        terminal.type("reviewer")
        terminal.press("\r")
        review = terminal.expect("› Create Agent", also=("Agent reviewer", "Thinking · None", "Service tier · Default"))["screen"]
        choose_agent_entry(terminal, review, "Model · claude-sonnet-4-6")
        models = terminal.expect("review-model", state="Agent model", also=("test-anthropic",))["screen"]
        choose_agent_entry(terminal, models, "review-model")
        review = terminal.expect("Model · review-model", also=("Thinking · None", "Esc Back"))["screen"]
        choose_agent_entry(terminal, review, "Thinking · None")
        thinking = terminal.expect("High", state="Agent thinking")["screen"]
        choose_agent_entry(terminal, thinking, "High")
        review = terminal.expect("Thinking · High", also=("Service tier · Default",))["screen"]
        choose_agent_entry(terminal, review, "Service tier · Default")
        service = terminal.expect("Fast", state="Agent service tier", also=("Flex",))["screen"]
        choose_agent_entry(terminal, service, "Fast")
        terminal.expect("Service tier · Fast", also=("Model · review-model", "Thinking · High", "› Create Agent"))
        terminal.press("\r")
        terminal.expect("Agent reviewer", also=("review-model high priority",), absent=("› Create Agent",))
        session = client.session(workspace)
        original_main_tier = client.snapshot(workspace, session)["config"].get("service_tier")
        terminal.submit("Review at the initial tier")
        call = client.provider.next("Review at the initial tier")
        assert call.request["service_tier"] == "priority", call.request.get("service_tier")
        call.text("Initial reviewer tier verified.")
        terminal.expect("Initial reviewer tier verified.", state="Finished")

        terminal.press("/speed\r")
        terminal.expect("reviewer · review-model", also=("Follow model default", "flex"))
        terminal.press("fl")
        terminal.expect("›flex", also=("Tab Complete",))
        terminal.press("\t")
        terminal.expect("/ flex", also=("›flex",))
        terminal.press("\r")
        terminal.expect("review-model high flex", absent=("Cache may miss",))
        assert client.snapshot(workspace, session)["config"].get("service_tier") == original_main_tier

        terminal.submit("Review this request")
        terminal.expect("Service tier changed · Cache may miss", also=("priority → flex", "Enter Send"))
        client.provider.assert_idle(duration=0.2)
        terminal.press("\r")
        call = client.provider.next("Review this request")
        assert call.request["model"] == "review-model", call.request["model"]
        assert call.request["output_config"]["effort"] == "high", call.request.get("output_config")
        assert call.request["service_tier"] == "flex", call.request.get("service_tier")
        call.text("Reviewer settings verified.")
        terminal.expect("Reviewer settings verified.", state="Finished")
        terminal.submit("Review again at the same tier")
        client.provider.next("Review again at the same tier").text("Same tier verified.")
        terminal.expect("Same tier verified.")
        terminal.exit_cleanly()
    client.provider.assert_idle()


def composer_actions(client):
    workspace = client.workspace("work-composer")
    config = client.home / "config.toml"
    old = config.read_text(encoding="utf-8")
    config.write_text(old.replace('reasoning_efforts = ["none"]',
                                  'reasoning_efforts = ["none", "low", "medium", "high"]'),
                      encoding="utf-8")
    model = "claude-sonnet-4-6"
    draft = "keep this composer draft"
    input_text = "Hold this request during composer checks"
    output = "Accepted work finished after detach"
    screens = []

    def action(terminal, command):
        terminal.press("\x10")
        terminal.expect("Help and shortcuts")
        terminal.submit(command)

    def thinking(terminal):
        action(terminal, "/thinking")
        return terminal.expect("D Save default", also=("Use default", "Low", "Medium", "High", "Esc Close", draft))["screen"]

    def position(screen, text):
        return next((row, line.index(text)) for row, line in enumerate(screen.splitlines()) if text in line)

    with client.terminal(workspace) as terminal:
        terminal.request("resize", rows=28, cols=60)
        terminal.expect(f"{model} none")
        terminal.press("/")
        terminal.expect("›/new", also=("Actions and Skills",))
        terminal.press("\x1b[A")
        terminal.expect("›/quit")
        terminal.press("\x1b[B")
        terminal.expect("›/skill:kuku-docs")
        terminal.press("\x1b[B")
        terminal.expect("›/new")
        terminal.press("\x7f")
        terminal.expect("New session", absent=("Actions and Skills",))
        terminal.press("/models")
        suggestions = terminal.expect("Actions and Skills", also=("›/model ",))["screen"]
        assert suggestions.count("›/model ") == 1, suggestions
        screens.append(("Commands at 60 columns", suggestions))
        terminal.press("\t")
        terminal.expect("/model", absent=("Actions and Skills", "/models"))
        terminal.press("\r")
        models = terminal.expect("Model route", also=("Search models…", "New Sessions default"))["screen"]
        assert position(models, "Model route")[0] == position(models, "Search models…")[0]
        assert position(models, "Model route")[0] < position(models, "Ready")[0]
        screens.append(("Models at 60 columns", models))
        terminal.press("\x1b")
        terminal.expect(f"{model} none", absent=("Model route",))
        terminal.press("\x1b[200~/models\x1b[201~\r")
        terminal.expect("/models → /model", also=("Model route",))
        terminal.press("\x1b")
        terminal.expect(f"{model} none", absent=("Model route",))

        terminal.type(draft)
        previous = terminal.expect(f"{model} none", also=("Ctrl+T think", draft))["screen"]
        terminal.press("\x1b[D\x14")
        leveled = terminal.expect(f"{model} low", also=("Ctrl+T think", draft))["screen"]
        assert position(previous, "Agent main") == position(leveled, "Agent main"), leveled
        assert position(previous, draft) == position(leveled, draft), leveled
        for width in (60, 120):
            terminal.request("resize", rows=28, cols=width)
            screen = thinking(terminal)
            assert "Use default · Model default · None" in screen and "Low ✓" in screen, screen
            assert position(screen, "Use default")[1] == position(screen, draft)[1], screen
            assert position(screen, "Esc Close")[0] < position(screen, draft)[0], screen
            terminal.press("\x1b[B\x1b[B\x1b[B")
            screen = terminal.expect("›Medium", also=("Low ✓", draft))["screen"]
            screens.append((f"Thinking at {width} columns", screen))
            terminal.press("\x1b")
            terminal.expect(draft, absent=("Use default",))

        terminal.press("X")
        terminal.expect(draft[:-1] + "X" + draft[-1])
        terminal.press("\x03")
        terminal.submit(input_text)
        call = client.provider.next(input_text)
        session = client.session(workspace)
        query = urlencode({"workspace": workspace, "agent": "main"})

        def snapshot():
            return client.request("GET", f"/sessions/{session}/snapshot?{query}").as_json()

        def selected(level):
            view = snapshot()
            active = view["selection"]["settings"]["reasoning_effort"]
            pending = (view.get("pending_selection") or {}).get("settings", {}).get("reasoning_effort")
            return view if active == "low" and pending == level else None

        terminal.type(draft)
        terminal.press("\x14")
        eventually("Medium selected at the next request boundary", lambda: selected("medium"))
        terminal.expect("Next Medium", also=(f"{model} low", draft), absent=("Next model request",))
        thinking(terminal)
        terminal.press("\x1b[B\x1b[B\x1b[B\x1b[B\r")
        eventually("High selected at the next request boundary", lambda: selected("high"))
        terminal.expect("Next High", also=(f"{model} low", draft), absent=("Next model request",))

        original_config = config.read_text(encoding="utf-8")
        thinking(terminal)
        terminal.press("\x1b[B\x1b[Bd")
        confirmation = terminal.expect("Save Low as model default?", also=(model, draft))["screen"]
        screens.append(("Model default confirmation", confirmation))
        terminal.press("\x1b")
        terminal.expect("Use default", also=("High ✓",), absent=("Save Low as model default?",))
        assert config.read_text(encoding="utf-8") == original_config
        terminal.press("d\r")

        def saved_default():
            catalog = client.request("GET", "/models").as_json()
            return next(item for item in catalog["models"] if item["model"] == model)["settings"]["reasoning_effort"]

        eventually("Low persisted as the model default", lambda: saved_default() == "low")
        assert selected("high"), "Saving the default must retain the explicit Agent override"
        thinking(terminal)
        terminal.expect("Use default · Model default · Low", also=("High ✓", draft))
        terminal.press("\x1b")
        terminal.expect(draft, absent=("Use default",))

        action(terminal, "/speed turbo")
        custom_speed = terminal.expect("Custom service tier", also=("Probe and use", "Skip probe and use", draft))["screen"]
        assert position(custom_speed, "Skip probe and use")[0] < position(custom_speed, draft)[0], custom_speed
        terminal.press("\x1b[B\r")
        terminal.expect("Next service tier · Turbo", also=(draft,))
        eventually("turbo applies on the next turn",
                   lambda: (snapshot().get("pending_selection") or {}).get("settings", {}).get("service_tier") == "turbo")
        catalog = client.request("GET", "/models").as_json()
        capabilities = next(item for item in catalog["models"] if item["model"] == model)["capabilities"]
        assert capabilities["custom_service_tiers"] == {"turbo": "unverified"}, capabilities

        terminal.press("\x03")
        for word, submit in (("  exit  ", "\r"), (" quit ", "\x1b\r")):
            terminal.press(f"\x1b[200~{word}\x1b[201~{submit}")
            terminal.expect("Exit kuku? Y exit · Esc edit")
            terminal.press("\r\r")
            confirmation = terminal.expect("Exit kuku? Y exit · Esc edit")["screen"]
            assert position(confirmation, word.rstrip())[0] > position(confirmation, "Exit kuku?")[0]
            assert snapshot()["pending"] == [] and snapshot()["agent"]["status"] == "running"
            terminal.press("\x1b")
            terminal.expect(word.rstrip(), absent=("Exit kuku?",))
            assert snapshot()["pending"] == [] and snapshot()["agent"]["status"] == "running"
            terminal.press("\x03")

        terminal.press("exit\r")
        screens.append(("Exit confirmation", terminal.expect("Exit kuku? Y exit · Esc edit")["screen"]))
        terminal.exit_cleanly(keys="Y")

    call.text(output)
    eventually("the original reply to settle after detach",
               lambda: client.snapshot(workspace, session)["last_output"] == output)
    client.provider.assert_idle()
    for caption, screen in screens:
        print(f"{caption}\n{screen.rstrip()}")
