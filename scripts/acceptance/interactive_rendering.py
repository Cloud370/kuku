"""Exercise reasoning previews and mixed-width scrolling in the installed terminal."""

import json
import os
import shutil
import subprocess
import sys
from unittest.mock import patch
from urllib.parse import urlencode

from interactive_fixture import InstalledClient, Response, StreamPause, eventually


def event(name, **fields):
    body = json.dumps({"type": name, **fields})
    return f"event: {name}\ndata: {body}\n\n".encode()


def rendering_cases(binary, bridge, directory):
    with InstalledClient(binary, bridge, directory / "delegation") as client:
        targeted_delegation(client)
        file_change_cards(client)
        markdown_inline_elements(client)
        if os.name == "nt":
            windows_diagnostic(client)
            for shell in ("powershell.exe", "pwsh.exe"):
                if shutil.which(shell):
                    windows_command_encoding(client, shell)
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
            action = "Open →" if columns == 44 else "Open agent →"
            scene = terminal.expect(action, state="╭─ Agent writer")["screen"]
            terminal.click_text(scene, action)
            terminal.expect("To writer", state="Proof of delivery")
            terminal.press("/back\r")
            terminal.expect("To main", state="Received the delegated document.")
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


def markdown_inline_elements(client):
    workspace = client.workspace("work-markdown-links")
    first = "https://example.test/first"
    second = "https://example.test/report?item=2&format=md#section"
    source = (
        "x<sup>2</sup> + y<sup>2</sup> = z<sup>2</sup> · H<sub>2</sub>O<br>下一行\n\n"
        "> - [x] 中文勾选项\n> - [ ] 保留待办项\n\n"
        f"A [Manual]({first}) · B [Manual]({second})\n\n"
        "原样显示 `<sup>42</sup>`"
    )
    open_log = workspace / "opened.txt"
    env = {}
    if sys.platform.startswith("linux"):
        shim = workspace / "openers"
        shim.mkdir()
        for command in ("xdg-open", "powershell.exe"):
            opener = shim / command
            opener.write_text(
                "#!/usr/bin/env python3\nimport os, sys\n"
                "with open(os.environ['KUKU_LINK_PROBE'], 'a') as log:\n"
                "    print(os.environ.get('OPEN_RS_TARGET', sys.argv[-1]), file=log)\n"
            )
            opener.chmod(0o755)
        env = {"PATH": str(shim) + os.pathsep + client.env["PATH"], "KUKU_LINK_PROBE": str(open_log)}
    with patch.dict(client.env, env), client.terminal(workspace) as terminal:
        terminal.request("resize", cols=70, rows=32)
        terminal.expect("Ready")
        terminal.submit("Render this example")
        client.provider.next().text(source)
        screen = terminal.expect("x² + y² = z²", also=("H₂O", "下一行", "B Manual", "<sup>42</sup>"))["screen"]
        if env:
            terminal.click_text(screen, "B Manual", cell_offset=3)
            eventually("default opener receives second link's complete URL",
                       lambda: open_log.exists() and open_log.read_text().splitlines() == [second])
            terminal.click_text(screen, "A Manual", cell_offset=3)
            eventually("default opener receives first link's URL",
                       lambda: open_log.read_text().splitlines() == [second, first])
        terminal.request("resize", cols=36, rows=38)
        terminal.expect("中文勾选项", also=("保留待办项", "B Manual", "H₂O"))
        terminal.press("/source\r")
        terminal.expect("x<sup>2</sup>", also=("H<sub>2</sub>O",))
        terminal.exit_cleanly()


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
        request.tool("run_command", "missing-example", command=command, timeout=5, brief="Check for missing example")
        result = client.provider.next("missing-example")
        assert diagnostic in json.dumps(result.request, ensure_ascii=False)
        result.text("Check complete.")
        screen = terminal.expect(diagnostic, also=("Check complete.", "exit 1"))["screen"]
        assert "␍" not in screen and "�" not in screen and "command exited" not in screen and "Saved output" not in screen
        rows = screen.splitlines()
        header = next(i for i, line in enumerate(rows) if "╭─ Check for missing example" in line)
        assert "exit 1" not in rows[header] and not rows[header + 1].strip(" │"), screen
        assert command in rows[header + 2], screen
        assert diagnostic in rows[header + 3] and not rows[header + 4].strip(" │"), screen
        assert "exit 1" in rows[header + 5], screen
        terminal.click_text(screen, "Check for missing example")
        terminal.expect("Requested arguments", state=diagnostic)
        terminal.exit_cleanly()


