"""Run the same workspace scenario through the public Session caller or installed terminal."""

import concurrent.futures
import contextlib
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import tempfile
from urllib.parse import urlencode, urlsplit

from evidence import comparison_path, inspect_sdk
from interactive_fixture import CONFIG, EXE, InstalledClient, Provider, http_fixture, serve
from interactive_intent import history, sanitize_report, save_screen, submit, wait_for_turn
from paid_model import REQUIRED_WORKSPACE_TOOLS, RequestBoundary, RunLimits, paid_budget, route_for
from run import (ROOT, capture, command_environment, config_record, extract_release_binary, load_secrets, now,
                 pty_artifact, redact, require_success, write_json)


REPORT = "incident-report.json"
RESTORED = "restored-report.json"
VALUE_NAME = "ingest_incidents"
PROGRAM = f"./ingest-replay{EXE}"
ARGS = ["--workspace-log"]
TURN_TIMEOUT = 150
EXPECTED = {
    "records": 8193,
    "errors": 4,
    "codes": {"E_DISK": 1, "E_SCHEMA": 2, "E_TIMEOUT": 1},
    "first": {"line": 23, "code": "E_SCHEMA", "component": "入口"},
    "last": {"line": 8193, "code": "E_TIMEOUT", "component": "worker"},
}
PROMPTS = (
    f"Analyze the ingestion replay. Run {PROGRAM} with the argument --workspace-log in this workspace. "
    "It prints newline-delimited JSON to stdout, with a log record per line. Its nonzero exit can indicate "
    "rejected records, so analyze the entire captured stdout. Write incident-report.json with the exact keys "
    "records (total records), errors (number with level ERROR), codes (counts keyed by code for ERROR only), "
    "first and last (first and last ERROR records, each with line, code, component only). "
    f"Save the same report as a named Session value {VALUE_NAME}. Keep the captured replay available.",
    f"Use the saved Session value {VALUE_NAME} to write its JSON report to {RESTORED}. "
    f"Keep {REPORT} intact and use the saved value without running the replay again.",
)


def select_provider(args, stack, bridge, report):
    if args.provider is None:
        provider = Provider(bridge)
        endpoint = stack.enter_context(http_fixture(provider.dispatch))
        config = CONFIG.read_text(encoding="utf-8").replace("MOCK_PORT", str(urlsplit(endpoint).port))
        return provider, config, {}, (), None

    credentials = load_secrets(args.secrets)
    key = credentials.get(f"KUKU_TEST_{args.provider.upper()}_API_KEY")
    if not key:
        raise ValueError("The selected provider needs its acceptance credential")
    budget = paid_budget(args.ledger, args.provider)
    model = args.model or ("openai/gpt-6.1-terra" if args.provider == "openrouter" else "deepseek-flash")
    route = route_for(args.provider, model=model, effort=args.effort or "high", upstream=args.upstream, key=key)
    limits = RunLimits(requests=args.max_requests, request_bytes=131072, output_tokens=args.max_output_tokens)
    token = secrets.token_hex(32)
    masked = (token, *(value for name, value in credentials.items() if name.endswith(("_KEY", "_TOKEN", "_SECRET"))))
    boundary = RequestBoundary(route, limits, key, token, workspace=True)
    endpoint = stack.enter_context(serve(boundary.handler()))
    report.update(**route.evidence(limits), ledger=str(budget.path), requests=boundary.attempts,
                  transport_rejections=boundary.rejections)
    return boundary, route.config(endpoint, limits), {"KUKU_INTENT_FIXTURE_TOKEN": token}, masked, budget


def mock_result(call, identity):
    row = next(block for message in reversed(call.request["messages"]) for block in message["content"]
               if isinstance(block, dict) and block.get("tool_use_id") == identity)
    content = row["content"]
    return content if isinstance(content, str) else "\n".join(block["text"] for block in content)


def report_from_chunk(chunk):
    if (not chunk.get("complete") or chunk.get("has_more") or chunk.get("capture_truncated")
            or chunk.get("cleanup_incomplete") or chunk.get("filtered")):
        return None
    try:
        return json.loads("".join(segment["text"] for segment in chunk["segments"]))
    except (KeyError, ValueError):
        return None


