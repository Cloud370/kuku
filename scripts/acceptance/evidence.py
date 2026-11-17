"""Bounded, read-only checks of session_task reports against durable SDK facts."""

from collections import Counter
import hashlib
import json
import math
from pathlib import Path, PurePosixPath
import re


TOKEN_FIELDS = ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")
RESULT_FIELDS = ("tool_call_id", "status", "summary", "model_content", "structured", "truncated")
STREAM_COUNTERS = ("model_text_chunks", "model_thinking_chunks", "tool_output_chunks", "model_text_bytes",
                   "model_thinking_bytes", "tool_output_bytes", "model_request_events", "committed_events", "lagged_updates")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def fingerprint(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"),
                                     ensure_ascii=False).encode()).hexdigest()


def under(directory, path):
    resolved = path.resolve()
    require(resolved.is_relative_to(directory.resolve()), "evidence path escapes the Session data directory")
    return resolved


def json_file(path):
    require(path.stat().st_size <= 4 * 1024 * 1024, "SDK report exceeds the evidence read limit")
    return json.loads(path.read_text(encoding="utf-8"))


def output_reference(events_path, structured):
    output_id = structured.get("output_id")
    require(isinstance(output_id, str) and re.fullmatch(r"[A-Za-z0-9_-]{1,128}", output_id),
            "command result has an invalid output identity")
    reference = structured.get("output_ref", {})
    require(reference.get("id") == output_id and reference.get("offset") == 0,
            "command output reference does not match its retained identity")
    path = under(events_path.parent, events_path.parent / "outputs" / f"{output_id}.output")
    digest, retained = hashlib.sha256(), 0
    with path.open("rb") as handle:
        header = handle.read(4096)
        digest.update(header)
        require(len(header) == 4096 and header[:8] == b"KUKUOUT1", "command output header is incomplete or unsupported")
        length = int.from_bytes(header[8:12], "little")
        require(0 < length <= 4084, "command output metadata length is invalid")
        metadata = json.loads(header[12:12 + length])
        require(metadata.get("complete") is True, "retained command output was not finalized")
        require(metadata.get("cleanup_incomplete") is False, "retained command output reports incomplete cleanup")
        while frame := handle.read(5):
            digest.update(frame)
            require(len(frame) == 5 and frame[0] in (1, 2), "command output has an invalid frame header")
            size = int.from_bytes(frame[1:], "little")
            require(0 < size <= 8192, "command output frame exceeds its bounded format")
            payload = handle.read(size)
            require(len(payload) == size, "command output has an incomplete frame")
            digest.update(payload)
            retained += size
    require(retained == metadata.get("stored_bytes") == reference.get("bytes"),
            "command output reference byte count differs from retained frames")
    for event_field, metadata_field in (("stdout_bytes", "stdout_bytes"), ("stderr_bytes", "stderr_bytes"),
                                        ("quota_truncated", "quota_truncated"), ("cleanup_incomplete", "cleanup_incomplete")):
        require(structured.get(event_field) == metadata.get(metadata_field),
                f"command result {event_field} differs from retained metadata")
    require(structured.get("capture_complete") == (not metadata.get("quota_truncated")),
            "command output capture status differs from retained metadata")
    return {"id": output_id, "path": str(path), "sha256": digest.hexdigest(),
            "stored_bytes": retained, "complete": True, "quota_truncated": metadata["quota_truncated"]}


