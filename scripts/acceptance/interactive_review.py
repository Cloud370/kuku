"""Generate and inspect a reusable multi-Turn settlement review Session."""

import json
import re
import subprocess
import sys
import time
from interactive_fixture import (
    DenseTranscript, TOOL_PROGRAM, delegate_reviewer, eventually, page_up_after_rendered_resize,
)


def stage(call, actions, note=None):
    blocks = [{"type": "text", "text": note}] if note else []
    blocks.extend({"type": "tool_use", "id": key, "name": tool, "input": values}
                  for key, tool, values in actions)
    call.reply(blocks)


def run(client, terminal, prompt):
    terminal.submit(prompt)
    return client.provider.next(prompt)


def result_fact(root, call_id):
    for path in root.rglob("events.jsonl"):
        with path.open() as journal:
            for record in map(json.loads, journal):
                if record.get("kind") == "tool.result" and record.get("tool_call_id") == call_id:
                    return record["structured"]
    raise AssertionError(f"Missing committed tool result: {call_id}")


def seed(workspace):
    for folder in ("billing", "exports", "docs", "tests", "tools"):
        (workspace / folder).mkdir()
    (workspace / "billing/settlement.py").write_text(
        'import csv\n\n\ndef total(path):\n    with open(path, newline="") as source:\n'
        '        rows = csv.DictReader(source)\n        return sum(int(row["amount"]) for row in rows if row["status"] == "paid")\n',
        encoding="utf-8",
    )
    for day in range(1, 25):
        (workspace / f"exports/2026-11-{day:02}.csv").write_text(
            'status,amount,description\npaid,1000,订阅\nrefunded,-300,部分退款\npending,999,未入账\n', encoding="utf-8",
        )
    (workspace / "tests/test_settlement.py").write_text(
        'import unittest\nfrom billing.settlement import total\n\n\nclass SettlementTest(unittest.TestCase):\n'
        '    def test_partial_refund(self):\n        self.assertEqual(total("exports/2026-11-01.csv"), 700)\n',
        encoding="utf-8",
    )
    (workspace / "docs/release-notes.md").write_text(
        '# 结算导出\n\n导出金额按分计算。退款仍需人工扣减。\n', encoding="utf-8",
    )
    (workspace / "docs/refund-workaround.md").write_text("旧版临时流程：手工扣减退款。\n", encoding="utf-8")
    (workspace / "tools/audit.py").write_text(
        'from pathlib import Path\nfrom billing.settlement import total\n\n'
        'for path in sorted(Path("exports").glob("*.csv")):\n'
        '    for channel in ("web", "app", "partner"):\n'
        '        print(f"{path.stem} | {channel:7} | net={total(path):4} cents | validated")\n',
        encoding="utf-8",
    )


LOCATE = "结算导出的退款没有扣减，先定位原因"
EDIT = "退款冲减净额，未入账继续排除；把测试补齐"
DELEGATE = "请子 Agent 独立复核退款边界和文档"
AUDIT = "生成逐日验收报告，并列出任何缺失项"
APPROVAL = "保留审批状态，确认发布范围"
CLOSE = "收尾，给我发布前检查清单"


WORKSPACE = "settlement-export"
WIDTHS = (174, 76)


def review_workspace(client):
    workspace = client.workspace(WORKSPACE)
    seed(workspace)
    return workspace


def verify_workspace(workspace):
    tests = subprocess.run(
        [sys.executable, "-B", "-m", "unittest", "discover", "-s", "tests", "-v"],
        cwd=workspace, capture_output=True, text=True, timeout=12,
    )
    notes = (workspace / "docs/release-notes.md").read_text(encoding="utf-8")
    return {
        "tests_pass_and_cover_boundaries": tests.returncode == 0 and "Ran 3 tests" in tests.stderr,
        "release_notes_updated": "手工扣减" not in notes and "退款仍需人工扣减" not in notes,
        "policy_present": (workspace / "docs/settlement-policy.md").is_file(),
        "stale_workaround_removed": not (workspace / "docs/refund-workaround.md").exists(),
    }