def drive_mock(provider, turn, evidence):
    call = provider.next(PROMPTS[turn])
    names = sorted(tool["name"] for tool in call.request["tools"])
    assert REQUIRED_WORKSPACE_TOOLS <= set(names), names
    evidence["offered_tools"] = names
    if turn == 0:
        call.tool("run_command", "ingest-replay", program=PROGRAM, args=ARGS, cwd=".", timeout=15,
                  brief="Run ingestion replay")
        call = provider.next("ingest-replay")
        result = mock_result(call, "ingest-replay")
        output_id = re.search(r"^Output: ([\w-]+)", result, re.MULTILINE)
        assert "command exited 7" in result and output_id, result
        output_id = json.dumps(output_id[1])
        source = f"""
let records = 0, errors = 0, first = null, last = null;
const codes = {{}};
for await (const {{text}} of workspace.output_lines({{id:{output_id}, stream:"stdout"}})) {{
    const row = JSON.parse(text);
    records++;
    if (row.level !== "ERROR") continue;
    errors++;
    const {{line, code, component}} = row;
    codes[code] = (codes[code] ?? 0) + 1;
    last = {{line, code, component}};
    first ??= last;
}}
const report = {{records, errors, codes, first, last}};
const write = await workspace.write_file({{path:{json.dumps(REPORT)}, content:JSON.stringify(report)+"\\n"}});
return {{report, status:write.status}};
"""
        call.tool("workspace_script", "analyze-replay", code=source)
        call = provider.next("analyze-replay")
        report = json.loads(mock_result(call, "analyze-replay"))
        assert report.get("report") == EXPECTED and report.get("status") == "ok", report
        call.tool("save_value", "name-report", name=VALUE_NAME, value=report["report"])
        provider.next("name-report").text("Saved the ingestion incident report.")
    else:
        call.tool("read_output", "restore-report", name=VALUE_NAME)
        call = provider.next("restore-report")
        content = json.loads(mock_result(call, "restore-report"))
        report = report_from_chunk(content)
        assert report == EXPECTED, content
        call.tool("write_file", "write-restored-report", path=RESTORED, content=json.dumps(report) + "\n")
        provider.next("write-restored-report").text("Restored the saved incident report.")


def turn_evidence(entries, turn):
    calls = {}
    for row in entries:
        if row.get("turn") != turn:
            continue
        identity = row.get("tool_call_id")
        if row["type"] == "tool_call":
            calls[identity] = {"call_id": identity, "name": row["tool"], "args": row.get("args", {}),
                               "status": None, "result": {}}
        elif row["type"] == "tool_result":
            call = calls[identity]
            call.update(status=row["status"], result=row.get("structured") or {},
                        summary=(row.get("summary") or "")[:440],
                        preview_bytes=len((row.get("model_content") or "").encode()))
    return list(calls.values())


def sdk_entries(path):
    entries = []
    with path.open("rb") as handle:
        for _ in range(10000):
            line = handle.readline(1048577)
            if not line:
                return entries
            if len(line) > 1048576:
                raise ValueError("Workspace evidence event exceeds 1 MiB")
            event = json.loads(line)
            if event.get("kind") in {"tool.call", "tool.result"} and event.get("conversation", "main") == "main":
                event["type"] = event["kind"].replace(".", "_")
                entries.append(event)
    raise ValueError("Workspace evidence has too many events")


def isolated_env(env, path):
    env["PATH"] = str(path)
    runtimes = ("node", "nodejs", "python", "python3", "py", "bash", "git")
    assert all(not shutil.which(name, path=env["PATH"]) for name in runtimes), "Unexpected tool on product PATH"
    return env


