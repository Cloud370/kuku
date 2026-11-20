"""Installed-terminal checks for settings, defaults and retained rewind history."""

import json
from urllib.parse import urlencode

from interactive_fixture import InstalledClient, eventually
from interactive_rendering import has_selected_queued_input


def lifecycle_cases(binary, bridge, directory):
    first_run_credential_repair(binary, bridge, directory / "settings")
    with InstalledClient(binary, bridge, directory / "defaults") as client:
        persistent_model_default(client)
    with InstalledClient(binary, bridge, directory / "keyboard-queue") as client:
        keyboard_queue_withdrawal(client)
    with InstalledClient(binary, bridge, directory / "rewind") as client:
        readonly_rewind(client)


def first_run_credential_repair(binary, bridge, directory):
    instance = InstalledClient(binary, bridge, directory)
    key = "fixture-repaired-credential"
    authenticated = []
    original_dispatch = instance.provider.dispatch

    def dispatch(method, path, headers, body):
        assert headers.get("x-api-key") == key, "Repaired credential did not reach the provider"
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
            terminal.expect("Step 1/5", also=("Existing · test-anthropic",))
            terminal.press("\r")
            terminal.expect("Step 2/5")
            terminal.press("\r")
            missing = terminal.expect("Step 3/5", also=("not detected", "› Paste API key"))["screen"]
            client.provider.assert_idle()
            terminal.press("\r")
            terminal.expect("Paste a key; every character is masked.")
            terminal.press(f"\x1b[200~{key}\x1b[201~")
            masked = terminal.expect("•" * len(key))["screen"]
            assert key not in masked
            terminal.press("\r")
            terminal.expect("Save and continue")
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
        terminal.click_text(models, "Tier · light")
        terminal.press("\r")
        terminal.expect(chosen)
        terminal.submit(prompt)
        call = client.provider.next(prompt)
        assert call.request["model"] == chosen, call.request["model"]
        call.text("The Session model is selected.")
        terminal.expect("The Session model is selected.", state="Finished")
        session = client.session(workspace)
        before = client.snapshot(workspace, session)["config"]
        terminal.press("/model\r")
        models = terminal.expect("Set current model as default")["screen"]
        terminal.click_text(models, "Set current model as default")
        confirmation = terminal.expect("Save as default", also=(chosen, "new sessions"))["screen"]
        terminal.press("\x1b[A\r")
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
        readonly = terminal.expect(discarded_reply, also=("excluded from model context",))["screen"]
        terminal.press("\x1b")
        terminal.expect("Continue from the active anchor")
        terminal.press("\r")
        continued = client.provider.next("Continue from the active anchor")
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
