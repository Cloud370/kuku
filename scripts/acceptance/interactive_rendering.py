"""Exercise reasoning previews and mixed-width scrolling in the installed terminal."""

import json
import os
import subprocess

from interactive_fixture import InstalledClient, Response, StreamPause, eventually


def event(name, **fields):
    body = json.dumps({"type": name, **fields})
    return f"event: {name}\ndata: {body}\n\n".encode()


def rendering_cases(binary, bridge, directory):
    with InstalledClient(binary, bridge, directory / "delegation") as client:
        targeted_delegation(client)
        file_change_cards(client)
        if os.name == "nt":
            windows_diagnostic(client)
    with InstalledClient(binary, bridge, directory / "reasoning") as client:
        config = client.home / "config.toml"
        config.write_text(config.read_text().replace('profile = "compatible"', 'profile = "anthropic"')
                          .replace('think = "off"', 'think = "low"')
                          .replace('reasoning_efforts = ["none"]', 'reasoning_efforts = ["none", "low"]'))
        published_reasoning(client)
        scroll_mixed_table(client)


def targeted_delegation(client):
    workspace = client.workspace("work-reply-only")
    prompt = "Delegate a reply-only Markdown sample"
    task = "Generate a Markdown sample in your reply."
    result = "# Proof of delivery"
    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.submit(prompt)
        create = client.provider.next(prompt)
        schema = next(tool["input_schema"] for tool in create.request["tools"]
                      if tool["name"] == "create_agent")
        assert set(schema["properties"]) == {"address", "instructions", "tools", "tier", "preset", "workspace"}
        create.tool("create_agent", "create-writer", address="writer", tools=[])
        client.provider.next("create-writer").tool("send_message", "dispatch", to="writer", message=task)
        first, second = client.provider.next(), client.provider.next()
        parent, child = (first, second) if "dispatch" in json.dumps(first.request["messages"]) else (second, first)
        child.expect(task)
        assert not child.request.get("tools"), child.request.get("tools")
        assert "<kuku_delegation>" not in json.dumps(child.request["messages"])
        assert "<kuku_memory_guidance>" not in json.dumps(child.request["messages"])
        parent.tool("list_agents", "collect", wait_for="writer")
        terminal.expect("╭─ Agent writer", state="Timeline")
        client.provider.assert_idle(0.35)
        child.text(result)
        collected = client.provider.next("collect")
        tool = next(block for m in collected.request["messages"] for block in m["content"]
                    if isinstance(block, dict) and block.get("tool_use_id") == "collect")
        content = tool["content"]
        reply, = json.loads(content if isinstance(content, str) else content[0]["text"])
        assert reply == {"address": "writer", "status": "idle", "last_output": result}, reply
        collected.text("Received the delegated document.\n\n```python\nprint('Proof of delivery')\n```")
        terminal.expect("Received the delegated document.", state="3 tools")
        for columns in (44, 100, 174):
            terminal.request("resize", cols=columns, rows=34)
            def aligned():
                screen = terminal.request("screen")["screen"]
                rows = [next((r for r in screen.splitlines() if title in r and "╮" in r), "")
                        for title in ("╭─ Agent writer", "╭─ python")]
                if not all(rows):
                    return False
                agent, code = rows
                gutter = 1 if columns < 72 else 2 if columns < 110 else 3
                return all(row.index("╭") == gutter and row.index("╮") == columns - gutter - 1 for row in (agent, code))
            try:
                eventually(f"Agent and code borders align at {columns} columns", aligned)
            except AssertionError as error:
                raise AssertionError(terminal.request("screen")["screen"]) from error
        terminal.exit_cleanly()
    client.provider.assert_idle()


def file_change_cards(client):
    workspace = client.workspace("work-file-cards")
    prompt = "Write a small example and update it"
    with client.terminal(workspace) as terminal:
        terminal.request("resize", rows=36, cols=105)
        terminal.expect("Ready")
        terminal.submit(prompt)
        client.provider.next(prompt).tool("write_file", "greeting", path="hello.py", content='print("Hello")\n')
        client.provider.next("greeting").tool("read_file", "verify", path="hello.py")
        client.provider.next("verify").tool("edit_file", "update", path="hello.py", old_text='"Hello"', new_text='"Welcome"')
        client.provider.next("update").tool("write_file", "summary", path="notes.md", content="# Report\n\n- Checked the greeting.\n")
        client.provider.next("summary").text("Saved the example.")
        screen = terminal.expect("Saved the example.", also=("╭─ Write file", "╭─ Edit file", "1 line written", "+1 −1", "3 lines written"))["screen"]
        assert "Checked the greeting." in screen and "# Report" not in screen, screen
        settled = terminal.expect("Saved the example.", state="Finished")["screen"]
        assert "Input accepted" not in settled and "Finished 0s" not in settled, settled
        assert '+print("Welcome")' in screen and '-print("Hello")' in screen, screen
        assert (workspace / "hello.py").read_text() == 'print("Welcome")\n'
        terminal.exit_cleanly()
    client.provider.assert_idle()


