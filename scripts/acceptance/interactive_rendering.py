"""Exercise reasoning previews and mixed-width scrolling in the installed terminal."""

import json
import os
import re
import shutil
import subprocess
import sys
from unittest.mock import patch
from urllib.parse import urlencode

from interactive_fixture import (
    InstalledClient, Response, StreamPause, eventually, has_standalone_assistant_rail,
    is_composer_heading, run_cases,
)
from interactive_responses import responses_reasoning


def select_message_action(terminal, screen, action):
    choice = re.compile(r"(?P<selected>› | {2})(?P<label>Copy|Source|Thinking|Expand|Full output)(?= {2}|$)")

    def action_rows(value):
        lines = value.splitlines()
        start = next(
            (index for index, line in enumerate(lines) if "Message actions" in line),
            None,
        )
        if start is None:
            return []
        rows = lines[start + 1:]
        first_choice = next(
            (
                index
                for index, line in enumerate(rows)
                if (match := choice.search(line)) and match["selected"] == "› "
            ),
            None,
        )
        if first_choice is None:
            return []
        end = next(
            (index for index, line in enumerate(rows[first_choice:], first_choice)
             if "Operate message" in line),
            len(rows),
        )
        return rows[first_choice:end]

    def visible_actions(value):
        return [match["label"] for line in action_rows(value) if (match := choice.search(line))]

    def selected_action(value):
        for line in action_rows(value):
            if (match := choice.search(line)) and match["selected"] == "› ":
                return match["label"]
        return None

    visible = visible_actions(screen)
    assert action in visible, screen
    for _ in visible:
        selected = selected_action(screen)
        if selected == action:
            break
        assert selected in visible, screen
        next_action = visible[(visible.index(selected) + 1) % len(visible)]
        terminal.press("\t")
        screen = eventually(
            f"selection of message action {next_action!r}",
            lambda: (
                value
                if selected_action(value := terminal.request("screen")["screen"])
                == next_action
                else None
            ),
        )
    else:
        raise AssertionError(f"Could not select message action {action!r}:\n{screen}")

    terminal.press("\r")
    eventually(
        "message action picker to close",
        lambda: (
            value
            if "Message actions" not in (value := terminal.request("screen")["screen"])
            else None
        ),
    )


def queued_input_click_position(screen, text):
    lines = screen.splitlines()
    start = next(
        (index for index, line in enumerate(lines) if "Pending inputs" in line),
        None,
    )
    if start is None:
        return None
    end = next(
        (index for index, line in enumerate(lines[start + 1 :], start + 1)
         if is_composer_heading(line)),
        len(lines),
    )
    for index in range(start + 1, end):
        line = lines[index]
        if "Queued ·" not in line:
            continue
        next_input = next(
            (next_index for next_index in range(index + 1, len(lines))
             if "You ·" in lines[next_index]),
            end,
        )
        stop = min(next_input, end)
        if text in "\n".join(lines[index:stop]):
            heading = line.find("You ·")
            return (index + 1, heading + 1) if heading >= 0 else None
    return None


def click_queued_input(terminal, screen, text):
    position = queued_input_click_position(screen, text)
    if position is None:
        raise AssertionError(
            "Queued input is not visible in the pending-input panel; "
            "target row unavailable; no click coordinates were sent"
        )
    row, column = position
    terminal.press(f"\x1b[<0;{column};{row}M\x1b[<0;{column};{row}m")
    return row, column


def queued_input_click_context(row, column, screen_snapshot):
    return (
        f"Queued input target row: {row}\n"
        f"Mouse click coordinates (column, row; 1-based): ({column}, {row})\n"
        f"Terminal size: {screen_snapshot['rows']} rows × {screen_snapshot['cols']} columns\n"
        "Full screen used to compute click coordinates:\n"
        f"{screen_snapshot['screen']}"
    )


def has_selected_queued_input(screen, text):
    position = queued_input_click_position(screen, text)
    if position is None:
        return False
    row, _ = position
    return "› You ·" in screen.splitlines()[row - 1]


def pending_inputs_excerpt(screen):
    lines = screen.splitlines()
    start = next(
        (index for index, line in enumerate(lines) if "Pending inputs" in line),
        max(0, len(lines) - 8),
    )
    context = [line for line in lines if "Removed from queue" in line]
    context.extend(lines[start:])
    return "\n".join(context)[:2048]


def event(name, **fields):
    body = json.dumps({"type": name, **fields})
    return f"event: {name}\ndata: {body}\n\n".encode()


def operate_on_message(terminal, screen, role, marker, action):
    terminal.click_text(screen, "Messages" if "Messages" in screen else "Msg")
    picker = terminal.expect("Active messages")["screen"]
    lines = picker.splitlines()
    header = next(index for index, line in enumerate(lines) if "Active messages" in line)
    choices = lines[header + 1:]
    rows = [
        (index, line) for index, line in enumerate(choices)
        if role in line
        and (not marker or marker in line or index + 1 < len(choices) and marker in choices[index + 1])
    ]
    assert len(rows) == 1, picker
    index, _ = rows[0]
    column = terminal.cols // 2 + 1
    row_number = header + index + 2
    terminal.press(f"\x1b[<0;{column};{row_number}M\x1b[<0;{column};{row_number}m")
    actions = terminal.expect(f"Selected · {role}")["screen"]
    if marker:
        assert marker in actions, actions
    assert action in actions, actions
    select_message_action(terminal, actions, action)


