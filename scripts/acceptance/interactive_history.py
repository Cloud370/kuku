"""Exercise conversation switches and their optional workspace effects through the installed TUI."""

import json
import sys
from urllib.parse import urlencode

from interactive_fixture import eventually, is_composer_heading


def expect_draft(terminal, prompt, reply, replaced_reply):
    screen = terminal.expect(reply, state="Finished", also=("Draft",), absent=(replaced_reply,))["screen"]
    rows = screen.splitlines()
    heading = next(index for index, row in enumerate(rows) if is_composer_heading(row))
    assert prompt in "\n".join(rows[heading:]), screen
    return screen


def paginated_rewind(client):
    workspace = client.workspace("work-rewind-history")
    prompts = [f"HISTORY-PAGE-USER-{index:02d}" for index in range(1, 27)]
    replies = [f"HISTORY-PAGE-ANSWER-{index:02d}" for index in range(1, 27)]
    query = {"workspace": workspace, "agent": "main", "limit": 48}

    def history():
        return client.request("GET", f"/sessions/{session}/history?{urlencode(query)}").as_json()

    def oldest_message(terminal):
        terminal.press("\x07")
        terminal.expect("Conversation history", also=(f"Preview  ·  Turn {len(prompts)}", prompts[-1]))
        for turn in range(len(prompts) - 1, 0, -1):
            terminal.press("\x1b[A")
            screen = terminal.expect(f"Preview  ·  Turn {turn}", also=(prompts[turn - 1],))["screen"]
        return len(prompts) - 1, screen

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
        terminal.expect("Finished", absent=("Conversation history",))
        assert history() == before, "Browsing and cancelling rewind changed durable history"
        client.provider.assert_idle()
        confirmed_steps, selected = oldest_message(terminal)
        terminal.press("\r")
        confirmation = terminal.expect("Switch to Turn 1", also=(prompts[0], "Enter Switch"))["screen"]
        terminal.press("\r")
        expect_draft(terminal, prompts[0], replies[0], replies[-1])
        rewound = history()
        assert [entry["id"] for entry in rewound["entries"] if entry["type"] == "user"] == [first_id]
        terminal.press("\x03")
        terminal.submit("Continue after the earliest paginated rewind")
        call = client.provider.next("Continue after the earliest paginated rewind")
        context = json.dumps(call.request["messages"])
        assert prompts[0] in context and replies[0] in context
        assert all(text not in context for text in prompts[1:] + replies[1:])
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
        "confirmation_screen": confirmation,
        "discarded_content_in_next_request": any(text in context for text in prompts[1:] + replies[1:]),
    }, ensure_ascii=False, indent=2), encoding="utf-8")
    print("Rewind loaded beyond 48 committed messages, cancelled safely and confirmed the exact earliest event.")


def rewind_failed_input(client):
    for name, key in (("enter", "\r"), ("alt-enter", "\x1b[13;3u")):
        workspace = client.workspace(f"work-rewind-failed-{name}")
        first = "Retain the successful turn before the failure"
        prior_reply = "This earlier answer stays in the conversation."
        failed = "Resend this input after rewinding the interrupted turn"
        with client.terminal(workspace) as terminal:
            terminal.expect("New session", state="Ready")
            terminal.submit(first)
            client.provider.next(first).text(prior_reply)
            terminal.expect(prior_reply, state="Finished")
            session = client.session(workspace)

            terminal.submit(failed)
            client.provider.next(failed).fail()
            retry_banner = "Paused · F2 Continue"
            terminal.expect(retry_banner, state="Model request failed",
                            also=("Reply interrupted", "HTTP 400", "Enter queue only"))
            terminal.press("\x07")
            terminal.expect("Conversation history", also=("Preview  ·  Turn 2", failed))
            terminal.press("\r")
            terminal.expect("Switch to Turn 2", also=(failed, "Enter Switch"))
            terminal.press("\r")

            def rewind_is_idle():
                agent = client.snapshot(workspace, session)
                return agent["status"] == "idle" and agent["pending_inputs"] == 0

            eventually("rewind to leave the Agent idle with no queued input", rewind_is_idle)
            screen = terminal.expect(prior_reply, also=("Draft", failed),
                                     absent=(retry_banner, "Pending inputs"))["screen"]
            rows = screen.splitlines()
            heading = next(index for index, row in enumerate(rows) if is_composer_heading(row))
            assert failed in "\n".join(rows[heading:]), screen
            client.provider.assert_idle(duration=0.2)
            terminal.press(key)
            call = client.provider.next(failed)
            context = json.dumps(call.request["messages"])
            assert prior_reply in context and context.count(failed) == 1
            call.text("The rewound input was sent once.")
            terminal.expect("The rewound input was sent once.", state="Finished")
            terminal.exit_cleanly()
        client.provider.assert_idle()
    print("After a provider HTTP 400, rewind restored the draft; Enter and Alt+Enter sent it without Continue.")