def windows_diagnostic(client):
    import ctypes

    workspace = client.workspace("work-command-diagnostic")
    command = "dir /b kuku-absent-38c207b6.txt"
    reference = subprocess.run(["cmd.exe", "/d", "/c", command], cwd=workspace, capture_output=True)
    assert reference.returncode == 1 and reference.stderr and not reference.stdout
    try:
        diagnostic = reference.stderr.decode("utf-8").strip()
    except UnicodeDecodeError:
        diagnostic = reference.stderr.decode(f"cp{ctypes.windll.kernel32.GetOEMCP()}").strip()
    with client.terminal(workspace) as terminal:
        terminal.expect("Ready")
        prompt = "Check for a missing example."
        terminal.submit(prompt)
        request = client.provider.next(prompt)
        assert "Windows cmd.exe /S /C" in json.dumps(request.request)
        request.tool("run_command", "missing-example", command=command, timeout=5)
        result = client.provider.next("missing-example")
        assert diagnostic in json.dumps(result.request, ensure_ascii=False)
        result.text("Check complete.")
        screen = terminal.expect(diagnostic, also=("Check complete.", "exit 1"))["screen"]
        assert "␍" not in screen and "�" not in screen and "command exited" not in screen and "Saved output" not in screen
        terminal.exit_cleanly()


def published_reasoning(client):
    workspace = client.workspace("work-reasoning")
    prompt = "Review the public summary"
    summary = "\n".join(["Checking the totals.", "Comparing the published results.", *(f"Note {n}" for n in range(3, 21))])
    answer = "The totals agree."
    hidden = "opaque-fixture-signature"
    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.submit(prompt)
        call = client.provider.next(prompt)
        assert call.request["thinking"]["display"] == "summarized"
        start = event("message_start", message={"id": call.id, "model": "claude-sonnet-4-6", "content": [],
                                               "usage": {"input_tokens": 10, "output_tokens": 0}})
        start += event("content_block_start", index=0, content_block={"type": "redacted_thinking", "data": hidden})
        start += event("content_block_stop", index=0)
        public = event("content_block_start", index=1, content_block={"type": "thinking", "thinking": ""})
        public += event("content_block_delta", index=1, delta={"type": "thinking_delta", "thinking": summary})
        public += event("content_block_delta", index=1, delta={"type": "signature_delta", "signature": hidden})
        public += event("content_block_stop", index=1)
        final = event("content_block_start", index=2, content_block={"type": "text", "text": answer})
        final += event("content_block_stop", index=2)
        final += event("message_delta", delta={"stop_reason": "end_turn"}, usage={"output_tokens": 12})
        final += event("message_stop")
        waiting, published = StreamPause(), StreamPause()
        call.response.put_nowait(Response(200, start + public + final, "text/event-stream",
                                          ((len(start), waiting), (len(start + public), published))))
        try:
            assert waiting.arrived.wait(8)
            screen = terminal.expect("To main · Thinking")["screen"]
            assert "View thinking" not in screen and hidden not in screen
            waiting.release.set()
            assert published.arrived.wait(8)
            compact = terminal.expect("View thinking", state="Check")["screen"]
            flow_row = next(i for i, row in enumerate(compact.splitlines()) if "Thinking" in row and "Check" in row)
            header_row = flow_row - 1
            assert "Assistant" in compact.splitlines()[header_row] and "View thinking" in compact.splitlines()[header_row]
            assert not any("Check" in row for row in compact.splitlines() if "To main" in row)
            terminal.click_text(compact, "View thinking")
            reader = terminal.expect("Comparing the published results.", state="Esc back")["screen"]
            assert hidden not in reader and "Esc back" in reader.splitlines()[header_row] and "Type a prompt" in reader
            terminal.press("\x1b")
            terminal.expect("View thinking", state="To main · Thinking")
            with client.terminal(workspace) as attached:
                attached.resume(prompt)
                restored = attached.expect("View thinking", state="To main · Thinking")["screen"]
                assert hidden not in restored
                attached.exit_cleanly()
        finally:
            waiting.release.set()
            published.release.set()
        completed = terminal.expect(answer, state="Finished", also=("Turn 1", "View thinking"))["screen"]
        assert completed.count("View thinking") == 1 and hidden not in completed
        assert answer in completed.splitlines()[flow_row], completed
        terminal.press("/source\r")
        source = terminal.expect(answer, state="Assistant · source")["screen"]
        assert "Checking the totals." not in source and hidden not in source
        terminal.exit_cleanly()
    with client.terminal(workspace) as reopened:
        reopened.resume(prompt)
        settled = reopened.expect(answer, also=("Assistant   View thinking",))["screen"]
        reopened.press("\x0f")
        quiet = reopened.expect(answer, state="View thinking")["screen"]
        assert "Comparing the published results." not in quiet
        reopened.press("/notes\r")
        notes = reopened.expect("Comparing the published results.", state="Esc back", also=(answer,))["screen"]
        assert hidden not in settled and hidden not in notes
        answer_row = next(i for i, row in enumerate(notes.splitlines()) if answer in row)
        note_row = next(i for i, row in enumerate(notes.splitlines(), 1) if "Checking the totals." in row)
        reopened.press(f"\x1b[<65;5;{note_row}M")
        scrolled = reopened.expect("Note 6", state="Esc back")["screen"]
        assert "Checking the totals." not in scrolled and answer in scrolled.splitlines()[answer_row]
        reopened.press("\x1b[B")
        down = reopened.expect("Note 7", state="Esc back")["screen"]
        assert answer in down.splitlines()[answer_row]
        reopened.press("\x1b")
        restored = reopened.expect(answer, state="Assistant   View thinking")["screen"]
        assert "Esc back" not in restored and "Note 7" not in restored
        assert next(i for i, row in enumerate(restored.splitlines()) if answer in row) == next(i for i, row in enumerate(settled.splitlines()) if answer in row)

        reopened.exit_cleanly()