def operate_on_assistant(terminal, screen, marker, action):
    operate_on_message(terminal, screen, "Assistant", marker, action)


def rendering_cases(binary, bridge, directory):
    def configured(name, operation, reasoning=False):
        with InstalledClient(binary, bridge, directory / name) as client:
            if reasoning:
                config = client.home / "config.toml"
                config.write_text(config.read_text().replace('profile = "compatible"', 'profile = "anthropic"')
                                  .replace('think = "off"', 'think = "low"')
                                  .replace('reasoning_efforts = ["none"]', 'reasoning_efforts = ["none", "low"]'))
            operation(client)

    operations = [("delegation", targeted_delegation), ("file-cards", file_change_cards),
                  ("inline", markdown_inline_elements), ("named-output", named_output_cards)]
    if os.name == "nt":
        operations.append(("windows-diagnostic", windows_diagnostic))
        operations.extend((shell, lambda client, shell=shell: windows_command_encoding(client, shell))
                          for shell in ("powershell.exe", "pwsh.exe") if shutil.which(shell))
    cases = [(name, lambda name=name, operation=operation: configured(name, operation))
             for name, operation in operations]
    cases.append(("responses-reasoning", lambda: responses_reasoning(binary, bridge, directory / "responses-reasoning")))
    cases.extend((name, lambda name=name, operation=operation: configured(name, operation, True))
                 for name, operation in (("reasoning", published_reasoning), ("mixed-table", scroll_mixed_table)))
    run_cases(cases)


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
        assert set(schema["properties"]) == {"address", "instructions", "tools", "tier", "preset", "workspace",
                                            "reasoning_effort", "service_tier"}
        create.tool("create_agent", "create-writer", address="writer", tools=[])
        client.provider.next("create-writer").tool("send_message", "dispatch", to="writer", message=task)
        first, second = client.provider.next(), client.provider.next()
        parent, child = (first, second) if "dispatch" in json.dumps(first.request["messages"]) else (second, first)
        child.expect(task)
        child_tools = sorted(tool["name"] for tool in child.request.get("tools", []))
        assert child_tools == ["list_agents", "send_message"], child_tools
        assert "<kuku_delegation>" not in json.dumps(child.request["messages"])
        assert "<kuku_memory_guidance>" not in json.dumps(child.request["messages"])
        parent.tool("list_agents", "collect", wait_for="writer")
        waiting = terminal.expect("Agent check · writer · Waiting", also=("Agent writer", "Timeline", "Thinking · None"),
                                  absent=("· Fast", "· Ultra Fast"))["screen"]
        client.provider.assert_idle(0.35)
        child.text(result)
        collected = client.provider.next("collect")
        tool = next(block for m in collected.request["messages"] for block in m["content"]
                    if isinstance(block, dict) and block.get("tool_use_id") == "collect")
        content = tool["content"]
        reply, = json.loads(content if isinstance(content, str) else content[0]["text"])
        assert reply == {"address": "writer", "status": "idle", "last_output": result,
                         "last_request": {"model": child.request["model"]}}, reply
        timeline = terminal.expect("Timeline · 3 calls", also=("Finished · Proof of delivery",))["screen"]
        terminal.click_text(timeline, "Timeline")
        checked = terminal.expect("Agent check · writer (Idle, reply available)")["screen"]
        def target_column(screen):
            row = next(row for row in screen.splitlines() if "Agent check · writer" in row)
            return row.index("writer") - row.index("Agent check")
        assert target_column(waiting) == target_column(checked), checked
        terminal.click_text(checked, "Timeline")
        collected.text("Received the delegated document.\n\n```python\nprint('Proof of delivery')\n```")
        terminal.expect("Received the delegated document.", state="3 tools")
        for columns in (44, 100, 174):
            terminal.request("resize", cols=columns, rows=34)
            def aligned_cards():
                screen = terminal.request("screen")["screen"]
                agent = next((row for row in screen.splitlines()
                              if row.lstrip().startswith("╭─ Agent writer") and "Open" in row), "")
                code = next((row for row in screen.splitlines() if "╭─ python" in row and "╮" in row), "")
                if not agent or not code:
                    return False
                gutter = 1 if columns < 72 else 2 if columns < 110 else 3
                transcript_width = columns - 2 * gutter
                return (agent.index("╭") == gutter
                        and agent.index("→") == gutter + transcript_width - 3
                        and code.index("╭") == gutter + 2
                        and code.index("╮") == gutter + transcript_width - 3)
            try:
                eventually(f"Agent card and code block align at {columns} columns", aligned_cards)
            except AssertionError as error:
                raise AssertionError(terminal.request("screen")["screen"]) from error
            action = "Open →" if columns == 44 else "Open agent →"
            scene = terminal.expect(action, state="Agent writer")["screen"]
            terminal.click_text(scene, action)
            back = terminal.expect("F2 Back", state="writer ▾", also=("Proof of delivery",))["screen"]
            assert "Agent writer" in back.splitlines()[1], back
            writer_heading = next(row for row in back.splitlines() if "writer ▾" in row)
            assert is_composer_heading(writer_heading), back
            if columns == 100:
                terminal.click_text(back, "writer ▾")
                terminal.expect("Switch Agent", also=("Create Agent",))
                terminal.press("\x1b")
                back = terminal.expect("F2 Back", state="writer ▾", absent=("Switch Agent",))["screen"]
            terminal.click_text(back, "F2 Back")
            terminal.press("\r")
            main = terminal.expect("main ▾", state="Received the delegated document.")["screen"]
            assert "Agent main" in main.splitlines()[1], main
            main_heading = next(row for row in main.splitlines() if "main ▾" in row)
            assert is_composer_heading(main_heading), main
            assert main_heading.index("⢎⡱") == writer_heading.index("⢎⡱"), main
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
        client.provider.next("update").tool("write_file", "summary", path="notes.md", content="# Report\n\n- Prepared the example.\n- Ran the check.\n- Checked the greeting.\n")
        client.provider.next("summary").text("Saved the example.")
        edit = "Edited   hello.py"
        write = "Added    notes.md"
        screen = terminal.expect("Saved the example.", also=(
            "1 file read · 3 file changes", "Added    hello.py", f"{edit} (+1 -1)", f"{write} (+5 -0)",
            "3 + - Prepared the example.", "5 + - Checked the greeting.",
            '1 - print("Hello")', '1 + print("Welcome")',
        ))["screen"]
        blank_row = re.compile(r"\b2 \+ +│")
        assert not blank_row.search(screen), screen
        terminal.click_text(screen, edit)
        changed = terminal.expect('1 - print("Hello")', also=('1 + print("Welcome")', edit, "(+1 -1)", write))["screen"]
        assert "Requested arguments · edit_file" not in changed, changed
        assert "Read hello.py" not in changed, changed
        terminal.click_text(changed, write)
        written = terminal.expect("│ 2 + ", also=(f"{write} (+5 -0)", "5 + - Checked the greeting."))["screen"]
        assert "# Report" in written and blank_row.search(written), written
        settled = terminal.expect("Saved the example.", state="Finished")["screen"]
        assert "Input accepted" not in settled and "Finished 0s" not in settled, settled
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
        screen = terminal.expect("中文勾选项", also=("保留待办项", "B Manual", "H₂O", "Msg"), rows=38)["screen"]
        operate_on_assistant(terminal, screen, "<sup>", "Source")
        terminal.expect("x<sup>2</sup>")
        terminal.request("resize", cols=70, rows=38)
        terminal.expect("x<sup>2</sup>", also=("H<sub>2</sub>O",), rows=38)
        terminal.exit_cleanly()