def usage_summary(requests, usages, reports, session_scope=False):
    partial = {}
    complete = {}
    for field in TOKEN_FIELDS:
        values = [(usages.get(request) or {}).get(field) for request in requests]
        require(all(value is None or type(value) is int and value >= 0 for value in values),
                f"provider usage contains invalid {field}")
        known = [value for value in values if value is not None]
        partial[field] = sum(known) if known else None
        complete[field] = sum(known) if requests and len(known) == len(requests) else None
    missing = sum(any((usages.get(request) or {}).get(field) is None
                      for field in ("input_tokens", "output_tokens")) for request in requests)
    zero_input = sum((usages.get(request) or {}).get("input_tokens") == 0 for request in requests)
    if all(complete[field] is not None for field in TOKEN_FIELDS if field != "output_tokens"):
        require(complete["cache_read_input_tokens"] + complete["cache_creation_input_tokens"] <= complete["input_tokens"],
                "reported cached input exceeds total input tokens")
    costs = {}
    providers = set()
    for event in reports:
        report = event.get("report", {})
        if report.get("provider"):
            providers.add(report["provider"])
        cost = report.get("cost_usd")
        if cost is not None:
            require(isinstance(cost, (float, int)) and not isinstance(cost, bool)
                    and math.isfinite(cost) and cost >= 0, "provider report contains an invalid cost")
            key = (event["turn"], event["request_id"])
            if session_scope:
                key = (event.get("agent", "main"), *key)
            costs[key] = cost
    complete_cost = missing == 0 and zero_input == 0 and bool(requests) and all(request in costs for request in requests)
    return {"requests": len(requests), "unreported_requests": missing,
            "zero_input_requests": zero_input, "complete": bool(requests) and missing == 0 and zero_input == 0, **complete,
            "reported_partial": partial,
            "uncached_input_tokens": (complete["input_tokens"] - complete["cache_read_input_tokens"]
                                      - complete["cache_creation_input_tokens"])
            if all(complete[field] is not None for field in TOKEN_FIELDS if field != "output_tokens") else None,
            "provider_reported_cost_usd": sum(costs.values()) if complete_cost else None,
            "provider_reported_cost_partial_usd": sum(costs.values()) if costs else None,
            "reported_providers": sorted(providers), "cost_ledger_basis": "conservative_upper_bound"}


def streaming_summary(report, requests, records, text_bytes, thinking_bytes):
    source = report.get("streaming")
    if source is None:
        return {"reported": False, "coverage_complete": False}
    require(isinstance(source, dict), "SDK streaming statistics are not an object")
    counters = {field: source.get(field) for field in STREAM_COUNTERS}
    require(all(type(value) is int and value >= 0 for value in counters.values()),
            "SDK streaming statistics contain missing or invalid counters")
    elapsed = report.get("elapsed_ms")
    require(type(elapsed) is int and elapsed >= 0, "streaming statistics have no valid elapsed duration")
    for field, count in (("first_model_text_ms", "model_text_chunks"),
                         ("first_model_thinking_ms", "model_thinking_chunks"),
                         ("first_tool_output_ms", "tool_output_chunks"),
                         ("first_model_request_ms", "model_request_events")):
        value = source.get(field)
        require((counters[count] == 0 and value is None) or
                (counters[count] > 0 and type(value) is int and 0 <= value <= elapsed),
                "streaming first-observation time differs from its counter or elapsed duration")
        counters[field] = value
    require(counters["model_request_events"] <= requests and counters["committed_events"] <= records,
            "SDK observed more model requests or committed events than exist in durable facts")
    require(counters["model_text_bytes"] <= text_bytes and counters["model_thinking_bytes"] <= thinking_bytes,
            "SDK streaming model byte counts exceed durable responses")
    require((counters["model_text_bytes"] == 0 or counters["model_text_chunks"] > 0)
            and (counters["model_thinking_bytes"] == 0 or counters["model_thinking_chunks"] > 0)
            and (counters["tool_output_bytes"] == 0 or counters["tool_output_chunks"] > 0),
            "SDK streaming bytes have no corresponding observed chunks")
    return {"reported": True, "validated": True,
            "scope": "caller-observed transient updates; durable facts validate count/byte bounds, not replay timing",
            "coverage_complete": counters["model_request_events"] > 0 and counters["lagged_updates"] == 0
            and counters["model_text_bytes"] + counters["model_thinking_bytes"] > 0, **counters}