def run_sdk(args, scratch, workspace, config, environment, provider, evidence, report, budget):
    caller = args.sdk.resolve(strict=True)
    data = scratch / "sessions"
    env = {key: value for key, value in os.environ.items()
           if not key.upper().startswith("KUKU_") and not key.upper().endswith("_PROXY")}
    env.update(NO_PROXY="127.0.0.1,localhost", KUKU_HOME=str(scratch / "sdk-home"), **environment)
    env = isolated_env(env, scratch / "bin")
    session_id = "workspace-acceptance"
    report["input"] = config_record(caller)
    report["turns"] = turns = []
    help_check = capture([caller, "--help"], evidence, "sdk-help", timeout=30, env=env, cwd=scratch)
    report["sdk_help"] = help_check
    assert "--inspect-named-value" in Path(require_success(help_check)["log"]).read_text(encoding="utf-8")
    if isinstance(provider, RequestBoundary):
        provider.reserve(budget, report["run_id"], "Public SDK workspace execution")
    connection = provider.route.connection if isinstance(provider, RequestBoundary) else "test-anthropic"
    for index, prompt in enumerate(PROMPTS):
        label = ("analyze", "reopen")[index]
        result = evidence / (label + "-sdk.json")
        prompt_file = scratch / (label + ".txt")
        prompt_file.write_text(prompt, encoding="utf-8")
        command = [
            caller, "--workspace", workspace, "--prompt-file", prompt_file, "--config", config,
            "--data-dir", data, "--output", result, "--session-id", session_id, "--command-id", label,
            "--max-steps", str(args.max_requests), "--max-requests", str(args.max_requests),
            "--max-output-tokens", str(args.max_output_tokens), "--max-context-tokens", "131072",
            "--timeout", str(TURN_TIMEOUT), "--inspect-named-value", VALUE_NAME,
        ]
        turn = {"name": label}
        turns.append(turn)
        if isinstance(provider, RequestBoundary):
            provider.phase = label
            check = capture(command, evidence, label, timeout=TURN_TIMEOUT + 15, env=env, cwd=scratch,
                            secrets=environment.values())
        else:
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                execution = pool.submit(capture, command, evidence, label, TURN_TIMEOUT + 15, env, (), scratch)
                try:
                    drive_mock(provider, index, turn)
                except (AssertionError, ValueError) as error:
                    turn["fixture_error"] = str(error)[:1200]
                check = execution.result()
            if "fixture_error" not in turn:
                provider.assert_idle()
        turn["caller"] = check
        if not result.is_file():
            require_success(check)
            raise ValueError("The SDK caller did not produce its result")
        sdk = json.loads(result.read_text(encoding="utf-8"))
        snapshot = sdk.get("agent") or {}
        turn.update(session_id=sdk["session_id"], number=snapshot.get("turn"), usage=snapshot.get("usage"),
                    last_error=sdk.get("error") or snapshot.get("last_error"),
                    tools=turn_evidence(sdk_entries(Path(sdk["events_path"])), snapshot.get("turn")),
                    sdk_workspace=sdk["workspace"])
        if index == 0:
            checks = verify_analysis(turn, workspace, evidence)
            observed = sdk["workspace"]["outcomes"]
            checks["sdk_decoded_script_outcome"] = any(item["facts"]["kind"] == "workspace_script_result"
                                                       for item in observed)
        else:
            checks = verify_reopen(turns, workspace, evidence)
            saved = turns[0]["sdk_workspace"]["named_after"]
            checks["sdk_read_persisted_binding_before_turn"] = sdk["workspace"]["named_before"] == saved \
                and saved.get("complete") is True and saved.get("value") == EXPECTED
        checks["runtime_completed"] = sdk["session_id"] == session_id and sdk["runtime_completed"] \
            and sdk["cleanup"]["closed"]
        inspected = inspect_sdk(result, data, connection)
        turn["sdk_inspection"] = inspected
        checks["sdk_history_and_close_verified"] = inspected["passed"]
        if "fixture_error" in turn:
            checks["fixture_completed"] = False
        assert all(checks.values()), {"checks": checks, "last_error": turn["last_error"]}
        require_success(check)


def native_named_values(client, workspace, session):
    query = urlencode({"workspace": workspace, "agent": "main"})
    return client.request("GET", f"/sessions/{session}/values?{query}").as_json()["values"]


