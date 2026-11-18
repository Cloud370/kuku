#!/usr/bin/env python3
"""Verify intended reply and workspace actions in native TUI sessions with a budgeted provider."""

import argparse
from dataclasses import dataclass
from decimal import Decimal
import hashlib
import http.client
from http.server import BaseHTTPRequestHandler
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import sys
import tempfile
import threading
import time
from urllib.parse import urlencode

from interactive_fixture import EXE, InstalledClient, eventually, install, serve
from run import Budget, ROOT, config_record, load_secrets, now, redact, write_json


MAX_REQUESTS = 36
MAX_BYTES = 64_000
MAX_OUTPUT = 2048
TURN_TIMEOUT = 125
RATE_UNIT = Decimal(1_000_000)
ARTIFACT_LIMIT = 64 * 1024
ROUTING_FIELDS = ("connection", "tier", "model", "reasoning_effort", "service_tier", "preset")


def upstream_json(host, path, key=None):
    connection = http.client.HTTPSConnection(host, timeout=12)
    try:
        headers = {"Accept": "application/json", "Cache-Control": "no-cache"}
        if key:
            headers["Authorization"] = "Bearer " + key
        connection.request("GET", path, headers=headers)
        response = connection.getresponse()
        if response.status != 200:
            return response.status, None
        return response.status, json.loads(response.read(262_144))["data"]
    finally:
        connection.close()


@dataclass(frozen=True)
class Route:
    connection: str
    host: str
    wire: str
    model: str
    upstream_model: str
    credential: str
    context: int
    input_rate: Decimal
    output_rate: Decimal
    price_source: str

    @property
    def router(self):
        return self.wire == "openai-responses"

    @property
    def path(self):
        return "/responses" if self.router else "/chat/completions"

    @property
    def reserve(self):
        return MAX_REQUESTS * (self.context * self.input_rate + MAX_OUTPUT * self.output_rate)

    def config(self, endpoint):
        prefix = 'default_model = "balanced"\n'
        for tier in ("balanced", "strong", "light"):
            prefix += f'''
[model.{tier}]
provider = "{self.connection}"
model = "{self.model}"
think = "low"
reasoning_effort = "low"
context_window = 32768
max_output_tokens = {MAX_OUTPUT}
'''
        template = f'''
[provider.{self.connection}]
format = "{self.wire}"
profile = "{'openrouter' if self.router else 'deepseek'}"
base_url = "{endpoint}"
api_key = "$KUKU_INTENT_FIXTURE_TOKEN"
'''
        if self.router:
            template += f'''
reasoning_efforts = ["low"]
[provider.{self.connection}.routing]
order = ["openai"]
only = ["openai"]
allow_fallbacks = false
require_parameters = true
[provider.{self.connection}.routing.max_price]
prompt = 0.20
completion = 0.75
'''
        return prefix + template


def route_for(name):
    if name == "deepseek":
        return Route("deepseek-chat", "api.deepseek.com", "openai-chat", "deepseek-flash", "DeepSeek-V4.1-Flash-1017",
                     "KUKU_TEST_DEEPSEEK_API_KEY", 1_000_000, Decimal("0.30") / RATE_UNIT, Decimal("1.20") / RATE_UNIT,
                     "https://api-docs.deepseek.com/quick_start/pricing/")
    model = "openai/gpt-6-luna"
    path = f"/api/v1/models/{model}/endpoints"
    status, data = upstream_json("openrouter.ai", path)
    if status != 200:
        raise RuntimeError(f"OpenRouter endpoint metadata returned HTTP {status}")
    standard, = [endpoint for endpoint in data["endpoints"] if endpoint["tag"] == "openai"]
    assert standard["provider_name"] == "OpenAI" and "reasoning_effort" in standard["supported_parameters"]
    pricing = standard["pricing"]
    tiers = [pricing, *pricing.get("overrides", [])]
    input_rate = max(Decimal(tier.get(field, "0")) for tier in tiers
                     for field in ("prompt", "input_cache_read", "input_cache_write"))
    output_rate = max(Decimal(tier["completion"]) for tier in tiers)
    return Route("openrouter-openai", "openrouter.ai", "openai-responses", model, standard["name"].split(" | ")[-1],
                 "KUKU_TEST_OPENROUTER_API_KEY", standard["context_length"], input_rate, output_rate,
                 "https://openrouter.ai" + path)