def scroll_mixed_table(client):
    workspace = client.workspace("work-table-scroll")
    prompt = "Show the disk report"
    intro = [f"Before table {n:02}" for n in range(16)]
    tail = [f"After table {n:02}" for n in range(16)]
    table = "| 盘符 | 卷标 | 使用率 |\n| --- | --- | --- |\n| C: | 系统 | ⚠️ 99.5% |\n| D: | 游戏 | 34% |"
    with client.terminal(workspace) as terminal:
        terminal.request("resize", rows=22, cols=42)
        terminal.expect("New session", state="Ready")
        terminal.submit(prompt)
        client.provider.next(prompt).text("\n\n".join([*intro, table, *tail]))
        finished = terminal.expect("Turn 1", state="Finished")["screen"].splitlines()
        editor_row = next(i for i, row in enumerate(finished) if "Type a prompt" in row)
        turn_row = next(i for i, row in enumerate(finished) if "Turn 1" in row and "in" in row)
        assert finished[turn_row].index("Turn 1") == finished[editor_row].index("Type a prompt"), finished
        terminal.type("preserve draft")
        def rows():
            return [line.rstrip() for line in terminal.request("screen")["screen"].splitlines()]
        saw_border = False
        for _ in range(22):
            old = rows()
            terminal.press("\x1b[<64;10;10M")
            frame = eventually("the table viewport to scroll", lambda: (r if (r := rows()) != old else None))
            saw_border |= any("┌" in line for line in frame)
            assert all("│" not in line for line in frame if "Before table" in line or "After table" in line), frame
            assert not any(line.strip() in {"│", "┃"} for line in frame), frame
            if any("Before table 01" in line for line in frame):
                break
        assert "preserve draft" in frame[editor_row], frame
        floating = next(row for row in frame if "Turn 1" in row and any(hint in row for hint in ("History", "Alt+↑", "New", "Ctrl+End")))
        assert floating.index("Turn 1") == frame[editor_row].index("preserve draft"), frame
        assert saw_border and any("preserve draft" in line for line in frame)
        terminal.press("\x1b[1;5F")
        terminal.expect("After table 15", state="preserve draft")
        terminal.exit_cleanly()