def run_native(args, scratch, workspace, config, environment, provider, evidence, report, bridge, masked, budget):
    artifact = (args.binary or args.archive).resolve()
    binary = scratch / "bin" / ("kuku" + EXE)
    if args.archive:
        extract_release_binary(artifact, binary)
    else:
        shutil.copy2(artifact, binary)
    client = InstalledClient(binary, bridge, scratch)
    if isinstance(provider, Provider):
        client.provider = provider
    isolated_env(client.env, binary.parent).update(environment)
    (client.home / "config.toml").write_text(config.read_text(encoding="utf-8"), encoding="utf-8")
    report["input"] = config_record(artifact)
    report["binary"] = config_record(binary)
    version, _ = client.cli(workspace, "--version")
    report["binary"]["version"] = version.strip()
    report["native_pty"] = "conpty" if os.name == "nt" else "unix-pty"
    report["turns"] = turns = []
    prior = 0
    try:
        for index, prompt in enumerate(PROMPTS):
            label = ("analyze", "reopen")[index]
            turn = {"name": label}
            turns.append(turn)
            with client.terminal(workspace) as terminal:
                terminal.request("resize", rows=68, cols=152)
                if index == 0:
                    terminal.expect("New session", state="Ready")
                    if isinstance(provider, RequestBoundary):
                        selected = client.request("GET", "/models/resolve").as_json()["selection"]
                        route = provider.route
                        assert (selected["model"], selected["connection"], selected["settings"]["reasoning_effort"]) == (
                            route.model, route.connection, route.effort
                        ), selected
                        provider.reserve(budget, report["run_id"], "Installed workspace execution")
                else:
                    terminal.resume(PROMPTS[0][:26])
                    turn["named_values_before"] = native_named_values(client, workspace, client.session(workspace))
                    assert turn["named_values_before"] == turns[0]["named_values_after"]
                turn["service_instance_id"] = client.connection()["instance_id"]
                if index > 0:
                    assert turn["service_instance_id"] != turns[0]["service_instance_id"]
                try:
                    if isinstance(provider, RequestBoundary):
                        provider.phase = label
                    submit(terminal, prompt, marker=("replay available.", "replay again.")[index])
                    if isinstance(provider, Provider):
                        drive_mock(provider, index, turn)
                    session_id, snapshot = wait_for_turn(client, workspace, prior, timeout=TURN_TIMEOUT)
                    prior = snapshot["turn"]
                    turn.update(session_id=session_id, number=prior, usage=snapshot["usage"], last_error=snapshot.get("last_error"),
                                elapsed_ms=snapshot.get("turn_elapsed_ms"),
                                tools=turn_evidence(history(client, workspace, session_id), prior),
                                named_values_after=native_named_values(client, workspace, session_id))
                    checks = verify_analysis(turn, workspace, evidence) if index == 0 else verify_reopen(turns, workspace, evidence)
                    checks["runtime_completed"] = snapshot["status"] == "idle"
                    assert all(checks.values()), {"checks": checks, "last_error": turn["last_error"]}
                    terminal.expect("Finished")
                finally:
                    save_screen(evidence, label, terminal.request("screen")["screen"], masked)
                    turn["screen"] = label + ".txt"
                terminal.exit_cleanly()
            client.stop()
            if isinstance(provider, Provider):
                provider.assert_idle()
    finally:
        client.stop()


def read_artifact(workspace, evidence, name):
    source = workspace / name
    if not source.is_file() or source.is_symlink() or source.stat().st_size > 65536:
        return None
    data = source.read_bytes()
    (evidence / name).write_bytes(data)
    try:
        return json.loads(data)
    except (UnicodeDecodeError, ValueError):
        return None


def tool_actions(tools):
    for tool in tools:
        yield tool
        for action in tool["result"].get("actions", []):
            yield {**action, "result": action["result"] or {}}


def read_all_captured_output(script, reference):
    pages = [action["result"] for action in script["result"].get("actions", [])
             if action["tool"] == "read_output" and action["status"] == "ok"
             and action["result"].get("output_id") == reference["id"]]
    offset = 0
    for page in sorted(pages, key=lambda item: item["offset"]):
        if (not page["complete"] or page["capture_truncated"] or page.get("cleanup_incomplete")
                or page.get("filtered") or page["offset"] > offset):
            return False
        offset = max(offset, page["next_offset"])
    return offset == reference["bytes"]


