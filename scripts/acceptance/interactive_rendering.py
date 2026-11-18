"""Exercise reasoning previews and mixed-width scrolling in the installed terminal."""

import json

from interactive_fixture import InstalledClient, Response, StreamPause, eventually


def event(name, **fields):
    body = json.dumps({"type": name, **fields})
    return f"event: {name}\ndata: {body}\n\n".encode()


def rendering_cases(binary, bridge, directory):
    with InstalledClient(binary, bridge, directory / "delegation") as client:
        targeted_delegation(client)
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


def published_reasoning(client):
    workspace = client.workspace("work-reasoning")
    prompt = "Review the public summary"
    summary = "Checking the totals.\nComparing the published results."
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
            assert any("To main" in row and "Check" in row for row in compact.splitlines()), compact
            assert not any("Check" in row for row in compact.splitlines() if "To main" not in row)
            terminal.click_text(compact, "View thinking")
            reader = terminal.expect("Comparing the published results.", state="Esc Close")["screen"]
            assert hidden not in reader
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
        completed = terminal.expect(answer, state="Finished", also=("▌ Turn 1", "View thinking"))["screen"]
        assert completed.count("View thinking") == 1 and hidden not in completed
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
        reopened.press("/details\r")
        reopened.expect("Transcript details", state="View thinking")
        reopened.press("\r")
        notes = reopened.expect("Comparing the published results.", state="Esc Close")["screen"]
        assert hidden not in settled and hidden not in notes
        reopened.press("\x1b")
        reopened.expect(answer, state="Assistant   View thinking")
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
        terminal.expect("▌ Turn 1", state="Finished")
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
        assert saw_border and any("preserve draft" in line for line in frame)
        terminal.press("\x1b[1;5F")
        terminal.expect("After table 15", state="preserve draft")
        terminal.exit_cleanly()