def named_output_cards(client):
    workspace = client.workspace("work-named-output")
    pasted = "Checks to retain\n第一行: status recorded"
    log = "retained-screen-proof"
    host_log = "host-command-evidence"
    edited_text = "verified summary text\nfirst check passed\nsecond check passed\nlast check passed"
    prompt = "Capture one replay diagnostic"
    with client.terminal(workspace) as terminal:
        terminal.request("resize", rows=42, cols=108)
        terminal.expect("Ready")
        terminal.press("/save\r")
        terminal.expect("Save pasted text")
        terminal.press(f"\x1b[200~{pasted}\x1b[201~")
        terminal.expect("status recorded")
        terminal.press("\r")
        terminal.expect("Name saved value")
        terminal.press("checks\r")
        terminal.expect("Saved checks")
        client.provider.assert_idle(0.2)

        terminal.submit(prompt)
        call = client.provider.next(prompt)
        assert "status recorded" not in json.dumps(call.request)
        command = f"echo {log}&&exit /b 7" if os.name == "nt" else f"printf '%s\\n' {log}; exit 7"
        invocation = ({"program": "cmd.exe", "args": ["/D", "/S", "/C", command]} if os.name == "nt"
                      else {"program": "sh", "args": ["-c", command]})
        call.tool("run_command", "capture-log", title="Capture replay output", **invocation)
        after_command = client.provider.next("capture-log")
        session = client.session(workspace)
        query = urlencode({"workspace": workspace, "agent": "main"})
        entries = client.request("GET", f"/sessions/{session}/history?{query}").as_json()["entries"]
        result = next(entry["structured"] for entry in entries
                      if entry["type"] == "tool_result" and entry["tool_call_id"] == "capture-log")
        assert result["termination"] == "exited" and result["exit_code"] == 7, result
        assert result["capture_complete"] and not result["output_preview_truncated"], result
        reference = result["output_ref"]
        page_range = f"0–{reference['bytes']} / {reference['bytes']} bytes"
        host_invocation = {**invocation, "args": [arg.replace(log, host_log) for arg in invocation["args"]]}
        after_command.tool(
            "run_js", "summarize-log",
            title="Summarize saved replay",
            code=f"""const chunk = await tools.read_output({{id: {json.dumps(reference['id'])}}});
await tools.write_file({{path: "summary.txt", content: chunk.result.segments[0].text}});
await tools.run_command({json.dumps(host_invocation)});
try {{ await tools.run_command({{program: "kuku-deliberately-missing-fixture"}}); }} catch {{}}
await tools.read_file({{path: "summary.txt"}});
await tools.edit_file({{path: "summary.txt", old_text: {json.dumps(log)}, new_text: {json.dumps(edited_text)}}});
await tools.read_output({{id: {json.dumps(reference['id'])}, offset: 9, limit: 6}});
return {{verified: {{path: "summary.txt", ok: true}}}};""",
        )
        client.provider.next("summarize-log").text("Replay inspected.")
        display = terminal.expect("Replay inspected.", also=(
            "Capture replay output", "exit 7", "Summarize saved replay", "2 file changes",
            "Edited   summary.txt (+4 -1)", "Added    summary.txt (+1 -0)", "1 - " + log,
        ))["screen"]
        terminal.click_text(display, "Capture replay output")
        display = terminal.expect("View output", also=("Capture replay output", "View arguments"),
                                  absent=("Requested arguments",))["screen"]
        terminal.click_text(display, "View arguments")
        display = terminal.expect("Hide arguments", also=(f"Program    {invocation['program']}", "Arguments", "Directory"),
                                  absent=("Requested arguments",))["screen"]
        terminal.click_text(display, "Hide arguments")
        display = terminal.expect("View output", also=("Capture replay output", "View arguments"),
                                  absent=("Hide arguments",))["screen"]
        terminal.click_text(display, "View output")
        inline = terminal.expect("Hide output", also=(log, page_range), absent=("S Name",))["screen"]
        assert any(line.strip(" │") == f"1 {log}" for line in inline.splitlines()), inline
        terminal.click_text(inline, "Hide output")
        display = terminal.expect("View output", also=("Capture replay output",), absent=("Hide output",))["screen"]
        terminal.click_text(display, "Capture replay output")
        display = terminal.expect("Summarize saved replay", absent=("View output",))["screen"]
        title = "Script   Summarize saved replay"
        closed_rows = display.splitlines()
        title_row = next(i for i, line in enumerate(closed_rows) if title in line)
        assert "partial" not in closed_rows[title_row], display
        terminal.click_text(display, title)
        display = terminal.expect("program missing", also=(
            "│   │ Status · partial", "Actions · 5/7 completed",
            "Added    summary.txt", "Edited   summary.txt", "Output · 1 field",
            "verified · 2 fields", "Input · JavaScript", "const chunk =",
        ), absent=("return {verified:", '"verified": {'))["screen"]
        positions = display.splitlines()
        assert closed_rows[title_row] == positions[title_row], display
        returned = next(i for i, line in enumerate(positions) if "Output · 1 field" in line)
        first_action = next(i for i, line in enumerate(positions) if "Added    summary.txt" in line)
        source = next(i for i, line in enumerate(positions) if "Input · JavaScript" in line)
        host = next(i for i, line in enumerate(positions) if "Actions ·" in line)
        assert source < host < first_action < returned, display
        assert positions[source].index("Input ·") == positions[host].index("Actions ·"), display
        assert positions[host].index("Actions ·") == positions[returned].index("Output ·"), display
        assert positions[first_action].index("Added ") > positions[host].index("Actions ·"), display
        assert "│   │   │ verified · 2 fields" in positions[returned + 1], display
        closed_row = returned
        terminal.click_text(display, "Output · 1 field")
        display = terminal.expect('"verified": {', also=('"path": "summary.txt"', "Output · 1 field"),
                                  absent=("return {verified:",))["screen"]
        rows = display.splitlines()
        returned = next(i for i, line in enumerate(rows) if "Output · 1 field" in line)
        assert returned == closed_row and "│   │ Output" in rows[returned], display
        assert "│   │   │ 1 {" in rows[returned + 1], display
        terminal.click_text(display, "Input · JavaScript")
        display = terminal.expect("return {verified:", also=("Input · JavaScript",),
                                  absent=('"verified": {',))["screen"]
        terminal.click_text(display, "Input · JavaScript")
        terminal.expect("Input · JavaScript", also=("const chunk =",), absent=("return {verified:",))
        read_row = "output bytes 9..15"
        display = terminal.expect(read_row, also=("Output · 1 field",))["screen"]
        terminal.click_text(display, read_row)
        viewer = terminal.expect("Read saved output", state="S Name", also=(
            f"9–15 / {reference['bytes']} bytes",))["screen"]
        assert any(line.strip() == "screen" for line in viewer.splitlines()), viewer
        assert log not in viewer, viewer
        terminal.press("\x1b")
        display = terminal.expect(read_row, also=("Output · 1 field",), absent=("S Name",))["screen"]
        host_command = f"Run {invocation['program']} ["

        def host_preview_visible(screen):
            rows = screen.splitlines()
            for index, row in enumerate(rows):
                if host_command not in row:
                    continue
                detail = rows[index + 1:index + 6]
                if (any(line.count("│") >= 2 and "Output · 1 line" in line for line in detail)
                        and any(line.count("│") >= 2 and "View output" in line for line in detail)):
                    return True
            return False

        def screen_with_host_expansion(expanded):
            screen = terminal.request("screen")["screen"]
            if host_command in screen and host_preview_visible(screen) == expanded:
                return screen

        terminal.click_text(display, host_command)
        try:
            display = eventually("the selected host command output preview", lambda: screen_with_host_expansion(True))
        except AssertionError as error:
            raise AssertionError(terminal.request("screen")["screen"]) from error
        terminal.click_text(display, "View output")
        inline = terminal.expect("Hide output", also=(host_log, page_range), absent=("S Name",))["screen"]
        assert any(line.strip(" │") == f"1 {host_log}" for line in inline.splitlines()), inline
        terminal.click_text(inline, "Hide output")
        display = eventually("the host command still expanded after hiding output",
                             lambda: screen_with_host_expansion(True))
        terminal.click_text(display, host_command)
        display = eventually("the collapsed host command", lambda: screen_with_host_expansion(False))
        def has_write_preview(screen):
            rows = screen.splitlines()
            return any("Added    summary.txt" in row and log in after for row, after in zip(rows, rows[1:]))
        assert has_write_preview(display), display
        terminal.click_text(display, "Edited   summary.txt")
        display = terminal.expect("4 + last check passed", also=("1 - " + log, "Edited   summary.txt"))["screen"]
        edit_row = next(i for i, line in enumerate(display.splitlines()) if "Edited   summary.txt" in line)
        assert "1 - " + log in display.splitlines()[edit_row + 2] and has_write_preview(display), display
        terminal.click_text(display, "Edited   summary.txt")
        display = terminal.expect(
            "Edited   summary.txt", also=("1 - " + log,), absent=("4 + last check passed",),
        )["screen"]
        assert (workspace / "summary.txt").read_text(encoding="utf-8") == edited_text + "\n"
        operate_on_message(terminal, display, "Tool · run_command", None, "Full output")
        terminal.expect(log, state="S Name", also=(page_range,))
        terminal.press("\x1b")
        display = terminal.expect("Edited   summary.txt", absent=("S Name",))["screen"]
        operate_on_message(terminal, display, "Tool · run_js", None, "Full output")
        outputs = terminal.expect("Outputs for selected message")["screen"]
        terminal.click_text(outputs, "Script action 3 · Run command")
        terminal.expect(host_log, state="S Name", also=(page_range,))
        entries = client.request("GET", f"/sessions/{session}/history?{query}").as_json()["entries"]
        script_result = next(entry["structured"] for entry in entries
                             if entry["type"] == "tool_result" and entry["tool_call_id"] == "summarize-log")
        reference = script_result["actions"][2]["result"]["output_ref"]
        assert reference["id"] != result["output_ref"]["id"], script_result
        terminal.press("s")
        terminal.expect("Name saved value")
        terminal.press("baseline\r")
        terminal.expect("Saved baseline")
        terminal.exit_cleanly()

    before = client.connection()["instance_id"]
    client.cli(workspace, "server", "restart")
    assert before != client.connection()["instance_id"]
    values = client.request("GET", f"/sessions/{session}/values?{query}").as_json()["values"]
    assert [(value["name"], value["kind"]) for value in values] == [
        ("baseline", "output"), ("checks", "text"),
    ], values
    assert values[0]["reference"] == reference, values

    with client.terminal(workspace) as terminal:
        terminal.request("resize", rows=38, cols=105)
        terminal.resume(prompt)
        terminal.expect("Replay inspected.")
        terminal.press("/values\r")
        terminal.expect("baseline", state="Saved values", also=("checks",))
        terminal.press("baseline\r")
        terminal.expect("Attach to draft", also=("Read saved value",))
        terminal.press("\x1b[B\r")
        restored = terminal.expect(host_log, state="baseline", also=(page_range,))["screen"]
        terminal.press("\x1b")
        eventually("closing the saved output viewer", lambda: (
            "Enter steer" in (screen := terminal.request("screen")["screen"])
            and page_range not in screen and "S Name" not in screen
        ))
        terminal.press("/values\r")
        terminal.expect("baseline", state="Saved values")
        terminal.press("baseline\r")
        terminal.expect("Attach to draft")
        terminal.press("\r")
        attached = terminal.expect("Attached · baseline")["screen"]
        terminal.submit("Read the attached replay output")
        call = client.provider.next("Read the attached replay output")
        assert reference["id"] in json.dumps(call.request["messages"][-1]), call.request["messages"][-1]
        call.tool("read_output", "read-baseline", id=reference["id"])
        recalled = client.provider.next("read-baseline")
        recalled.expect(host_log)
        recalled.text("Verified the saved replay output.")
        terminal.expect("Verified the saved replay output.", state="Finished")
        terminal.exit_cleanly()
    client.provider.assert_idle()
    evidence = {
        "session": session, "values": values, "command_exit": result["exit_code"],
        "partial_script_screen": display, "reopened_output_screen": restored, "attached_screen": attached,
    }
    (client.directory / "named-output.json").write_text(
        json.dumps(evidence, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    print("Named output round trip: " + json.dumps(evidence, ensure_ascii=False))


def windows_diagnostic(client):
    import ctypes

    workspace = client.workspace("work-command-diagnostic")
    command = "dir /b kuku-absent-38c207b6.txt"
    reference = subprocess.run(["cmd.exe", "/d", "/s", "/c", command], cwd=workspace, capture_output=True)
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
        schema = next(tool["input_schema"] for tool in request.request["tools"] if tool["name"] == "run_command")
        assert schema["required"] == ["program"], schema
        assert set(schema["properties"]) == {"program", "args", "cwd", "background", "title"}, schema
        request.tool("run_command", "missing-example", program="cmd.exe", args=["/D", "/S", "/C", command])
        result = client.provider.next("missing-example")
        assert diagnostic in json.dumps(result.request, ensure_ascii=False)
        result.text("Check complete.")
        screen = terminal.expect(diagnostic, also=("Check complete.", "exit 1"))["screen"]
        assert "␍" not in screen and "�" not in screen and "command exited" not in screen and "View output" not in screen
        rows = screen.splitlines()
        header = next(i for i, line in enumerate(rows) if line.lstrip().startswith("╭─ cmd.exe "))
        assert command in rows[header] and "exit 1" in rows[header], screen
        assert diagnostic in rows[header + 1] and rows[header + 2].lstrip().startswith("╰"), screen
        terminal.click_text(screen, "cmd.exe ")
        expanded = terminal.expect("View arguments", state=diagnostic)["screen"]
        terminal.click_text(expanded, "View arguments")
        expanded = terminal.expect("Hide arguments", also=("Program    cmd.exe", "Arguments", "Directory"))["screen"]
        terminal.click_text(expanded, "Hide arguments")
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
    args = ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", "output.ps1"]
    reference = subprocess.run([shell, *args], cwd=workspace, capture_output=True,
                               creationflags=subprocess.CREATE_NO_WINDOW, check=True)
    stdout = reference.stdout.decode("oem").strip()
    stderr = reference.stderr.splitlines()[0].decode("oem").removeprefix("\x1b[31;1m").removesuffix("\x1b[0m")
    with client.terminal(workspace) as terminal:
        terminal.expect("Ready")
        terminal.submit("Check shell output")
        client.provider.next().tool("run_command", "encoding", program=shell, args=args)
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
            screen = terminal.expect("Thinking")["screen"]
            assert "View thinking" not in screen and hidden not in screen
            waiting.release.set()
            assert published.arrived.wait(8)
            compact = terminal.expect("View thinking", state="Check")["screen"]
            flow_row = next(i for i, row in enumerate(compact.splitlines()) if "Thinking" in row and "Check" in row)
            header_row = flow_row - 1
            assert "Assistant" in compact.splitlines()[header_row] and "View thinking" in compact.splitlines()[header_row]
            assert not any("Check" in row for row in compact.splitlines() if is_composer_heading(row))
            terminal.click_text(compact, "View thinking")
            reader = terminal.expect("Comparing the published results.", state="Esc back")["screen"]
            assert hidden not in reader and "Esc back" in reader.splitlines()[header_row] and "Enter steer" in reader
            terminal.press("\x1b")
            terminal.expect("View thinking", state="Thinking")
            with client.terminal(workspace) as attached:
                attached.resume(prompt)
                restored = attached.expect("View thinking", state="Thinking")["screen"]
                assert hidden not in restored
                attached.exit_cleanly()
        finally:
            waiting.release.set()
            published.release.set()
        completed = terminal.expect(answer, state="Finished", also=("Turn 1", "View thinking"))["screen"]
        assert completed.count("View thinking") == 1 and hidden not in completed
        assert answer in completed.splitlines()[flow_row], completed
        operate_on_assistant(terminal, completed, answer, "Source")
        source = terminal.expect(answer, state="Assistant · source")["screen"]
        assert "Checking the totals." not in source and hidden not in source
        terminal.exit_cleanly()
    with client.terminal(workspace) as reopened:
        reopened.resume(prompt)
        settled = reopened.expect(answer, also=("Assistant   View thinking",))["screen"]
        reopened.press("\x0f")
        quiet = reopened.expect(answer, state="View thinking")["screen"]
        assert "Comparing the published results." not in quiet
        reopened.request("resize", rows=17, cols=100)
        quiet = reopened.expect(answer, state="View thinking")["screen"]
        operate_on_assistant(reopened, quiet, answer, "Thinking")
        notes = reopened.expect("Comparing the published results.", state="Esc back", rows=17)["screen"]
        assert hidden not in settled and hidden not in notes
        outside_notes = len(notes.splitlines())
        note_row = next(i for i, row in enumerate(notes.splitlines()) if "Note 5" in row)
        last_note = max(int(row.rsplit("Note ", 1)[1].strip()) for row in notes.splitlines() if "│ Note " in row)
        reopened.press(f"\x1b[<65;5;{outside_notes}M")
        scrolled = reopened.expect(f"Note {last_note + 3}", also=("Note 5", "Reading history"), rows=17)["screen"]
        moved = next(i for i, row in enumerate(scrolled.splitlines()) if "Note 5" in row)
        assert moved == note_row - 3, scrolled
        assert "View thinking" not in scrolled
        reopened.press("\x1b[6~")
        reopened.press("\x1b[6~")
        reopened.expect("Note 20", also=(answer,))
        reopened.press("\x1b[5~")
        reopened.press("\x1b[5~")
        expanded = reopened.expect("Comparing the published results.", state="Esc back")["screen"]
        reopened.click_text(expanded, "Thinking ▾")
        restored = reopened.expect(answer, state="Assistant   View thinking")["screen"]
        assert "Esc back" not in restored and "Note 5" not in restored

        reopened.exit_cleanly()


def check_agent_reading_position(client, terminal):
    terminal.press("\x1b[1;5F")
    prompt = "Print numbered checkpoints for review"
    terminal.submit(prompt)
    markers = [f"Checkpoint {number:03}" for number in range(70)]
    client.provider.next(prompt).text("\n\n".join(markers))
    terminal.expect(markers[-1], state="Finished")
    follow_up = "Record a checkpoint while the terminal is reading history"
    queued = "Process this after the checkpoint report"
    original = "First version of the checkpoint follow-up"
    discarded = "Obsolete checkpoint follow-up"
    beginning = "Streaming checkpoint update"
    new_content = "Another checkpoint arrived while reading"

    def queue_and_composer(screen):
        rows = screen.splitlines()
        start = next((index for index, line in enumerate(rows) if "Pending inputs" in line), None)
        end = next((index for index, line in enumerate(rows) if is_composer_heading(line)), None)
        if start is None or end is None or start >= end:
            return [], []
        return rows[start:end], rows[end:]

    def composer_lines(screen):
        return queue_and_composer(screen)[1]

    def queued_row_contains(screen, text):
        lines, _ = queue_and_composer(screen)
        for index, line in enumerate(lines):
            if "Queued ·" not in line:
                continue
            stop = next(
                (next_index for next_index in range(index + 1, len(lines))
                 if "You ·" in lines[next_index]),
                len(lines),
            )
            if text in "\n".join(lines[index:stop]):
                return True
        return False

    def eventually_screen(description, predicate, *, return_snapshot=False):
        last_screen = ""

        def find_screen():
            nonlocal last_screen
            snapshot = terminal.request("screen")
            last_screen = snapshot["screen"]
            if not predicate(last_screen):
                return None
            return snapshot if return_snapshot else last_screen

        try:
            return eventually(description, find_screen)
        except AssertionError as error:
            raise AssertionError(
                f"{error}\nLast pending-input screen:\n{pending_inputs_excerpt(last_screen)}"
            ) from error

    terminal.submit(follow_up)
    with client.provider.next(follow_up).streaming_text(beginning, f"\n\n{new_content}", "\n\nCheckpoint stream finished") as pauses:
        terminal.expect(beginning, state="↓  Reply")
        terminal.type(original)
        terminal.press("\x1b\r")
        terminal.expect(original, state="Queued · next Turn")
        terminal.press("\x1b[A")
        terminal.press("\r")
        terminal.expect("Edit queued input", also=(original,))
        terminal.press("\x03")
        terminal.type(queued)
        terminal.press("\r")
        terminal.expect(queued, state="Queued · next Turn")
        terminal.type(discarded)
        terminal.press("\x1b\r")
        terminal.expect(discarded, state="Queued · next Turn")
        screen_snapshot = terminal.request("screen")
        screen = screen_snapshot["screen"]
        if not queued_row_contains(screen, discarded):
            screen_snapshot = eventually_screen(
                "discarded input to remain queued in its delivery row",
                lambda frame: queued_row_contains(frame, discarded),
                return_snapshot=True,
            )
            screen = screen_snapshot["screen"]
        click_row, click_column = click_queued_input(terminal, screen, discarded)
        try:
            eventually_screen(
                "mouse to focus the discarded queued input",
                lambda frame: has_selected_queued_input(frame, discarded),
            )
        except AssertionError as error:
            raise AssertionError(
                f"{error}\n{queued_input_click_context(click_row, click_column, screen_snapshot)}"
            ) from error
        terminal.press("\x1b[3~")

        def withdrawn_from_pending(screen):
            composer = composer_lines(screen)
            if (
                "Removed from queue" in screen
                and queued_row_contains(screen, queued)
                and not queued_row_contains(screen, discarded)
                and composer
                and "Draft" in composer[0]
                and discarded in "\n".join(composer[1:])
            ):
                return screen

        eventually_screen(
            "withdrawn input to leave the queued rows and restore its draft",
            withdrawn_from_pending,
        )
        terminal.press("\x03")

        def withdrawal_draft_cleared():
            result = terminal.request("screen")
            if result["exit_code"] is not None or result["console_restored"]:
                return None
            composer = composer_lines(result["screen"])
            if (
                composer
                and "Draft" not in composer[0]
                and "Enter steer" in "\n".join(composer[1:])
                and discarded not in "\n".join(composer[1:])
            ):
                return result

        eventually("restored withdrawal draft to clear", withdrawal_draft_cleared)
        check_reading_focus(terminal, markers, pauses[0], new_content, queued)
    delivered = client.provider.next(queued)
    messages = json.dumps(delivered.request["messages"], ensure_ascii=False)
    assert original not in messages and discarded not in messages and messages.count(queued) == 1
    delivered.text("Queued checkpoint processed")
    terminal.expect("Queued checkpoint processed", state="Finished")
    terminal.press("\x03")
    client.provider.assert_idle()


def check_reading_focus(terminal, markers, stream_pause, new_content, queued):
    terminal.press("\x1b[5~")
    screen = terminal.expect("Reading history · ↑↓ scroll")["screen"]
    visible = [line for line in markers if line in screen]
    assert len(visible) >= 2 and markers[-1] not in visible, screen
    anchor = visible[1]
    row = next(i for i, line in enumerate(screen.splitlines()) if anchor in line)

    def checkpoint_at(expected):
        reading = terminal.request("screen")["screen"]
        lines = reading.splitlines()
        return ("Reading history · ↑↓ scroll" in reading and "Enter steer" in reading
                and len(lines) > expected and anchor in lines[expected])

    terminal.press("\x1b[A")
    eventually("Up scrolls one row and keeps the draft empty", lambda: checkpoint_at(row + 1))
    terminal.press("\x1b[B")
    eventually("Down scrolls one row toward the latest reply", lambda: checkpoint_at(row))
    stream_pause.release.set()
    updated = terminal.expect("New content", state=queued)["screen"]
    assert anchor in updated.splitlines()[row] and new_content not in updated, updated
    terminal.type("draft at checkpoint")
    terminal.press("\x1b[1;3C")
    assert "Switch Agent" not in terminal.expect("reviewer ▾")["screen"]
    terminal.press("\x1b[1;3D")
    restored = terminal.expect("draft at checkpoint", state="Reading history · Editing")["screen"]
    restored_lines = restored.splitlines()
    assert anchor in restored_lines[row], (visible, row, restored)
    terminal.press("\x1b[1;3A")
    terminal.expect(new_content, state=queued)
    terminal.press("\x1b[<64;5;7M")
    scrolled = terminal.expect("Jump to latest")["screen"]
    jump = next(i for i, line in enumerate(scrolled.splitlines(), 1) if "Jump to latest" in line)
    column = scrolled.splitlines()[jump - 1].index("Jump to latest") + 1
    terminal.press(f"\x1b[<0;{column};{jump}M\x1b[<0;{column};{jump}m")

    def followed_latest():
        latest = terminal.request("screen")["screen"]
        if all(text in latest for text in (new_content, queued, "draft at checkpoint")) and "Reading history" not in latest:
            return latest

    eventually("Clicking Jump to latest resumes following with the draft intact", followed_latest)
    print(f"Returning to the Agent restored {visible[0]} through {visible[-1]} and its draft.")


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
        editor_row = next(i for i, row in enumerate(finished) if "Enter steer" in row)
        turn_row = next(i for i, row in enumerate(finished) if "Turn 1" in row and "in" in row)
        last_reply = next(row for row in finished if "After table 15" in row)
        left = last_reply.index("│")
        assert finished[turn_row].index("╰") == left, finished
        terminal.type("preserve draft")
        def rows():
            return [line.rstrip() for line in terminal.request("screen")["screen"].splitlines()]
        saw_border = False
        for _ in range(22):
            old = rows()
            terminal.press("\x1b[<64;10;10M")
            frame = eventually("the table viewport to scroll", lambda: (r if (r := rows()) != old else None))
            saw_border |= any("┌" in line for line in frame)
            assert all(line.strip().startswith("│") and line.strip().endswith("│")
                       for line in frame if "Before table" in line or "After table" in line), frame
            assert not has_standalone_assistant_rail(frame), frame
            if any("Before table 01" in line for line in frame):
                break
        assert "preserve draft" in frame[editor_row], frame
        floating = next(row for row in frame if "Turn 1" in row and any(hint in row for hint in ("History", "Alt+↑", "New", "Ctrl+End")))
        assert floating.index("Turn 1") == frame[editor_row].index("preserve draft"), frame
        assert saw_border and any("preserve draft" in line for line in frame)
        terminal.press("\x1b[1;5F")
        terminal.expect("After table 15", state="preserve draft")
        terminal.press("\x03")
        eventually(
            "Ctrl+C clears the table-view draft",
            lambda: result
            if (result := terminal.request("screen"))["exit_code"] is None
            and "preserve draft" not in result["screen"]
            else None,
        )
        terminal.exit_cleanly()
