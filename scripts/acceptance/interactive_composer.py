"""Check composer actions with the installed client and its native terminal."""

from urllib.parse import urlencode

from interactive_fixture import eventually


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

    def thinking(terminal):
        terminal.press("\x10/thinking\r")
        return terminal.expect("D Save default", also=("Use default", "Low", "Medium", "High", draft))["screen"]

    def position(screen, text):
        return next((row, line.index(text)) for row, line in enumerate(screen.splitlines()) if text in line)

    with client.terminal(workspace) as terminal:
        terminal.request("resize", rows=28, cols=60)
        terminal.expect(f"{model} · None")
        terminal.press("/models")
        suggestions = terminal.expect("Actions and Skills", also=("/model ·",))["screen"]
        assert suggestions.count("/model ·") == 1, suggestions
        terminal.press("\t")
        terminal.expect("/model", absent=("Actions and Skills", "/models"))
        terminal.press("\r")
        terminal.expect("Model route")
        terminal.press("\x1b")
        terminal.expect(f"{model} · None", absent=("Model route",))
        terminal.press("\x1b[200~/models\x1b[201~\r")
        terminal.expect("/models → /model", also=("Model route",))
        terminal.press("\x1b")
        terminal.expect(f"{model} · None", absent=("Model route",))

        terminal.type(draft)
        terminal.press("\x1b[D\x14")
        terminal.expect(f"{model} · Low", also=(draft,))
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
        terminal.expect("Next model request", also=(f"{model} · Low", "· Medium", draft))
        thinking(terminal)
        terminal.press("\x1b[B\x1b[B\x1b[B\x1b[B\r")
        eventually("High selected at the next request boundary", lambda: selected("high"))
        terminal.expect("Next model request", also=("· High", draft))

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
            return next(tier for tier in catalog["tiers"] if tier["name"] == "balanced")["settings"]["reasoning_effort"]

        eventually("Low persisted as the model default", lambda: saved_default() == "low")
        assert selected("high"), "Saving the default must retain the explicit Agent override"
        thinking(terminal)
        terminal.expect("Use default · Model default · Low", also=("High ✓", draft))
        terminal.press("\x1b")
        terminal.expect(draft, absent=("Use default",))

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