def windows_command_encoding(client, shell):
    workspace = client.workspace(f"work-encoding-{shell[:-4]}")
    script = workspace / "output.ps1"
    script.write_text(
        "[Console]::WriteLine('stdout 正常输出')\n"
        "[Console]::Error.WriteLine(([char]27).ToString() + '[31;1mstderr 中文错误' + [char]27 + '[0m')\n"
        "$bytes = [Text.Encoding]::UTF8.GetBytes('utf8 中文' + [Environment]::NewLine)\n"
        "[Console]::OpenStandardError().Write($bytes, 0, $bytes.Length)\n",
        encoding="utf-8-sig",
    )
    command = f"{shell} -NoProfile -NonInteractive -ExecutionPolicy Bypass -File output.ps1"
    reference = subprocess.run(command, cwd=workspace, capture_output=True,
                               creationflags=subprocess.CREATE_NO_WINDOW, check=True)
    stdout = reference.stdout.decode("oem").strip()
    stderr = reference.stderr.splitlines()[0].decode("oem").removeprefix("\x1b[31;1m").removesuffix("\x1b[0m")
    with client.terminal(workspace) as terminal:
        terminal.expect("Ready")
        terminal.submit("Check shell output")
        client.provider.next().tool("run_command", "encoding", command=command, timeout=15, brief="Check shell output")
        result = client.provider.next("encoding")
        content = json.dumps(result.request["messages"], ensure_ascii=False)
        assert all(line in content for line in (stdout, stderr, "utf8 中文")), content
        assert "\ufffd" not in content and "[31;1m" not in content
        result.text("Output checked.")
        terminal.expect(stdout, also=(stderr, "Output checked."))
        session = client.session(workspace)
        query = urlencode({"workspace": workspace, "agent": "main"})
        entries = client.request("GET", f"/sessions/{session}/history?{query}").as_json()["entries"]
        output_id = next(entry["output_id"] for entry in entries if entry["type"] == "tool_result")
        query = urlencode({"workspace": workspace, "offset": 0, "max_bytes": 4096})
        page = client.request("GET", f"/sessions/{session}/outputs/{output_id}?{query}").as_json()["output"]
        assert page["complete"] and not page["lossy_utf8"]
        display = "".join(segment["text"] for segment in page["segments"])
        assert all(line in display for line in (stdout, stderr, "utf8 中文")) and "[31;1m" not in display
        for stream in ("stdout", "stderr"):
            raw = b"".join(bytes(segment["raw_bytes"]) if "raw_bytes" in segment else segment["text"].encode()
                           for segment in page["segments"] if segment["stream"] == stream)
            assert raw == getattr(reference, stream)
        utf8_line = "utf8 中文".encode() + b"\r\n"
        stderr_segment = next(segment for segment in page["segments"] if "utf8 中文" in segment["text"])
        raw_stderr = bytes(stderr_segment.get("raw_bytes", stderr_segment["text"].encode()))
        offset = stderr_segment["offset"] + raw_stderr.index(utf8_line) + len("utf8 ".encode()) + 1
        query = urlencode({"workspace": workspace, "offset": offset, "max_bytes": 2})
        split = client.request("GET", f"/sessions/{session}/outputs/{output_id}?{query}").as_json()["output"]
        assert split["lossy_utf8"] and split["next_offset"] == offset + 2
        assert bytes(split["segments"][0]["raw_bytes"]) == "中".encode()[1:]
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