def replay_release_review(client, *, markers=(), settlements=()):
    workspace = client.directory / WORKSPACE
    snapshots = {}
    for width in WIDTHS:
        with client.terminal(workspace, color=True) as terminal:
            terminal.request("resize", rows=50, cols=width)
            terminal.resume(LOCATE)
            terminal.expect("Finished")
            frames = []
            seen = set()
            expected = set(markers) | {later for later, _ in settlements}

            def observe(previous=None):
                frame = terminal.request("screen", styled=True)
                return frame if frame["screen"] != previous else None

            frame = observe()
            for _ in range(18):
                screen = frame["screen"]
                lines = screen.splitlines()
                frames.append({key: frame[key] for key in ("screen", "styled")})
                seen.update(marker for marker in markers if marker in screen)
                for later, metrics in settlements:
                    if any(later in row for row in lines) and any(metrics in row for row in lines):
                        later_row = next(i for i, row in enumerate(lines) if later in row)
                        metrics_row = next(i for i, row in enumerate(lines) if metrics in row)
                        assert later_row < metrics_row, f"{metrics} must end its Turn"
                        seen.add(later)
                reached_first_turn = any(LOCATE in row for row in lines[1:])
                if seen >= expected and (expected or reached_first_turn):
                    break
                terminal.press("\x1b[5~")
                try:
                    frame = eventually("previous transcript page", lambda: observe(screen), timeout=2)
                except AssertionError:
                    break
            assert seen >= expected, f"{width}-column transcript is missing {sorted(expected - seen)}"
            snapshots[str(width)] = frames
            terminal.exit_cleanly()
    return snapshots