def is_replay(command):
    invocation = command.get("invocation", {})
    if invocation.get("mode") == "program":
        return Path(invocation["program"]).name == Path(PROGRAM).name and invocation.get("args") == ARGS
    if invocation.get("mode") == "shell":
        return Path(PROGRAM).name in invocation["command"] and ARGS[0] in invocation["command"]
    return False


def script_failures(tools):
    return [
        {"call_id": entry["call_id"], "status": entry["status"], "summary": entry.get("summary"),
         "completed_actions": [action["tool"] for action in entry["result"].get("actions", [])
                               if action["status"] == "ok"],
         "later_successful_calls": [later["name"] for later in tools[index + 1:] if later["status"] == "ok"]}
        for index, entry in enumerate(tools)
        if entry["name"] == "workspace_script" and entry["status"] not in (None, "ok")
    ]


def verify_analysis(turn, workspace, evidence):
    tools = turn["tools"]
    actions = list(tool_actions(tools))
    commands = [action["result"] for action in actions if action["result"].get("kind") == "command_result"]
    replay = [row for row in commands if row["invocation"]["mode"] == "program" and is_replay(row)]
    scripts = [tool for tool in tools if tool["name"] == "workspace_script"]
    named_reference = saved_report_reference(turn)
    named = [action["result"] for action in actions if action.get("name", action.get("tool")) == "save_value"
             and action["status"] == "ok" and action["result"].get("name") == VALUE_NAME]
    turn["script_failures"] = script_failures(tools)
    turn["checks"] = checks = {
        "report_matches_known_records": read_artifact(workspace, evidence, REPORT) == EXPECTED,
        "one_replay": len(replay) == 1,
        "typed_exit_and_capture": len(replay) == 1 and replay[0]["termination"] == "exited"
            and replay[0]["exit_code"] == 7 and replay[0]["capture_complete"]
            and comparison_path(Path(replay[0]["cwd"]).resolve()) == comparison_path(workspace.resolve())
            and replay[0]["stdout_lines"] == EXPECTED["records"] and replay[0]["stderr_lines"] == 1
            and replay[0]["output_ref"]["bytes"] == replay[0]["stdout_bytes"] + replay[0]["stderr_bytes"]
            and replay[0]["stdout_bytes"] > 500000 and replay[0]["preview_truncated"],
        "embedded_script_read_entire_capture": len(replay) == 1 and any(
            tool["result"]["kind"] == "workspace_script_result" and read_all_captured_output(tool, replay[0]["output_ref"])
            for tool in scripts
        ),
        "named_report_committed": named_reference is not None and any(
            value.get("reference") == named_reference for value in named
        ),
    }
    return checks


def saved_report_reference(turn):
    if "sdk_workspace" in turn:
        binding = (turn["sdk_workspace"]["named_after"] or {}).get("binding", {})
        return binding.get("reference") if binding.get("name") == VALUE_NAME else None
    return next((binding["reference"] for binding in turn.get("named_values_after", [])
                 if binding["name"] == VALUE_NAME), None)


def saved_report_reads(tools, reference):
    if reference is None:
        return []
    reads = []
    for tool in tools:
        result = tool["result"]
        direct = (tool["name"] == "read_output" and tool["status"] == "ok" and tool["args"].get("name") == VALUE_NAME
                  and result.get("output_id") == reference["id"] and report_from_chunk(result) == EXPECTED)
        scripted = tool["name"] == "workspace_script" and read_all_captured_output(tool, reference)
        if direct or scripted:
            reads.append({"tool": tool["name"], "call_id": tool["call_id"], "reference": reference})
    return reads


def verify_reopen(turns, workspace, evidence):
    first, reopened = turns
    actions = list(tool_actions(reopened["tools"]))
    reads = saved_report_reads(reopened["tools"], saved_report_reference(first))
    reopened["script_failures"] = script_failures(reopened["tools"])
    reopened["saved_report_reads"] = reads
    reopened["checks"] = checks = {
        "same_session_after_close": reopened["session_id"] == first["session_id"] and reopened["number"] is not None
            and reopened["number"] > first["number"],
        "used_saved_name": bool(reads),
        "no_replay": not any(is_replay(action["result"]) for action in actions),
        "saved_report_restored": read_artifact(workspace, evidence, RESTORED) == EXPECTED,
        "original_report_unchanged": (workspace / REPORT).is_file()
            and (workspace / REPORT).read_bytes() == (evidence / REPORT).read_bytes(),
    }
    return checks


