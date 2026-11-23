#!/usr/bin/env python3
"""Verify intended reply and workspace actions in native TUI sessions with a budgeted provider."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import sys
from urllib.parse import urlencode

from interactive_fixture import EXE, InstalledClient, eventually, install, is_composer_heading, scratch_directory, serve
from interactive_review import (
    APPROVAL, AUDIT, CLOSE, DELEGATE, EDIT, LOCATE, replay_release_review, review_workspace, verify_workspace,
)
from paid_model import RequestBoundary, RunLimits, paid_budget, route_for
from run import ROOT, config_record, load_secrets, now, redact, write_json


INTENT_REQUEST_BYTES = 64000
REVIEW_REQUEST_BYTES = 192 * 1024
TURN_TIMEOUT = 125
ARTIFACT_LIMIT = 64 * 1024
ROUTING_FIELDS = ("connection", "tier", "model", "reasoning_effort", "service_tier", "preset")


def files_at(workspace):
    files = {}
    for path in sorted(workspace.rglob("*")):
        name = path.relative_to(workspace).as_posix()
        if path.is_symlink():
            files[name] = {"kind": "symlink"}
        elif path.is_dir():
            files[name] = {"kind": "directory"}
        elif path.is_file():
            size = path.stat().st_size
            files[name] = {"bytes": size}
            if size <= ARTIFACT_LIMIT:
                files[name]["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
    return files


def expected_files(contents):
    return {name: {"bytes": len(body), "sha256": hashlib.sha256(body).hexdigest()} for name, body in contents.items()}


def history(client, workspace, session, agent="main"):
    params = urlencode({"workspace": workspace, "agent": agent, "limit": 128})
    return client.request("GET", f"/sessions/{session}/history?{params}").as_json()["entries"]


def tool_facts(entries):
    results = {row["tool_call_id"]: row for row in entries if row["type"] == "tool_result"}
    tools = []
    for row in entries:
        if row["type"] != "tool_call":
            continue
        result = results.get(row["tool_call_id"], {})
        summary = result.get("summary") or result.get("text") or ""
        tools.append({"name": row["tool"], "args": row["args"], "status": result.get("status"), "summary": summary[:440]})
    return tools


def selected_snapshot(client, workspace, session, agent="main"):
    params = urlencode({"workspace": workspace, "agent": agent})
    return client.request("GET", f"/sessions/{session}/snapshot?{params}").as_json()


def submit(terminal, prompt, marker=None):
    terminal.press("\x1b[200~" + prompt + "\x1b[201~")
    terminal.expect(marker or prompt.splitlines()[0][:26])
    terminal.press("\r")


def wait_for_turn(client, workspace, previous, timeout=TURN_TIMEOUT):
    def settled():
        sessions = client.sessions(workspace)
        if not sessions:
            return None
        assert len(sessions) == 1, "The test workspace must have one fresh Session"
        session = sessions[0]["session_id"]
        agent = client.snapshot(workspace, session)
        if (agent.get("turn") or 0) <= previous or agent["status"] == "running":
            return None
        return session, agent

    return eventually("the submitted native TUI turn to settle", settled, timeout=timeout)


def assistant_lines(screen):
    lines = [line.strip() for line in screen.splitlines()]
    start = max((index + 1 for index, line in enumerate(lines)
                 if re.fullmatch(r"Assistant(?:\s+View thinking)?", line)), default=len(lines))
    end = next((index for index in range(start, len(lines)) if is_composer_heading(lines[index])), len(lines))
    body = [line for line in lines[start:end] if line and line != "│"]
    summary_pattern = (r"(?:Turn \d+\s+)?\d+(?:h \d{2}m \d{2}|m \d{2})?s"
                       r"(?: · \d+(?:\.\d+)?[KMB]? (?:in|out)| · Cache \d+(?:\.\d+)?%)* · \d+ tools?")
    if body and re.fullmatch(summary_pattern, body[-1]):
        body.pop()
    return body


def read_only_inspection(tool, target=None):
    if tool["name"] != "run_command":
        return False
    args = tool["args"]
    literals = args.get("args", [])
    if not isinstance(literals, list) or any(not isinstance(arg, str) for arg in literals):
        return False
    if args.get("program"):
        if args["program"] == "od" and target and literals == ["-c", target]:
            return True
        command = " ".join((Path(args["program"]).stem, *literals))
    else:
        command = args.get("command", "").strip()
    return bool(re.fullmatch(r"(?:ls(?: +-[aAlh1]+)*(?: +\.)?|pwd|dir(?: +\.)?|Get-ChildItem(?: +\.)?)", command, re.IGNORECASE))


def tool_is_file_work(tool, target):
    return tool["name"] in {"read_file", "stat_file", "find_files", "search_text", "write_file", "edit_file"} or read_only_inspection(tool, target)


def used_default_route(config):
    return all(not (config.get(field) or "").strip() for field in ROUTING_FIELDS)


def save_screen(folder, name, screen, secret_values):
    (folder / f"{name}.txt").write_text(redact(screen, secret_values), encoding="utf-8")


def sanitize_report(report, secret_values):
    data = json.dumps(report, ensure_ascii=False)
    for secret_value in secret_values:
        if secret_value:
            data = data.replace(json.dumps(secret_value, ensure_ascii=False)[1:-1], "[REDACTED]")
    return json.loads(data)


def intent_turn(client, boundary, terminal, workspace, folder, secrets_to_mask, name, prompt,
                contents, previous=0, *, reply=None, path=None):
    before = files_at(workspace)
    boundary.phase = name
    submit(terminal, prompt)
    session, agent = wait_for_turn(client, workspace, previous)
    entries = [row for row in history(client, workspace, session) if row["turn"] == agent["turn"]]
    tools = tool_facts(entries)
    after = files_at(workspace)
    screen = terminal.request("screen")["screen"]
    finished = agent["status"] == "idle"
    if finished:
        screen = terminal.expect("Finished")["screen"]
    checks = {"finished": finished, "expected_file_bytes": after == expected_files(contents)}
    if reply is not None:
        def painted_reply():
            value = terminal.request("screen")["screen"]
            return value if "Finished" in value and assistant_lines(value) == [reply] else None

        if finished and reply in (agent.get("last_output") or ""):
            try:
                screen = eventually("the assistant wording in the native terminal", painted_reply, timeout=8)
            except AssertionError:
                screen = terminal.request("screen")["screen"]
        checks.update(visible_assistant_wording=assistant_lines(screen) == [reply],
                      no_tool_calls=not tools, unchanged_workspace=after == before)
    else:
        writes = [tool for tool in tools if tool["name"] in {"write_file", "edit_file"}]
        checks.update(file_tool_succeeded=any(tool["args"].get("path") == path and tool["status"] == "ok" for tool in writes),
                      writes_target_requested_file=all(tool["args"].get("path") == path for tool in writes),
                      no_unrelated_tools=all(tool_is_file_work(tool, path) for tool in tools))
    save_screen(folder, name, screen, secrets_to_mask)
    return {"name": name, "passed": all(checks.values()), "checks": checks, "session_id": session,
            "turn": agent["turn"], "assistant": (agent.get("last_output") or "")[:1200],
            "before": before, "after": after, "tools": tools, "screen": f"{name}.txt",
            "read_only_commands": [tool["args"] for tool in tools if read_only_inspection(tool, path)]}


def delivery_and_revision(client, boundary, folder, secret_values, cases):
    dispatch = client.workspace("intent-delivery")
    guard = b"Leave this archive unchanged.\n"
    (dispatch / "archive.txt").write_bytes(guard)
    first = "North loading gate opens at 06:45."
    second = "South garden closes at 18:15."
    saved = {"archive.txt": guard, "handoff.txt": (first + "\n").encode()}
    with client.terminal(dispatch) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.request("resize", rows=62, cols=150)
        case = intent_turn(client, boundary, terminal, dispatch, folder, secret_values, "write-first-handoff",
                           f"Create handoff.txt with exactly this single line and a trailing newline: {first}",
                           saved, path="handoff.txt")
        cases.append(case)
        case = intent_turn(client, boundary, terminal, dispatch, folder, secret_values, "answer-next-handoff",
                           f"第二条交接先发在聊天里。原文是「{second}」；只发这一句。",
                           saved, case["turn"], reply=second)
        cases.append(case)
        saved["published.txt"] = (second + "\n").encode()
        case = intent_turn(client, boundary, terminal, dispatch, folder, secret_values, "write-the-chat-line",
                           "把刚才那条聊天消息写进 published.txt。文件只保留原句和一个尾部换行。",
                           saved, case["turn"], path="published.txt")
        cases.append(case)
        terminal.exit_cleanly()

    workshop = client.workspace("intent-variation")
    source = b"item,category\ngoggles,paint\nbrush,paint\n"
    (workshop / "inventory.csv").write_bytes(source)
    with client.terminal(workshop) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.request("resize", rows=62, cols=150)
        sign = "Badge collection moves to Hall C."
        case = intent_turn(client, boundary, terminal, workshop, folder, secret_values, "museum-desk-copy",
                           f"The museum kiosk has room for just one line. What should the desk sign say? Use the exact wording ‘{sign}’.",
                           {"inventory.csv": source}, reply=sign)
        cases.append(case)
        case = intent_turn(client, boundary, terminal, workshop, folder, secret_values, "correct-workshop-inventory",
                           "The workshop inventory has goggles under paint. Please correct that category to safety in inventory.csv and keep the other entries.",
                           {"inventory.csv": source.replace(b"goggles,paint", b"goggles,safety")}, case["turn"], path="inventory.csv")
        cases.append(case)
        terminal.exit_cleanly()


def delegation(client, boundary, folder, secret_values, route):
    workspace = client.workspace("intent-first-delegation")
    stamp = "CEDAR-FORK-17"
    (workspace / "review.txt").write_text(stamp + "\n", encoding="utf-8")
    boundary.phase = "first-delegation"
    prompt = ("Create an independent Agent named reviewer with its default route (leave model, tier, and connection unspecified). "
              "Give reviewer the ability to send a message back to main. Ask it to read the one-line stamp in review.txt "
              "and use send_message to report the actual stamp to main. "
              "Tell me the stamp after reviewer messages it back.")
    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.request("resize", rows=62, cols=150)
        submit(terminal, prompt)
        observed = {}

        def settled():
            sessions = client.sessions(workspace)
            if not sessions:
                return None
            assert len(sessions) == 1
            session = sessions[0]["session_id"]
            root = selected_snapshot(client, workspace, session)
            exists = any(agent["address"] == "reviewer" for agent in root["agents"])
            reviewer = selected_snapshot(client, workspace, session, "reviewer") if exists else {}
            observed.update(session=session, root=root["agent"], reviewer=reviewer)
            agents = [root["agent"], reviewer["agent"]] if reviewer else [root["agent"]]
            if root["agent"].get("turn") and all(agent["status"] != "running" and not agent["pending_inputs"] for agent in agents):
                return True

        try:
            completed = eventually("the main and first delegated Agent to settle", settled, timeout=TURN_TIMEOUT)
        except AssertionError:
            if not observed:
                raise
            completed = False
        session, reviewer = observed["session"], observed["reviewer"]
        main_history = history(client, workspace, session)
        main_tools = tool_facts(main_history)
        reviewer_tools = tool_facts(history(client, workspace, session, "reviewer")) if reviewer else []
        create_requests = [tool for tool in main_tools if tool["name"] == "create_agent"]
        created = [tool for tool in create_requests if tool["status"] == "ok"]
        first_create_status = create_requests[0]["status"] if create_requests else None
        selection = reviewer.get("selection") or {}
        received = next((row["id"] for row in main_history if row["type"] == "user"
                         and row.get("from") == "reviewer" and stamp in row.get("text", "")), None)
        spoken_stamp = stamp in (observed["root"].get("last_output") or "")
        answered_after_receipt = received is not None and any(
            row["id"] > received and row["type"] == "assistant" and stamp in row.get("text", "") for row in main_history
        )
        screen = terminal.expect(stamp, state="Finished")["screen"] if spoken_stamp else terminal.request("screen")["screen"]
        checks = {
            "agents_settled": completed and observed["root"]["status"] == "idle" and reviewer.get("agent", {}).get("status") == "idle",
            "first_create_request_succeeded": first_create_status == "ok",
            "first_created_agent_used_defaults": len(created) == 1 and created[0]["args"].get("address") == "reviewer"
                and used_default_route(created[0]["args"]),
            "child_model_request_and_route": reviewer.get("agent", {}).get("usage", {}).get("model_requests", 0) > 0
                and selection.get("model") == route.model and selection.get("connection") == route.connection
                and (selection.get("settings") or {}).get("reasoning_effort") == route.effort,
            "task_sent_to_child": any(tool["name"] == "send_message" and tool["status"] == "ok"
                                      and tool["args"].get("to") == "reviewer" for tool in main_tools),
            "child_read_owned_task_file": any(tool["name"] == "read_file" and tool["status"] == "ok"
                                              and tool["args"].get("path") == "review.txt" for tool in reviewer_tools),
            "child_reported_to_main": any(tool["name"] == "send_message" and tool["status"] == "ok"
                                          and tool["args"].get("to") == "main" and stamp in tool["args"].get("message", "")
                                          for tool in reviewer_tools),
            "main_answered_after_child_receipt": answered_after_receipt,
            "main_showed_actual_stamp": spoken_stamp and any(stamp in line for line in assistant_lines(screen)),
            "unchanged_workspace": files_at(workspace) == expected_files({"review.txt": (stamp + "\n").encode()}),
        }
        save_screen(folder, "first-delegation", screen, secret_values)
        terminal.exit_cleanly()
    return {"name": "first-delegation", "passed": all(checks.values()), "checks": checks, "session_id": session,
            "child_selection": {key: selection.get(key) for key in ("model", "connection", "tier", "settings")},
            "first_create_request_status": first_create_status, "child_report_in_main_event_id": received,
            "main_tools": main_tools, "child_tools": reviewer_tools, "screen": "first-delegation.txt"}


def live_release_review(client, boundary, folder, secret_values):
    workspace = review_workspace(client)
    stages = (
        ("locate", LOCATE, "先执行 python -B -m unittest discover -s tests -v 并定位原因，此轮只排查。"),
        ("edit", EDIT, (
            "只修 billing/settlement.py 中的净额规则，更新 docs/release-notes.md，新增 docs/settlement-policy.md。"
            "只新增两个单元测试，分别验证只含退款时为负数、只含待结算记录时为零。执行三个测试。"
        )),
        ("delegate", DELEGATE, (
            "创建地址为 reviewer 的只读子 Agent，向它分派核对实现、文档和测试的任务，等它完成再汇报。"
        )),
        ("audit", AUDIT, (
            "执行 python -B -m tools.audit，按实际输出给出结果。检查 docs/finance-approval.md 是否存在。"
            "移除已被替换的 docs/refund-workaround.md，先验证路径和文件版本。"
        )),
        ("approval", APPROVAL, "调用 ask_user 询问我是否只提交代码、把财务审批留为发布前置。"),
    )
    turns = []
    previous = 0
    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.request("resize", rows=55, cols=160)
        for name, prompt, detail in stages:
            boundary.phase = name
            submit(terminal, prompt + "\n" + detail, marker=prompt)
            session, agent = wait_for_turn(client, workspace, previous)
            previous = agent["turn"]
            turns.append({"phase": name, "turn": previous, "status": agent["status"]})
            if agent.get("question"):
                boundary.phase = "answer"
                answer = "是，只提交代码和测试，将财务审批保留为发布前置条件。"
                with client.command(workspace, "reply", session, agent["question"]["id"], "--answer", answer) as process:
                    client.finish(process)
                session, agent = wait_for_turn(client, workspace, previous)
                previous = agent["turn"]
                turns.append({"phase": "answer", "turn": previous, "status": agent["status"]})
            assert agent["status"] == "idle", f"{name} settled as {agent['status']}"
        boundary.phase = "compact"
        terminal.press("/compact\r")
        compaction = eventually(
            "manual compaction to settle",
            lambda: next((entry for entry in reversed(history(client, workspace, session))
                          if entry["type"] == "compaction"
                          and entry.get("status") in {"completed", "failed", "cancelled"}), None),
            timeout=TURN_TIMEOUT,
        )
        boundary.phase = "close"
        submit(terminal, CLOSE + "\n用 Markdown 表格给出测试、样本、子 Agent 复核、审批状态。", marker=CLOSE)
        session, agent = wait_for_turn(client, workspace, previous)
        turns.append({"phase": "close", "turn": agent["turn"], "status": agent["status"]})
        entries = history(client, workspace, session)
        terminal.exit_cleanly()
    checks = verify_workspace(workspace)
    checks.update(
        single_session_reused=all(first["turn"] < second["turn"] for first, second in zip(turns, turns[1:])),
        real_question_answered=any(turn["phase"] == "answer" for turn in turns),
        compaction_completed=compaction["status"] == "completed",
        final_turn_finished=agent["status"] == "idle",
    )
    tools = tool_facts(entries)
    checks["reviewer_created"] = any(tool["name"] == "create_agent" and tool["status"] == "ok" for tool in tools)
    markers = ["Assistant"]
    if any(tool["name"] == "edit_file" and tool["status"] == "ok" for tool in tools):
        markers.append("Edited")
    if checks["reviewer_created"]:
        markers.append("Agent reviewer")
    if checks["compaction_completed"]:
        markers.append("Context compacted")
    requests_before_replay = len(boundary.attempts)
    for width, frames in replay_release_review(client, markers=markers).items():
        save_screen(folder, f"settlement-review-{width}", "\n\n".join(frame["screen"] for frame in frames), secret_values)
        save_screen(folder, f"settlement-review-{width}-styled", "\n\n".join(frame["styled"] for frame in frames), secret_values)
    checks["replay_used_no_model_requests"] = len(boundary.attempts) == requests_before_replay
    return {"name": "release-review", "passed": all(checks.values()), "checks": checks,
            "session_id": session, "workspace": str(workspace), "turns": turns, "tools": tools}


def run_selected_live(args, directory, record, scratch, bridge, checked):
    artifact = args.archive or args.binary
    record["input"] = {"type": "release_archive" if args.archive else "installed_binary", **config_record(artifact)}
    if args.intent:
        script, label, evidence, key = "interactive_intent.py", "installed-intent", directory / "intent", "intent_result"
        product = ["--archive" if args.archive else "--binary", str(artifact.resolve())]
        options = ["--scratch-base", str(scratch), "--provider", args.intent, "--case", args.intent_case, "--revision", args.revision]
        for flag, value in (("model", args.intent_model), ("effort", args.intent_effort), ("upstream", args.intent_upstream)):
            if value is not None:
                options.extend(["--" + flag, value])
        options.extend(["--max-requests", str(args.intent_max_requests),
                        "--max-output-tokens", str(args.intent_max_output_tokens)])
        result = evidence / "result.json"
    else:
        script, label, evidence, key = "interactive_live.py", "installed-live", directory, "live_result"
        product = ["--archive", str(artifact.resolve())]
        options = ["--directory", str(scratch / "test")]
        result = evidence / "live-result.json"
    command = [sys.executable, str(Path(__file__).with_name(script)), *product, *options, "--bridge", str(bridge),
               "--evidence", str(evidence), "--run-id", record["run_id"], "--ledger", str(args.ledger.resolve()),
               "--secrets", str(args.secrets.resolve())]
    try:
        checked(command, label, timeout=900, cwd=scratch)
    except RuntimeError:
        if args.intent and result.is_file():
            intent_result = json.loads(result.read_text(encoding="utf-8"))
            if "error" not in intent_result and intent_result.get("upstream_confirmed"):
                record["failure_kind"] = "intent_checks"
        raise
    finally:
        if result.is_file():
            record[key] = str(result)
    record["passed"] = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    binary = parser.add_mutually_exclusive_group(required=True)
    binary.add_argument("--archive", type=Path)
    binary.add_argument("--binary", type=Path)
    for arg in ("bridge", "evidence", "ledger", "secrets"):
        parser.add_argument("--" + arg, type=Path, required=True)
    parser.add_argument("--scratch-base", type=Path)
    parser.add_argument("--keep-session", action="store_true", help="keep the review Session for rendering replay")
    parser.add_argument("--provider", required=True, choices=("deepseek", "openrouter"))
    parser.add_argument("--model")
    parser.add_argument("--effort", default="low")
    parser.add_argument("--upstream")
    parser.add_argument("--max-requests", type=int, default=36)
    parser.add_argument("--max-output-tokens", type=int, default=2048)
    parser.add_argument("--case", default="all", choices=("all", "delivery", "delegation", "release-review"))
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--revision", required=True)
    args = parser.parse_args()
    if args.keep_session and (args.case != "release-review" or args.scratch_base is None):
        parser.error("--keep-session requires --case release-review and an explicit --scratch-base")
    if not re.fullmatch(r"[0-9a-f]{7,40}", args.revision):
        parser.error("--revision must be a Git commit ID of at least seven hexadecimal characters")
    credentials = load_secrets(args.secrets)
    upstream_key = credentials.get(f"KUKU_TEST_{args.provider.upper()}_API_KEY")
    if not upstream_key:
        raise ValueError("The selected provider needs its acceptance credential")
    route = route_for(args.provider, model=args.model, effort=args.effort, upstream=args.upstream, key=upstream_key)
    request_bytes = REVIEW_REQUEST_BYTES if args.case == "release-review" else INTENT_REQUEST_BYTES
    limits = RunLimits(args.max_requests, request_bytes, args.max_output_tokens)
    loopback_key = secrets.token_hex(32)
    masked = [loopback_key, *(value for name, value in credentials.items() if name.endswith(("_KEY", "_TOKEN", "_SECRET")))]
    boundary = RequestBoundary(route, limits, upstream_key, loopback_key)
    budget = paid_budget(args.ledger, args.provider)
    artifact = (args.archive or args.binary).resolve()
    evidence = args.evidence.resolve()
    if args.scratch_base and args.scratch_base.resolve().is_relative_to(ROOT):
        raise ValueError("Intent installation requires a scratch directory outside the source tree")
    assert args.bridge.is_file() and args.ledger.is_file()
    evidence.mkdir(parents=True)
    report = {"passed": False, "run_id": args.run_id, "started_at": now(), "revision": args.revision,
              "selected_case": args.case, "artifact": config_record(artifact), "bridge": config_record(args.bridge),
              **route.evidence(limits), "ledger": str(budget.path), "requests": boundary.attempts,
              "transport_rejections": boundary.rejections, "cases": []}
    try:
        with (scratch_directory(args.scratch_base, report, prefix="kuku-live-intent-", retain=args.keep_session) as scratch,
              serve(boundary.handler()) as endpoint):
            executable = scratch / "data" / "bin" / ("kuku" + EXE)
            client = InstalledClient(executable, args.bridge.resolve(), scratch)
            client.env.update(KUKU_INTENT_FIXTURE_TOKEN=loopback_key,
                              PATH=str(executable.parent) + os.pathsep + client.env["PATH"])
            if args.archive:
                install(artifact, client.home, client.env)
            else:
                executable.parent.mkdir()
                shutil.copy2(artifact, executable)
            (client.home / "config.toml").write_text(route.config(endpoint, limits), encoding="utf-8")
            version, _ = client.cli(scratch, "--version")
            report.update(version=version.strip(), binary=config_record(executable))
            installed_revision = re.search(r"\(([0-9a-f]{7,40})\)$", version.strip())
            assert installed_revision and installed_revision[1].startswith(args.revision[:len(installed_revision[1])]), (
                "Installed source revision does not match the selection"
            )
            try:
                with client.terminal(scratch) as terminal:
                    terminal.expect("New session", state="Ready")
                    selected = client.request("GET", "/models/resolve").as_json()["selection"]
                    assert selected["model"] == route.model and selected["connection"] == route.connection
                    assert selected["settings"]["reasoning_effort"] == route.effort and not boundary.attempts
                    terminal.exit_cleanly()
                boundary.reserve(budget, args.run_id, f"Native intent: {args.case}")
                if args.case in {"all", "delivery"}:
                    delivery_and_revision(client, boundary, evidence, masked, report["cases"])
                if args.case in {"all", "delegation"}:
                    report["cases"].append(delegation(client, boundary, evidence, masked, route))
                if args.case == "release-review":
                    report["cases"].append(live_release_review(client, boundary, evidence, masked))
            finally:
                client.stop()
                if args.keep_session:
                    replay_config = route.config("http://127.0.0.1:9", limits)
                    replay_config = replay_config.replace("$KUKU_INTENT_FIXTURE_TOKEN", "review-replay")
                    (client.home / "config.toml").write_text(replay_config, encoding="utf-8")
        report["upstream_confirmed"] = boundary.confirm_upstream()
        report["passed"] = report["upstream_confirmed"] is True and bool(report["cases"]) and not boundary.rejections \
            and all(result["passed"] for result in report["cases"])
    except Exception as error:
        report["error"] = type(error).__name__ + ": " + redact(str(error), masked)[:2400]
    finally:
        report.update(finished_at=now(), accounting=boundary.accounting(), reservation_applied=boundary.armed)
        write_json(evidence / "result.json", sanitize_report(report, masked))
        print(evidence / "result.json")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