class SessionFacts:
    def __init__(self, agent, events_path, connection):
        self.agent, self.events_path, self.connection = agent, events_path, connection
        self.counts = Counter()
        self.steps, self.responses, self.usages = {}, {}, {}
        self.session_steps, self.session_usages, self.session_reports = {}, {}, []
        self.calls, self.results, self.tool_prepared, self.dispatched = {}, {}, {}, set()
        self.segments, self.admitted_results = {}, set()
        self.context_used, self.context_pending = {}, {}
        self.settings, self.reports, self.outputs = [], [], []
        self.report_hashes = []
        self.settled, self.pending, self.created = None, set(), set()
        self.session_id, self.closed_at, self.last_id = None, 0, 0
        self.cleanup_failures = 0
        self.records, self.text_bytes, self.thinking_bytes = 0, 0, 0

    def accept(self, event):
        identity = event.get("id")
        require(type(identity) is int and identity > self.last_id, "event cursor is not strictly increasing")
        self.last_id = identity
        self.records += 1
        kind = event.get("kind")
        if kind == "session.created":
            require(self.session_id is None, "Session contains duplicate creation facts")
            self.session_id = event.get("session_id")
        if kind == "runtime.operation":
            self.operation(event)
        owner = event.get("agent", event.get("conversation") or "main")
        require(isinstance(owner, str) and len(owner) <= 64, "event Agent identity exceeds the diagnostic limit")
        request = (event.get("turn"), event.get("request_id"))
        session_request = (owner, *request)
        if kind == "runtime.step_prepared":
            require(isinstance(request[1], str) and len(request[1]) <= 128 and len(self.session_steps) < 4096,
                    "prepared requests exceed the diagnostic identity/count limit")
            require(session_request not in self.session_steps, "Session contains duplicate prepared request identities")
            metadata = {field: event.get(field) for field in ("id", "agent", "turn", "connection", "model")}
            require(all(isinstance(metadata[field], str) and len(metadata[field]) <= 512 for field in ("connection", "model")),
                    "prepared request routing metadata is invalid or oversized")
            self.session_steps[session_request] = metadata
        elif kind in ("runtime.model_usage", "runtime.provider_report"):
            require(session_request in self.session_steps, "Session usage or report has no prepared request")
            if kind == "runtime.model_usage":
                require(session_request not in self.session_usages, "Session contains duplicate usage facts")
                usage = event.get("usage")
                require(usage is None or isinstance(usage, dict), "invalid normalized usage record")
                usage = {field: usage.get(field) for field in TOKEN_FIELDS} if usage is not None else None
                require(usage is None or all(value is None or type(value) is int and value >= 0 for value in usage.values()),
                        "invalid normalized token count")
                self.session_usages[session_request] = usage
            else:
                require(len(self.session_reports) < 8192, "provider reports exceed the diagnostic count limit")
                report = event.get("report", {})
                require(report.get("provider") is None or isinstance(report["provider"], str) and len(report["provider"]) <= 512,
                        "provider name is invalid or oversized")
                self.session_reports.append({"agent": owner, "turn": event.get("turn"), "request_id": request[1],
                                             "report": {field: report.get(field) for field in ("cost_usd", "provider")}})
        if owner != self.agent:
            return
        key = (event.get("turn"), event.get("tool_call_id"))
        if kind == "runtime.context_segment":
            self.segments[identity] = 0
            self.context_used[identity], self.context_pending[identity] = set(), {}
        elif kind == "runtime.context_admitted":
            segment = event.get("segment")
            require(segment in self.segments and event.get("ordinal") == self.segments[segment],
                    "admitted context has a missing segment or nonsequential ordinal")
            self.segments[segment] += 1
            self.admit(event.get("message", {}), segment)
        elif kind == "runtime.step_prepared":
            require(isinstance(request[1], str) and request not in self.steps, "prepared request identity is missing or duplicated")
            require(event.get("segment") in self.segments
                    and event.get("context_head") == self.segments[event["segment"]],
                    "prepared request does not reference its exact admitted context head")
            require(not self.context_pending[event["segment"]], "prepared context has an unpaired admitted tool use")
            require(isinstance(event.get("body_hash"), str) and re.fullmatch(r"[a-f0-9]{64}", event["body_hash"]),
                    "prepared request has no valid body fingerprint")
            require(not self.connection or event.get("connection") == self.connection,
                    "prepared request used a different connection")
            self.steps[request] = self.session_steps[session_request]
            self.settings.append(fingerprint(event))
            self.counts["prepared_steps"] += 1
        elif kind in ("model.response", "runtime.model_usage", "runtime.provider_report"):
            require(request in self.steps and event.get("turn") == self.steps[request].get("turn"),
                    "model response, usage or report has no matching prepared request")
            if kind == "model.response":
                require(request not in self.responses, "request has duplicate model responses")
                self.responses[request] = event["id"]
                self.counts["model_responses"] += 1
                self.text_bytes += len((event.get("text") or "").encode())
                self.thinking_bytes += len((event.get("thinking") or "").encode())
            elif kind == "runtime.model_usage":
                require(request not in self.usages, "request has duplicate normalized usage records")
                require(event.get("usage") is None or isinstance(event["usage"], dict), "invalid normalized usage record")
                self.usages[request] = self.session_usages[session_request]
            else:
                self.reports.append(self.session_reports[-1])
                self.report_hashes.append(fingerprint(event))
        elif kind == "tool.call":
            require(isinstance(key[1], str) and len(key[1]) <= 128 and len(self.calls) < 8192,
                    "tool calls exceed the diagnostic identity/count limit")
            require(isinstance(event.get("tool"), str) and len(event["tool"]) <= 128, "tool name is invalid or oversized")
            require(request in self.responses and key not in self.calls, "tool call has no model response or is duplicated")
            require(event.get("turn") == self.steps[request].get("turn"), "tool call belongs to another request turn")
            self.calls[key] = {"tool_call_id": event.get("tool_call_id"), "tool": event.get("tool"),
                               "args_hash": fingerprint(event.get("args"))}
            self.counts["tool_calls"] += 1
        elif kind == "runtime.tool_prepared":
            require(key in self.calls and key not in self.tool_prepared, "tool preparation has no unique model call")
            require(event.get("definition", {}).get("schema", {}).get("name") == self.calls[key].get("tool"),
                    "prepared tool definition differs from the model call")
            self.tool_prepared[key] = True
        elif kind == "runtime.tool_dispatched":
            require(key in self.tool_prepared and key not in self.dispatched, "tool dispatch has no unique preparation")
            self.dispatched.add(key)
        elif kind == "tool.result":
            require(key in self.calls and key not in self.results, "tool result has no unique matching call")
            self.results[key] = fingerprint({field: event.get(field) for field in RESULT_FIELDS})
            self.counts["tool_results"] += 1
            structured = event.get("structured") or {}
            require(structured.get("cleanup_incomplete") is not True, "tool result reports incomplete cleanup")
            if structured.get("kind") == "command_result":
                self.outputs.append(output_reference(self.events_path, structured))
        elif kind == "turn.completed":
            self.counts["turns_completed"] += 1

    def operation(self, event):
        record = event.get("record", {})
        operation = record.get("operation")
        if operation == "session_closed":
            self.closed_at = event["id"]
        elif operation == "cleanup_failed":
            self.cleanup_failures += 1
        elif operation == "agent_created":
            self.created.add(record.get("address"))
        if record.get("address") == self.agent:
            if operation == "inputs_accepted":
                self.pending.difference_update(record.get("input_ids", []))
            elif operation == "turn_settled":
                self.settled = (event["id"], record)
        if operation == "input_delivered" and record.get("input", {}).get("target") == self.agent:
            self.pending.add(record["input"]["command_id"])

    def admit(self, message, segment):
        used, pending = self.context_used[segment], self.context_pending[segment]
        for block in message.get("blocks", []):
            if "ToolUse" in block:
                use = block["ToolUse"]
                candidates = [key for key, call in self.calls.items() if key not in used
                              and call["tool_call_id"] == use.get("id") and call["tool"] == use.get("name")
                              and call["args_hash"] == fingerprint(use.get("args"))]
                require(candidates and use.get("id") not in pending, "admitted tool use differs from its durable model call")
                pending[use["id"]] = candidates
            if "ToolResult" in block:
                result = block["ToolResult"]
                digest = fingerprint({field: result.get(field) for field in RESULT_FIELDS})
                matches = [key for key in pending.get(result.get("tool_call_id"), []) if self.results.get(key) == digest]
                require(matches, "admitted tool result differs from its exact durable first view")
                used.add(matches[0])
                self.admitted_results.add(matches[0])
                pending.pop(result["tool_call_id"])

    def finish(self, report):
        snapshot = report.get("agent") or {}
        require(report.get("session_id") == self.session_id, "SDK report names a different durable Session")
        require(report.get("last_event_id") == self.last_id, "SDK event cursor differs from the durable log")
        require(report.get("runtime_completed") is True and report.get("error") is None,
                "SDK runtime did not complete successfully")
        require(report.get("cleanup", {}).get("closed") is True and self.cleanup_failures == 0,
                "SDK cleanup failed or the durable log reports incomplete cleanup")
        require(self.settled and self.closed_at > self.settled[0], "Session close has no durable completion barrier")
        require(snapshot.get("status") == "idle" and snapshot.get("pending_inputs") == len(self.pending) == 0,
                "SDK snapshot still has active or pending work")
        require(self.settled[1].get("status") == "idle" and self.settled[1].get("error") is None
                and self.settled[1].get("output") == snapshot.get("last_output"),
                "SDK snapshot differs from the durable turn settlement")
        require(all(self.counts[field] > 0 for field in
                    ("tool_calls", "tool_results", "model_responses", "prepared_steps", "turns_completed")),
                "SDK task has no completed real model/tool execution")
        require(all(report.get("facts", {}).get(field, 0) == self.counts[field] for field in
                    ("tool_calls", "tool_results", "model_responses", "prepared_steps", "turns_completed")),
                "SDK fact counters differ from the durable log")
        require([fingerprint(value) for value in report.get("provider_settings", [])] == self.settings
                and [fingerprint(value) for value in report.get("provider_reports", [])] == self.report_hashes,
                "SDK provider settings or reports differ from durable facts")
        require(self.calls.keys() == self.results.keys() and bool(self.dispatched), "SDK tool calls are not fully paired or dispatched")
        require(self.results.keys() <= self.admitted_results, "completed tool results were not admitted for model continuation")
        require(self.steps.keys() == self.responses.keys() == self.usages.keys(),
                "prepared requests do not have paired model response and usage facts")
        limits = report.get("limits", {})
        require(type(limits.get("max_steps")) is int and limits["max_steps"] > 0
                and type(limits.get("max_request_bytes")) is int and limits["max_request_bytes"] >= 4096
                and type(limits.get("max_output_tokens")) is int and limits["max_output_tokens"] > 0
                and limits.get("automatic_retries") == 0, "SDK report has no explicit bounded request/retry limits")
        require(all(count <= limits["max_steps"] for count in Counter(step.get("turn") for step in self.steps.values()).values()),
                "durable model requests exceeded the declared per-turn limit")
        usage = usage_summary(self.steps, self.usages, self.reports)
        normalized = snapshot.get("usage", {})
        require(normalized.get("model_requests") == len(self.usages)
                and normalized.get("unreported_requests") == usage["unreported_requests"],
                "SDK usage request counters differ from durable usage")
        require(all(normalized.get(field) == usage["reported_partial"][field] for field in TOKEN_FIELDS),
                "SDK token totals differ from durable normalized usage")
        return {"passed": True, "agent": self.agent, "session_id": self.session_id,
                "facts": dict(self.counts), "last_event_id": self.last_id, "cleanup_closed": True,
                "output_references": self.outputs, "usage": usage, "usage_scope": "selected_agent",
                "session_usage": usage_summary(self.session_steps, self.session_usages, self.session_reports, True),
                "other_agents": sorted(self.created - {self.agent}), "limits": limits,
                "streaming": streaming_summary(report, len(self.steps), self.records, self.text_bytes, self.thinking_bytes),
                "models": sorted({step["model"] for step in self.steps.values()}),
                "connections": sorted({step["connection"] for step in self.steps.values()})}