class RequestBoundary:
    def __init__(self, route, upstream_key, loopback_key):
        self.route = route
        self.key = upstream_key
        self.loopback_key = loopback_key
        self.lock = threading.Lock()
        self.phase = "preflight"
        self.attempts = []

    def checked_output(self, payload):
        route = self.route
        output = payload.get("max_output_tokens") if route.router else payload.get("max_tokens", payload.get("max_completion_tokens"))
        if (payload.get("model") != route.model or payload.get("stream") is not True
                or type(output) is not int or not 0 < output <= MAX_OUTPUT):
            return None
        if not all(tool.get("type") == "function" for tool in payload.get("tools", [])):
            return None
        if route.router:
            pin = payload.get("provider", {})
            ceiling = pin.get("max_price", {})
            if (pin.get("order") != ["openai"] or pin.get("only") != ["openai"]
                    or pin.get("allow_fallbacks") is not False or pin.get("require_parameters") is not True
                    or not 0 < ceiling.get("prompt", 0) <= 0.20 or not 0 < ceiling.get("completion", 0) <= 0.75
                    or payload.get("store") is not False or (payload.get("reasoning") or {}).get("effort") != "low"):
                return None
        elif payload.get("thinking") != {"type": "enabled"} or payload.get("reasoning_effort") != "low":
            return None
        return output

    def observe(self, attempt, line):
        if not line.startswith(b"data:") or not any(marker in line for marker in (b'"usage"', b'"tool_calls"', b'"response.output_item.done"')):
            return
        try:
            event = json.loads(line[5:])
            if self.route.router:
                if event.get("type") == "response.output_item.done" and event.get("item", {}).get("type") == "function_call":
                    attempt.setdefault("streamed_tool_names", []).append(event["item"]["name"])
                if event.get("type") != "response.completed":
                    return
                event = event["response"]
            else:
                for choice in event.get("choices", []):
                    for call in choice.get("delta", {}).get("tool_calls", []):
                        if name := call.get("function", {}).get("name"):
                            attempt.setdefault("streamed_tool_names", []).append(name)
            usage = event.get("usage")
            if not isinstance(usage, dict):
                return
            attempt["model_received"] = event.get("model")
            fields = (("input_tokens", "output_tokens", "cost") if self.route.router
                      else ("prompt_tokens", "completion_tokens", "prompt_cache_hit_tokens", "prompt_cache_miss_tokens"))
            attempt["usage"] = {name: usage[name] for name in fields if type(usage.get(name)) in (int, float)}
            if self.route.router:
                attempt["usage"]["cache_read_input_tokens"] = (usage.get("input_tokens_details") or {}).get("cached_tokens")
        except (KeyError, TypeError, ValueError):
            return

    def handler(self):
        boundary = self
        route = self.route

        class Forward(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                try:
                    length = int(self.headers.get("Content-Length", 0))
                    authorized = self.headers.get("Authorization") == "Bearer " + boundary.loopback_key
                    if self.path != route.path or not 0 < length <= MAX_BYTES or not authorized:
                        return self.send_error(400, "outside the acceptance transport boundary")
                    body = self.rfile.read(length)
                    payload = json.loads(body)
                    output = boundary.checked_output(payload)
                except (AttributeError, TypeError, ValueError):
                    return self.send_error(400, "invalid request for the acceptance transport")
                if output is None:
                    return self.send_error(400, "outside the selected model, route, or output limit")
                with boundary.lock:
                    if len(boundary.attempts) >= MAX_REQUESTS:
                        return self.send_error(429, "acceptance request limit reached")
                    attempt = {"case": boundary.phase, "request_bytes": length, "max_output_tokens": output}
                    if route.router:
                        create = next((tool for tool in payload.get("tools", []) if tool.get("name") == "create_agent"), None)
                        if create is not None:
                            attempt["create_agent_strict_disabled"] = create.get("strict") is False
                            attempt["create_agent_required_fields"] = create.get("parameters", {}).get("required", [])
                    boundary.attempts.append(attempt)
                connection = http.client.HTTPSConnection(route.host, timeout=75)
                started = time.monotonic()
                try:
                    path = ("/api/v1" if route.router else "") + route.path
                    connection.request("POST", path, body, headers={"Authorization": "Bearer " + boundary.key,
                                       "Content-Type": "application/json", "Accept": "text/event-stream"})
                    reply = connection.getresponse()
                    attempt["http_status"] = reply.status
                    if route.router:
                        attempt["generation_id"] = reply.getheader("X-Generation-Id")
                    self.send_response(reply.status)
                    self.send_header("Content-Type", reply.getheader("Content-Type", "application/json"))
                    self.end_headers()
                    while line := reply.readline(1_048_576):
                        boundary.observe(attempt, line)
                        try:
                            self.wfile.write(line)
                            self.wfile.flush()
                        except ConnectionError:
                            continue
                    attempt["response_ended"] = True
                except (OSError, http.client.HTTPException) as error:
                    attempt["transport_error"] = type(error).__name__
                finally:
                    attempt["seconds"] = round(time.monotonic() - started, 4)
                    connection.close()

        return Forward

    def accounting(self):
        attempts = self.attempts
        if not attempts or any(not row.get("response_ended") or row.get("http_status") != 200 for row in attempts):
            return {"complete": False}
        try:
            if self.route.router:
                costs = [Decimal(str(row["usage"]["cost"])) for row in attempts]
                if any(not cost.is_finite() or cost < 0 for cost in costs):
                    return {"complete": False}
                return {"complete": True, "basis": "provider_reported", "cost_usd": str(sum(costs))}
            cost = Decimal(0)
            for row in attempts:
                usage = row["usage"]
                hit = usage["prompt_cache_hit_tokens"]
                miss = usage["prompt_cache_miss_tokens"]
                if hit + miss != usage["prompt_tokens"] or not usage["prompt_tokens"]:
                    return {"complete": False}
                cost += (hit * Decimal("0.006") + miss * Decimal("0.30") + usage["completion_tokens"] * Decimal("1.20")) / RATE_UNIT
            return {"complete": True, "basis": "token_estimate", "cost_usd": str(cost), "rates": "DeepSeek peak"}
        except KeyError:
            return {"complete": False}

    def confirm_upstream(self):
        if not self.route.router:
            return bool(self.attempts) and all(row.get("http_status") == 200 and row.get("model_received") == self.route.model
                                               for row in self.attempts)
        for attempt in self.attempts:
            if not attempt.get("generation_id") or "cost" not in attempt.get("usage", {}):
                return False
            query = urlencode({"id": attempt["generation_id"]})
            data = None
            for delay in (0, 1, 2, 4, 8, 12, 15):
                if delay:
                    time.sleep(delay)
                status, data = upstream_json(self.route.host, "/api/v1/generation?" + query, self.key)
                if status == 200:
                    break
                if status != 404:
                    raise RuntimeError(f"Generation receipt returned HTTP {status}")
            if data is None:
                return False
            attempt["receipt"] = {field: data.get(field) for field in ("model", "provider_name", "service_tier", "total_cost")}
            if (data.get("id") != attempt["generation_id"] or data.get("model") != self.route.upstream_model
                    or data.get("provider_name") != "OpenAI" or data.get("service_tier") != "default"
                    or Decimal(str(data["total_cost"])) != Decimal(str(attempt["usage"]["cost"]))):
                return False
        return bool(self.attempts)


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


def submit(terminal, prompt):
    terminal.press("\x1b[200~" + prompt + "\x1b[201~")
    terminal.expect(prompt.splitlines()[0][:26])
    terminal.press("\r")


def wait_for_turn(client, workspace, previous):
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

    return eventually("the submitted native TUI turn to settle", settled, timeout=TURN_TIMEOUT)


def assistant_lines(screen):
    lines = [line.strip() for line in screen.splitlines()]
    start = max((index + 1 for index, line in enumerate(lines) if line == "Assistant"), default=len(lines))
    end = next((index for index in range(start, len(lines)) if lines[index].startswith("│") and "To main" in lines[index]), len(lines))
    body = [line for line in lines[start:end] if line and line != "│"]
    summary_pattern = (r"\d+(?:h \d{2}m \d{2}|m \d{2})?s"
                       r"(?: · \d+(?:\.\d+)?[KMB]? (?:in|out)| · Cache \d+(?:\.\d+)?%)* · \d+ tools?")
    if body and re.fullmatch(summary_pattern, body[-1]):
        body.pop()
    return body


def read_only_inspection(tool):
    if tool["name"] != "run_command":
        return False
    command = tool["args"].get("command", "").strip()
    return bool(re.fullmatch(r"(?:ls(?: +-[aAlh1]+)*(?: +\.)?|pwd|dir(?: +\.)?|Get-ChildItem(?: +\.)?)", command, re.IGNORECASE))


def tool_is_file_work(tool):
    return tool["name"] in {"read_file", "find_files", "search_text", "write_file", "edit_file"} or read_only_inspection(tool)


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
                      no_unrelated_tools=all(tool_is_file_work(tool) for tool in tools))
    save_screen(folder, name, screen, secrets_to_mask)
    return {"name": name, "passed": all(checks.values()), "checks": checks, "session_id": session,
            "turn": agent["turn"], "assistant": (agent.get("last_output") or "")[:1200],
            "before": before, "after": after, "tools": tools, "screen": f"{name}.txt",
            "read_only_shell": [tool["args"]["command"] for tool in tools if read_only_inspection(tool)]}


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
                and (selection.get("settings") or {}).get("reasoning_effort") == "low",
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