def cleanup_path(path):
    if os.name != "nt":
        return path
    absolute = str(path)
    if absolute.startswith("\\\\?\\"):
        return absolute
    if absolute.startswith("\\\\"):
        return "\\\\?\\UNC\\" + absolute[2:]
    return "\\\\?\\" + absolute


@contextlib.contextmanager
def workspace_scratch(base, report):
    scratch = Path(tempfile.mkdtemp(prefix="kuku-workspace-", dir=base))
    failed = False
    try:
        yield scratch
    except BaseException:
        failed = True
        raise
    finally:
        try:
            shutil.rmtree(cleanup_path(scratch))
        except OSError as error:
            report["scratch_cleanup_error"] = f"{error.__class__.__name__}: {error}"
            if not failed:
                raise


def run_workspace(args, directory, record):
    if args.provider and (args.secrets is None or args.ledger is None):
        raise ValueError("A paid workspace scenario requires explicit --secrets and --ledger")
    if args.provider is None and any((args.model, args.effort, args.upstream, args.secrets, args.ledger)):
        raise ValueError("Select --provider to use model, route, effort, or credential options")
    if args.max_requests < 1 or args.max_output_tokens < 1:
        raise ValueError("Request and output token limits must be positive")
    base = (args.scratch_base or Path(tempfile.gettempdir())).resolve()
    if base.is_relative_to(ROOT):
        raise ValueError("Workspace acceptance requires scratch outside the source tree")
    base.mkdir(parents=True, exist_ok=True)
    if args.bridge:
        bridge = args.bridge.resolve(strict=True)
    else:
        compiled = capture(["cargo", "test", "--locked", "-p", "kuku-app", "--test", "interactive_pty",
                            "--no-run", "--message-format=json"], directory, "native-fixture-build",
                           timeout=1800, env=command_environment())
        record["fixture_build"] = compiled
        bridge = pty_artifact(require_success(compiled)["log"])
    report = {"run_id": record["run_id"], "started_at": now(), "passed": False, "paid": bool(args.provider),
              "mode": "sdk" if args.sdk else "installed_native", "isolated_product_path": True,
              "fixture": config_record(bridge), "expected_report": EXPECTED}
    masked = ()
    provider = None
    evidence = directory / "workspace"
    evidence.mkdir()
    try:
        with contextlib.ExitStack() as stack:
            scratch = stack.enter_context(workspace_scratch(base, report))
            (scratch / "bin").mkdir()
            workspace = scratch / "workspace 中文"
            workspace.mkdir()
            shutil.copy2(bridge, workspace / PROGRAM)
            provider, text, environment, masked, budget = select_provider(args, stack, bridge, report)
            config = scratch / "config.toml"
            config.write_text(text, encoding="utf-8")
            report["config"] = config_record(config)
            if args.sdk:
                run_sdk(args, scratch, workspace, config, environment, provider, evidence, report, budget)
            else:
                run_native(args, scratch, workspace, config, environment, provider, evidence, report, bridge, masked, budget)
            report["passed"] = True
    except Exception as error:
        report["error"] = type(error).__name__ + ": " + redact(str(error), masked)[:2400]
    finally:
        if isinstance(provider, RequestBoundary):
            report["reservation_applied"] = provider.armed
            try:
                report["upstream_confirmed"] = provider.confirm_upstream()
            except (ArithmeticError, KeyError, OSError, TypeError, ValueError):
                report["upstream_confirmed"] = None
            report["accounting"] = provider.accounting()
            report["passed"] = report["passed"] and bool(provider.attempts) and not provider.rejections \
                and report["upstream_confirmed"] is not False
        report["finished_at"] = now()
        result = evidence / "result.json"
        write_json(result, sanitize_report(report, masked))
        record["workspace_result"] = str(result)
        record["passed"] = report["passed"]
        if report.get("error"):
            record["error"] = report["error"]