def rewind_files(client):
    workspace = client.workspace("work-rewind-files")
    notes = workspace / "notes.txt"
    created = workspace / "created.txt"
    script_only = workspace / "script-only.txt"
    command_only = workspace / "command-only.txt"
    first, second = "Set up the original workspace notes", "Change the workspace for the next report"

    def switch_to(terminal, turn, *, steps=()):
        terminal.press("\x07")
        terminal.expect("Conversation history", also=("Preview  ·  Turn 2",))
        for step in steps:
            terminal.press(step)
        terminal.expect(f"Preview  ·  Turn {turn}")
        terminal.press("\r")
        return terminal.expect(f"Switch to Turn {turn}", also=("Enter Switch",))["screen"]

    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.submit(first)
        client.provider.next(first).tool("write_file", "base-notes", path=notes.name, content="original\n")
        client.provider.next("base-notes").text("Original workspace note recorded.")
        terminal.expect("Original workspace note recorded.", state="Finished")

        terminal.submit(second)
        client.provider.next(second).tool(
            "edit_file", "change-notes", path=notes.name, old_text="original", new_text="updated\nextra",
        )
        client.provider.next("change-notes").tool(
            "write_file", "create-file", path=created.name, content="one\ntwo\n",
        )
        client.provider.next("create-file").tool(
            "run_js", "script-file", code='return await tools.write_file({path:"script-only.txt",content:"script\\n"});',
        )
        client.provider.next("script-file").tool(
            "run_command", "command-file", program=sys.executable, background=False,
            args=["-c", "from pathlib import Path; Path('command-only.txt').write_bytes(b'command\\n')"],
        )
        client.provider.next("command-file").text("The next report is ready.")
        terminal.expect("The next report is ready.", state="Finished")
        assert notes.read_bytes() == b"updated\nextra\n"
        assert created.read_bytes() == b"one\ntwo\n"
        assert script_only.read_bytes() == b"script\n"
        assert command_only.read_bytes() == b"command\n"

        switch_to(terminal, 2)
        terminal.press(" ")
        empty_preview = terminal.expect("No files to restore.", also=(
            "[ ] Restore files", "Enter Switch",
        ))["screen"]
        terminal.press("\r")
        expect_draft(terminal, second, "The next report is ready.", "Switch to Turn 2")
        assert notes.read_bytes() == b"updated\nextra\n" and created.is_file()
        terminal.press("\x03")

        default_screen = switch_to(terminal, 1, steps=("\x1b[A",))
        assert "[ ] Restore files" in default_screen
        terminal.press("\r")
        expect_draft(terminal, first, "Original workspace note recorded.", "The next report is ready.")
        assert notes.read_bytes() == b"updated\nextra\n" and created.is_file()
        saved = switch_to(terminal, 2)
        assert "Keep current files" in saved and "Your draft stays" in saved
        terminal.press("\r")
        expect_draft(terminal, first, "The next report is ready.", "Switch to Turn 2")

        switch_to(terminal, 1, steps=("\x1b[A",))
        terminal.press(" ")
        preview = terminal.expect("[x] Restore files", also=(
            "notes.txt", "Restore   +1 −2", "created.txt", "Remove   +0 −2",
            "Script and shell changes stay as they are.", "Enter Switch",
        ))["screen"]
        terminal.press("\r")
        expect_draft(terminal, first, "Original workspace note recorded.", "The next report is ready.")
        assert notes.read_bytes() == b"original\n" and not created.exists()
        assert script_only.read_bytes() == b"script\n"
        assert command_only.read_bytes() == b"command\n"
        terminal.press("\x03")
        terminal.exit_cleanly()
    client.provider.assert_idle()
    (client.directory / "rewind-files.json").write_text(json.dumps({
        "default": default_screen, "empty_preview": empty_preview,
        "saved_turn": saved, "approved_preview": preview,
        "restored_file": notes.name, "removed_file": created.name,
        "kept_script_file": script_only.name, "kept_command_file": command_only.name,
    }, ensure_ascii=False, indent=2), encoding="utf-8")
    print("Conversation switching kept files by default and applied only the approved native file preview.")