def run_selected_live(args, directory, record, scratch, bridge, checked):
    artifact = args.archive or args.binary
    record["input"] = {"type": "release_archive" if args.archive else "installed_binary", **config_record(artifact)}
    if args.intent:
        script, label, evidence, key = "interactive_intent.py", "installed-intent", directory / "intent", "intent_result"
        product = ["--archive" if args.archive else "--binary", str(artifact.resolve())]
        options = ["--scratch-base", str(scratch), "--provider", args.intent, "--case", args.intent_case, "--revision", args.revision]
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
    parser.add_argument("--provider", required=True, choices=("deepseek", "openrouter"))
    parser.add_argument("--case", default="all", choices=("all", "delivery", "delegation"))
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--revision", required=True)
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{7,40}", args.revision):
        parser.error("--revision must be a Git commit ID of at least seven hexadecimal characters")
    route = route_for(args.provider)
    credentials = load_secrets(args.secrets)
    upstream_key = credentials[route.credential]
    loopback_key = secrets.token_hex(32)
    masked = [loopback_key, *(value for name, value in credentials.items() if name.endswith(("_KEY", "_TOKEN", "_SECRET")))]
    boundary = RequestBoundary(route, upstream_key, loopback_key)
    budget = Budget(args.ledger.resolve())
    artifact = (args.archive or args.binary).resolve()
    evidence = args.evidence.resolve()
    if args.scratch_base and args.scratch_base.resolve().is_relative_to(ROOT):
        raise ValueError("Intent installation requires a scratch directory outside the source tree")
    assert args.bridge.is_file() and args.ledger.is_file()
    evidence.mkdir(parents=True)
    report = {"passed": False, "run_id": args.run_id, "started_at": now(), "revision": args.revision,
              "selected_case": args.case, "artifact": config_record(artifact), "bridge": config_record(args.bridge),
              "provider": route.host, "wire": route.wire, "model": route.model, "reasoning_effort": "low",
              "max_requests_including_retries": MAX_REQUESTS, "max_bytes": MAX_BYTES, "max_output_tokens": MAX_OUTPUT,
              "price_source": route.price_source, "reservation_usd": str(route.reserve), "requests": boundary.attempts,
              "cases": []}
    try:
        with tempfile.TemporaryDirectory(prefix="kuku-live-intent-", dir=args.scratch_base) as directory, serve(boundary.handler()) as endpoint:
            scratch = Path(directory)
            executable = scratch / "data" / "bin" / ("kuku" + EXE)
            client = InstalledClient(executable, args.bridge.resolve(), scratch)
            client.env.update(KUKU_INTENT_FIXTURE_TOKEN=loopback_key,
                              PATH=str(executable.parent) + os.pathsep + client.env["PATH"])
            if args.archive:
                install(artifact, client.home, client.env)
            else:
                executable.parent.mkdir()
                shutil.copy2(artifact, executable)
            (client.home / "config.toml").write_text(route.config(endpoint), encoding="utf-8")
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
                    assert selected["settings"]["reasoning_effort"] == "low" and not boundary.attempts
                    terminal.exit_cleanly()
                budget.reserve(args.run_id, float(route.reserve), f"Native intent and first delegation on {route.model}: "
                               f"up to {MAX_REQUESTS} requests at full provider context and {MAX_OUTPUT} output tokens each, including retries.")
                if args.case in {"all", "delivery"}:
                    delivery_and_revision(client, boundary, evidence, masked, report["cases"])
                if args.case in {"all", "delegation"}:
                    report["cases"].append(delegation(client, boundary, evidence, masked, route))
            finally:
                client.stop()
        report["upstream_confirmed"] = boundary.confirm_upstream()
        report["passed"] = report["upstream_confirmed"] and all(result["passed"] for result in report["cases"])
    except Exception as error:
        report["error"] = type(error).__name__ + ": " + redact(str(error), masked)[:2400]
    finally:
        report.update(finished_at=now(), accounting=boundary.accounting())
        write_json(evidence / "result.json", sanitize_report(report, masked))
        print(evidence / "result.json")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