def inspect_sdk(result_path, data_dir, connection=None, container_data=None, session_id=None, require_streaming=False):
    inspected = {"passed": False, "result_path": str(result_path), "data_dir": str(data_dir)}
    try:
        report = json_file(result_path)
        inspected["runtime_completed"] = report.get("runtime_completed")
        inspected["cleanup_closed"] = report.get("cleanup", {}).get("closed")
        reported_path = report.get("events_path")
        require(isinstance(reported_path, str), "SDK report has no durable events path")
        if container_data:
            relative = PurePosixPath(reported_path).relative_to(PurePosixPath(container_data))
            require(".." not in relative.parts, "SDK events path escapes its container data mount")
            path = under(data_dir, data_dir.joinpath(*relative.parts))
        else:
            path = under(data_dir, Path(reported_path))
        require(not session_id or report.get("session_id") == session_id, "SDK report names a different requested Session")
        agent = (report.get("agent") or {}).get("address")
        require(isinstance(agent, str) and agent, "SDK report has no Agent snapshot")
        require(path.name == "events.jsonl" and path.parent.name == report.get("session_id"),
                "events path does not belong to the reported Session")
        facts, digest, records = SessionFacts(agent, path, connection), hashlib.sha256(), 0
        with path.open("rb") as handle:
            while line := handle.readline(1024 * 1024 + 1):
                require(len(line) <= 1024 * 1024 and line.endswith(b"\n"), "event log has an oversized record or incomplete tail")
                digest.update(line)
                if line.strip():
                    records += 1
                    require(records <= 100_000, "event log exceeds the diagnostic record limit")
                    facts.accept(json.loads(line))
        inspected.update(facts.finish(report), events_path=str(path), events_sha256=digest.hexdigest(), records=records)
        if require_streaming:
            require(inspected["streaming"]["coverage_complete"], "required SDK streaming coverage is missing, empty or lagged")
    except (OSError, ValueError, TypeError, KeyError, AttributeError) as error:
        inspected["passed"] = False
        inspected["error"] = str(error)
    return inspected