def write_renderings(path, frames):
    path.write_text(json.dumps(frames, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def release_review(client):
    workspace = review_workspace(client)
    config = client.home / "config.toml"
    config.write_text(config.read_text().replace("[handoff]\nenabled = false", "[handoff]\nenabled = true"))
    cli_test = dict(program=sys.executable, args=["-B", "-m", "unittest", "discover", "-s", "tests", "-v"])
    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.request("resize", rows=58, cols=156)
        stage(run(client, terminal, LOCATE), [
            ("find-exports", "find_files", {"path": "exports", "pattern": "*.csv"}),
            ("find-formula", "search_text", {"path": "billing", "pattern": "row.*status", "view": "lines"}),
            ("read-totals", "read_file", {"path": "billing/settlement.py"}),
            ("read-sample", "read_file", {"path": "exports/2026-11-01.csv"}),
        ], "我先核对导出样本和计算函数，再跑现有测试确认预期。")
        call = client.provider.next("read-sample").expect_tool_output("read-sample", "refunded")
        stage(call, [
            (f"read-day-{day:02}", "read_file", {"path": f"exports/2026-11-{day:02}.csv", "offset": 1, "limit": 4})
            for day in range(2, 17)
        ])
        call = client.provider.next("read-day-16").expect_tool_output("read-day-16", "pending")
        call.tool("run_command", "bad-test", title="复现部分退款少计", **cli_test)
        client.provider.next("bad-test").expect("AssertionError").text(
            "已复现。已支付 1,000 分，退款 -300 分，待结算金额排除。"
            "当前函数只接受 paid，所以测试得到 1,000 分，预期是 700 分。"
        )
        terminal.expect("已复现")
        session = client.session(workspace)

        stage(run(client, terminal, EDIT), [
            ("edit-total", "edit_file", {
                "path": "billing/settlement.py",
                "old_text": 'if row["status"] == "paid"', "new_text": 'if row["status"] in {"paid", "refunded"}',
            }),
            ("edit-notes", "edit_file", {
                "path": "docs/release-notes.md", "old_text": "退款仍需人工扣减。",
                "new_text": "已结算退款按带符号的金额冲减净额。\n未入账记录排除在净额之外。",
            }),
            ("write-regressions", "write_file", {
                "path": "tests/test_status_filters.py",
                "content": (
                    'import tempfile\nimport unittest\nfrom pathlib import Path\n'
                    'from billing.settlement import total\n\n\nclass FilterTest(unittest.TestCase):\n'
                    '    def evaluate(self, lines):\n        with tempfile.TemporaryDirectory() as folder:\n'
                    '            path = Path(folder) / "report.csv"\n'
                    '            path.write_text("status,amount\\n" + lines)\n'
                    '            return total(path)\n\n'
                    '    def test_pending_excluded(self):\n        self.assertEqual(self.evaluate("pending,999\\n"), 0)\n\n'
                    '    def test_refund_only(self):\n        self.assertEqual(self.evaluate("refunded,-300\\n"), -300)\n'
                ),
            }),
            ("write-policy", "write_file", {
                "path": "docs/settlement-policy.md",
                "content": "# 净额计算规则\n\n金额单位为分。\n\n- paid 计入。\n- refunded 保留负号。\n- pending 排除。\n",
            }),
        ], "计算规则和发布说明一起改，验收时能从测试追到业务定义。")
        call = client.provider.next("write-policy")
        for key in ("edit-total", "edit-notes", "write-policy", "write-regressions"):
            call.tool_result(key)
        call.tool("run_js", "inspect-workspace", title="核对计算函数、发布说明与导出目录", code="""
const implementation = await tools.read_file({path:'billing/settlement.py'});
const notes = await tools.read_file({path:'docs/release-notes.md'});
const exports = await tools.find_files({path:'exports', pattern:'*.csv'});
return {implementation: implementation.result.raw_text, notes: notes.result.raw_text,
        dailyExports: exports.result.entries.map(entry => entry.path)};
""".strip())
        call = client.provider.next("inspect-workspace").expect_tool_output("inspect-workspace", "dailyExports")
        call.tool("run_command", "good-test", title="执行退款回归测试", **cli_test)
        client.provider.next("good-test").expect("Ran 3 tests").text(
            "退款回归通过。实现和两份文档已经同步，待结算记录继续排除。"
        )
        terminal.expect("退款回归通过")

        prompt = DELEGATE
        task = "请独立检查 billing/settlement.py、现有测试与发布说明，对照部分退款样本。"
        terminal.submit(prompt)
        parent, child = delegate_reviewer(client.provider, prompt, "独立复核结算导出规则，确认退款符号与未入账状态。只读。", task)
        parent.text("已发给 reviewer 独立复核。我会根据复核结果决定是否扩大改动。")
        stage(child, [
            ("review-code", "read_file", {"path": "billing/settlement.py"}),
            ("review-notes", "read_file", {"path": "docs/release-notes.md"}),
            ("review-test", "run_command", {**cli_test, "title": "独立运行退款回归"}),
        ])
        child = client.provider.next("review-test").expect_tool_output("review-code", "refunded")
        child.expect("Ran 3 tests").text(
            "复核通过。负数退款冲减净额，pending 保持排除。"
            "建议在发布前保留一份逐日、逐渠道的可复查校验输出。"
        )
        eventually("reviewer finish", lambda: client.snapshot(workspace, session, "reviewer")["status"] == "idle")
        terminal.expect("Agent reviewer")

        stage(run(client, terminal, AUDIT), [
            ("audit-command", "run_command", {
                "program": sys.executable, "args": ["-B", "-m", "tools.audit"], "title": "生成逐日、逐渠道验收报告",
            }),
            ("missing-contract", "read_file", {"path": "docs/finance-approval.md"}),
        ])
        call = client.provider.next("audit-command").expect_tool_output("audit-command", "net= 700")
        call.tool("run_js", "check-references", title="检查发布说明与审批材料", code="""
const notes = await tools.read_file({path:'docs/release-notes.md'});
const approval = await tools.read_file({path:'docs/finance-approval.md'});
if (approval.status !== 'ok') throw new Error('审批材料缺失，发布说明已经核对');
return {notes: notes.result.raw_text, approval: approval.result.raw_text};
""".strip())
        call = client.provider.next("check-references").expect("审批材料缺失")
        call.tool("run_js", "retained-json", title="保存完整验收矩阵", code="""
const files = await tools.find_files({path:'exports', pattern:'*.csv'});
const checks = files.result.entries.flatMap(file =>
  ['web','app','partner','batch','refund','manual','billing','partner-api'].map(channel =>
    ({file:file.path, channel, currency:'CNY', expectation:'退款冲减净额，未入账记录排除', sampleNet:700})));
return {checked: checks.length, checks};
""".strip())
        call = client.provider.next("retained-json")
        audit_output = result_fact(client.home, "audit-command")["output_id"]
        stage(call, [
            ("read-audit", "read_output", {"id": audit_output}),
            ("stat-workaround", "stat_file", {"path": "docs/refund-workaround.md", "version": True}),
            ("collect-review", "list_agents", {}),
        ])
        call = client.provider.next("stat-workaround")
        call.expect_tool_output("read-audit", "net= 700")
        expected_version = result_fact(client.home, "stat-workaround")["content_hash"]
        call.tool("remove_file", "remove-workaround", path="docs/refund-workaround.md", expected_version=expected_version)
        client.provider.next("remove-workaround").text(
            "24 天样本均得到 700 分净额。逐日命令输出和验收矩阵已保存。财务审批文件待补。"
        )
        terminal.expect("24 天样本")

        call = run(client, terminal, APPROVAL)
        question = "是否只提交代码修复，审批单由财务随后补齐？"
        call.tool("ask_user", "approval-scope", question=question)
        terminal.expect(question)
        waiting = eventually("question committed", lambda: client.snapshot(workspace, session)["question"])
        answer = "是，只提交代码与测试；在发布说明标记等待财务审批。"
        with client.command(workspace, "reply", session, waiting["id"], "--answer", answer) as process:
            client.provider.next(answer).text("收到。代码和测试可合并，财务审批作为发布前置条件。")
            client.finish(process)
        terminal.expect("财务审批作为发布前置条件")

        terminal.press("/compact\r")
        client.provider.next("Write a checkpoint").fail()
        terminal.expect("Compaction failed")
        terminal.press("/compact retry\r")
        client.provider.next("Write a checkpoint").text(
            "## 已完成\n修复 billing/settlement.py，纳入负数退款，排除 pending，使用整数分。"
            "测试 3/3 通过，reviewer 独立复核通过。验收输出已保存。\n\n"
            "## 继续工作\n用户确认可以提交代码与测试，正式发布需等待财务审批。"
        )
        terminal.expect("Context compacted")

        call = run(client, terminal, CLOSE)
        fence = chr(96) * 3
        with call.streaming_text(
            "## 结算导出修复\n\n部分退款现在会冲减净额，待结算记录继续排除。\n\n",
            "| 检查 | 结果 |\n| --- | --- |\n| 回归测试 | 3/3 通过 |\n| 逐日样本 | 24 天通过 |\n| 独立复核 | 通过 |\n| 财务审批 | 等待补齐 |\n\n",
            f'计算单位保持为分。\n\n{fence}text\npaid  +1000\nrefund -300\nnet     700\n{fence}\n\n发布前请核对财务审批。',
            chunk_size=20,
        ) as pauses:
            for pause in pauses:
                assert pause.arrived.wait(timeout=4)
                time.sleep(.14)
                pause.release.set()
        terminal.expect("发布前请核对财务审批")
        terminal.exit_cleanly()
    checks = verify_workspace(workspace)
    assert all(checks.values()), checks
    frames = replay_release_review(
        client,
        markers=("Assistant", "Edited", "Agent reviewer", "Compaction failed", "messages summarized"),
        settlements=(("Answer", "Turn 5   "), ("Context compacted", "Turn 6   ")),
    )
    client.provider.assert_idle()
    write_renderings(client.directory / "renderings.json", frames)
    print(f"Settlement review Session {session} in {client.directory}")


def dense_transcript(client, reference=False):
    workspace = client.workspace("dense-transcript")
    scenario = DenseTranscript(client, workspace)
    screens = []
    tool_state = "Running tools"
    reply_state = "Streaming reply" if reference else "Reply"
    tool_heading = re.compile(
        r"(?:Read-only activity · \d+ calls|Read (?:file|\d+ files)|Find files|Search text|"
        r"Write file|Edit file|Run command|Tool [a-z_]+)"
        r"(?: ×\d+)? · (?:ok|error|running)\b"
        r"|^\s*(?:(?:│|╭─)\s*)?(?:\d+ file reads|Wrote\s+|Edited\s+)"
        rf"|^\s*╭─ {re.escape(TOOL_PROGRAM)}\b.* · running\b"
    )

    def row_of(screen, text):
        return next(row for row, line in enumerate(screen.splitlines(), 1) if text in line)

    def read_while(terminal, predicate, description):
        screen = ""

        def read():
            nonlocal screen
            screen = terminal.request("screen")["screen"]
            return screen if predicate(screen) else None

        try:
            return eventually(description, read)
        except AssertionError as error:
            raise AssertionError(f"{error}\nLast terminal screen:\n{screen}") from error

    def running_view(screen, height):
        rows = screen.split("\n")
        return len(rows) == height and any(tool_state in row for row in rows[-8:])

    def measure_tools(terminal):
        height = 144
        terminal.request("resize", rows=height, cols=140)
        screen = read_while(terminal, lambda value: running_view(value, height) and scenario.draft in value,
                            "the full tool transcript in a tall terminal")
        lines = screen.splitlines()
        headings = [index for index, line in enumerate(lines) if tool_heading.search(line)]
        last = next(index for index, line in enumerate(lines) if TOOL_PROGRAM in line)
        if reference:
            assert len(headings) == 21, screen
        else:
            first = next(index for index, line in enumerate(lines) if scenario.activity_group in line)
            assert lines[first].lstrip().startswith(f"╭─ {scenario.activity_group}"), screen
            assert all(not row.lstrip().startswith("╭─") for row in lines[first + 1:last + 1]), screen
        assert last >= headings[-1], screen
        return {"summary_rows": len(headings), "tool_rows": last - headings[0] + 1}

    def inspect_tools(terminal, before_resize):
        rows = 18
        terminal.request("resize", rows=rows, cols=140)
        try:
            before_page_up = page_up_after_rendered_resize(terminal, read_while, rows, (scenario.draft,))
        except AssertionError as error:
            raise AssertionError(
                f"{error}\nWide running-tools screen before resize to {rows}:\n{before_resize}"
            ) from error
        screens.append((f"{rows}-row running-tools screen before PageUp", before_page_up))
        try:
            collapsed = read_while(terminal, lambda value: running_view(value, rows)
                                   and scenario.activity_group in value and "Reading history" in value,
                                   "the original reading position before opening the group")
        except AssertionError as error:
            raise AssertionError(
                f"{error}\n{rows}-row running-tools screen before PageUp:\n{before_page_up}"
            ) from error
        screens.append((f"{rows}-row reading-history screen after PageUp", collapsed))
        assert scenario.read_target not in collapsed and scenario.draft in collapsed, (
            f"{collapsed}\n{rows}-row running-tools screen before PageUp:\n{before_page_up}"
        )
        anchor = row_of(collapsed, scenario.activity_group)
        terminal.click_text(collapsed, scenario.activity_group)
        anchored = terminal.expect("Read samples/read-01.txt", state=scenario.draft)["screen"]
        assert row_of(anchored, scenario.activity_group) == anchor
        terminal.request("resize", rows=38, cols=140)
        opened = terminal.expect(scenario.read_target, state="Search text · sample-marker in samples", also=(scenario.draft,))["screen"]
        assert row_of(opened, scenario.activity_group) == anchor and "sample-marker 12" not in opened, opened
        assert all(f"samples/read-{index:02d}.txt" in opened for index in range(1, 13)), opened
        assert "Find files · read-0*.txt in samples" in opened and "Search text · sample-marker in samples" in opened, opened
        screens.append(("Expanded read-only activity with file, find, and search targets", opened))
        terminal.click_text(opened, scenario.read_target)
        details = terminal.expect("sample-marker 12")["screen"]
        assert "Requested arguments" in details and scenario.read_target in details and scenario.draft in details, details
        screens.append(("One file's requested arguments and result from its detail row", details))
        terminal.click_text(details, "sample-marker 12")
        closed = read_while(terminal, lambda value: scenario.read_target in value and "sample-marker 12" not in value,
                            "the single file details to close")
        terminal.click_text(closed, scenario.activity_group)
        restored = read_while(terminal, lambda value: scenario.activity_group in value and scenario.read_target not in value,
                              "the group to return to the same reading position")
        assert row_of(restored, scenario.activity_group) == anchor and scenario.draft in restored, restored

        terminal.click_text(restored, scenario.missing)
        terminal.expect("Requested arguments")
        terminal.request("resize", rows=44, cols=140)
        error = terminal.expect(f"path does not exist: {scenario.missing}",
                                also=("Requested arguments", tool_state, scenario.draft), rows=44)["screen"]
        screens.append(("Grouped failed call with its exact error", error))
        terminal.click_text(error, "Read file · error")
        terminal.press("\x1b[1;5F")

    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.request("resize", rows=32, cols=140)
        terminal.submit(scenario.prompt)
        with scenario.running_tool():
            terminal.expect(tool_state)
            terminal.type(scenario.draft)
            measurements = measure_tools(terminal)
            overview_height = 36 if reference else 48
            terminal.request("resize", rows=overview_height, cols=140)
            wide = read_while(terminal, lambda value: running_view(value, overview_height)
                              and scenario.missing in value and scenario.draft in value,
                              "the wide tool turn")
            screens.append(("Dense tool turn with a running command and local draft", wide))
            if not reference:
                assert all(text in wide for text in (scenario.activity_group, *scenario.file_cards)), wide
                inspect_tools(terminal, wide)
            terminal.request("resize", rows=26, cols=74)
            running_card = "Run command · running" if reference else TOOL_PROGRAM
            narrow = read_while(terminal, lambda value: running_view(value, 26) and scenario.missing in value
                                and scenario.draft in value
                                and any(running_card in line and "running" in line for line in value.splitlines()),
                                "the narrow tool turn")
            screens.append(("Narrow tool turn with the same draft", narrow))
            terminal.request("resize", rows=32, cols=140)
        with scenario.streaming_reply():
            wide = terminal.expect(scenario.source, state=reply_state, also=(scenario.draft, scenario.heading), rows=32)["screen"]
            assert scenario.heading in wide and scenario.draft in wide and scenario.end not in wide and "rust" in wide, wide
            screens.append(("Wide Chinese report while the Rust fence is open", wide))
            terminal.request("resize", rows=26, cols=74)
            narrow = terminal.expect(scenario.source, state=reply_state, also=(scenario.draft,), rows=26)["screen"]
            assert scenario.draft in narrow and scenario.end not in narrow, narrow
            screens.append(("Narrow Chinese report while the Rust fence is open", narrow))
        if not reference:
            terminal.request("resize", rows=32, cols=74)
        complete = terminal.expect(scenario.end, state="Finished", also=(scenario.draft,),
                                   rows=26 if reference else 32)["screen"]
        assert all(text in complete for text in (scenario.draft, "验证项目", "十二项通过", "dense-literal verification saved")), complete
        screens.append(("Complete report and unchanged draft", complete))
        verification = scenario.verify()
        terminal.press("\x03")
        terminal.exit_cleanly()
    for label, screen in screens:
        print(label + "\n" + "\n".join(line.rstrip() for line in screen.splitlines()).rstrip())
    verification.update(measurements)
    print("Dense transcript: " + json.dumps(verification, ensure_ascii=False))
    return verification
