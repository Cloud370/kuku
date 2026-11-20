"""Installed-terminal checks for settings, defaults and retained rewind history."""

import json
import re
import tomllib
from urllib.parse import urlencode

from interactive_fixture import InstalledClient, eventually, json_response
from interactive_rendering import has_selected_queued_input


def lifecycle_cases(binary, bridge, directory):
    first_run_credential_repair(binary, bridge, directory / "settings")
    with InstalledClient(binary, bridge, directory / "defaults") as client:
        persistent_model_default(client)
    with InstalledClient(binary, bridge, directory / "keyboard-queue") as client:
        keyboard_queue_withdrawal(client)
    with InstalledClient(binary, bridge, directory / "rewind") as client:
        readonly_rewind(client)
    with InstalledClient(binary, bridge, directory / "rewind-history") as client:
        paginated_rewind(client)
    runtime_credential_repair(binary, bridge, directory / "auth-repair")


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
        original_config = tomllib.loads(config_path.read_text())
        with client.terminal(workspace) as terminal:
            terminal.expect("New session", state="Ready")
            terminal.submit(prompt)
            failure = terminal.expect("Repair credentials", also=("No automatic retry",))["screen"]
            session = client.session(workspace)
            original = client.connection()
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
        saved_config = tomllib.loads(saved_text)
        key_ref = saved_config["provider"]["test-anthropic"].pop("api_key")
        original_config["provider"]["test-anthropic"].pop("api_key")
        assert saved_config == original_config, "Repair changed unrelated provider or model settings"
        assert key_ref.startswith("file:")
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
