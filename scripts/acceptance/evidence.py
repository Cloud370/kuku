"""Bounded evidence capture and read-only checks against durable SDK facts."""

from collections import Counter
import hashlib
import json
import math
from pathlib import Path, PurePosixPath, PureWindowsPath
import re


TOKEN_FIELDS = ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")
RESULT_VIEW_FIELDS = ("tool_call_id", "status", "summary", "model_content", "truncated")
STREAM_COUNTERS = ("model_text_chunks", "model_thinking_chunks", "tool_output_chunks", "model_text_bytes",
                   "model_thinking_bytes", "tool_output_bytes", "model_request_events", "committed_events", "lagged_updates")
MAX_STAGED_TOOL_BLOCKS = 16_384
EVENT_LINE_BYTES = 1024 * 1024
EVENT_RECORD_LIMIT = 100_000


class SessionCapacityEvidence:
    def __init__(self, agent):
        self.agent = agent
        self.pending = {}
        self.committed = 0
        self.pages = 0

    def observe(self, event):
        if event.get("agent") != self.agent:
            return
        kind = event.get("kind")
        if kind == "runtime.context_segment" and event.get("reason") == "request_byte_budget":
            identity = event.get("id")
            if type(identity) is int and identity > 0:
                self.pending[identity] = event.get("output_pages_referenced")
        segment = event.get("segment")
        if kind == "runtime.step_prepared" and type(segment) is int and segment in self.pending:
            self.committed += 1
            pages = self.pending.pop(segment)
            if self.pages is not None:
                self.pages = self.pages + pages if type(pages) is int and pages > 0 else None

    def summary(self):
        return {"committed_rollovers": self.committed, "output_pages_referenced": self.pages}


class FailureSummary:
    LIMIT = 16_384
    LINE_LIMIT = 4096
    HEADERS = (b"Test failure summaries\n", b"\nPanic locations and bounded context\n")

    def __init__(self):
        self.partial = b""
        self.skipping_line = False
        self.summaries = bytearray()
        self.details = bytearray()
        self.capacity = (self.LIMIT - sum(map(len, self.HEADERS))) // 2
        self.context = 0
        self.target_list = False
        self.truncated = False

    def feed(self, chunk):
        if self.skipping_line:
            end = chunk.find(b"\n")
            if end < 0:
                return
            chunk = chunk[end + 1:]
            self.skipping_line = False
        pending = self.partial + chunk
        lines = pending.split(b"\n")
        self.partial = lines.pop()
        for line in lines:
            if len(line) <= self.LINE_LIMIT:
                self.line(line.rstrip(b"\r"))
            else:
                self.truncated = True
        if len(self.partial) > self.LINE_LIMIT:
            self.partial = b""
            self.skipping_line = True
            self.truncated = True

    def append(self, target, line):
        if len(target) + len(line) + 1 <= self.capacity:
            target.extend(line + b"\n")
        else:
            self.truncated = True

    def line(self, line):
        line = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", line)
        marker = (line.startswith(b"test ") and b"... FAILED" in line
                  or line.startswith((b"test result: FAILED", b"error:", b"error[")))
        if marker or self.target_list and line.startswith(b"    "):
            self.append(self.summaries, line)
        if line.startswith(b"error:") and b"target" in line and b"failed" in line:
            self.target_list = True
        elif line.strip() and not line.startswith(b"    "):
            self.target_list = False
        if line.startswith(b"thread ") and b" panicked at " in line:
            self.append(self.details, line)
            self.context = 3
        elif self.context:
            self.append(self.details, line)
            self.context -= 1

    def write(self, directory, label, redact):
        if self.partial:
            self.line(self.partial.rstrip(b"\r"))
            self.partial = b""
        output = b""
        if self.summaries or self.details:
            raw = self.HEADERS[0] + self.summaries + self.HEADERS[1] + self.details
            output = redact(raw.decode("utf-8", errors="replace")).encode()
            self.truncated |= len(output) > self.LIMIT
            output = output[:self.LIMIT].decode("utf-8", errors="ignore").encode()
        path = directory / f"{label}-failures.log"
        if output:
            path.write_bytes(output)
        else:
            path.unlink(missing_ok=True)
        return {"failure_summary": {"log": str(path) if output else None,
                                    "retained_bytes": path.stat().st_size if output else 0,
                                    "limit_bytes": self.LIMIT,
                                    "truncated": self.truncated}}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def fingerprint(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"),
                                     ensure_ascii=False).encode()).hexdigest()


def comparison_path(path):
    if not isinstance(path, PureWindowsPath):
        return path
    root = path.anchor
    if root.lower().startswith("\\\\?\\unc\\"):
        return PureWindowsPath("\\\\" + root[8:], *path.parts[1:])
    if root.startswith("\\\\?\\") and re.fullmatch(r"[A-Za-z]:\\", root[4:]):
        return PureWindowsPath(root[4:], *path.parts[1:])
    return path


def under(directory, path):
    resolved, base = path.resolve(), directory.resolve()
    require(comparison_path(resolved).is_relative_to(comparison_path(base)),
            "evidence path escapes the Session data directory")
    return resolved


def session_journal(live, sdk):
    try:
        root = live.resolve() / 'session-data'
        if root.resolve() != root:
            return None
        if 'events_path' in sdk:
            candidate = recorded_session_journal(root, sdk)
        else:
            candidates = root.glob('p/*/sessions/*/events.jsonl')
            candidate = next(candidates, None)
            if candidate is None or next(candidates, None) is not None:
                return None
        return candidate if candidate.resolve() == candidate and candidate.is_file() else None
    except (OSError, TypeError, ValueError, RuntimeError):
        return None


def recorded_session_journal(root, sdk):
    reported = sdk['events_path']
    path_type = PureWindowsPath if PureWindowsPath(reported).is_absolute() else PurePosixPath
    reported = comparison_path(path_type(reported))
    current = comparison_path(path_type(str(root)))
    recorded_root = comparison_path(path_type(sdk.get('data_dir', str(root))))
    require(reported.is_absolute() and '..' not in reported.parts, 'invalid recorded Session path')
    base = recorded_root if recorded_root.is_absolute() else current
    parts = reported.relative_to(base).parts
    require(len(parts) >= 5 and parts[0] == 'p'
            and parts[-3:] == ('sessions', sdk.get('session_id'), 'events.jsonl'),
            'journal does not belong to the recorded Session')
    if base != current:
        digest = sdk.get('events_sha256')
        require(isinstance(digest, str) and re.fullmatch(r'[a-f0-9]{64}', digest),
                'relocated Session requires a recorded journal digest')
    candidate = root.joinpath(*parts)
    require(candidate.relative_to(root).parts == parts, 'invalid retained Session location')
    return candidate


def session_events(journal, expected_sha256=None):
    digest = hashlib.sha256() if expected_sha256 is not None else None
    with journal.open('rb') as source:
        for index, line in enumerate(iter(lambda: source.readline(EVENT_LINE_BYTES + 1), b''), 1):
            require(index <= EVENT_RECORD_LIMIT and len(line) <= EVENT_LINE_BYTES and line.endswith(b'\n'),
                    'saved Session journal exceeds its read bounds or has an incomplete tail')
            if digest is not None:
                digest.update(line)
            event = json.loads(line)
            require(isinstance(event, dict), 'saved Session journal contains a non-object event')
            yield event
    require(digest is None or digest.hexdigest() == expected_sha256,
            'saved Session journal differs from its SDK evidence digest')


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
    retries = sum(step.get("retry_count", 0) for step in requests.values())
    attempts = len(requests) + retries
    partial = {}
    complete = {}
    for field in TOKEN_FIELDS:
        values = [(usages.get(request) or {}).get(field) for request in requests]
        require(all(value is None or type(value) is int and value >= 0 for value in values),
                f"provider usage contains invalid {field}")
        known = [value for value in values if value is not None]
        partial[field] = sum(known) if known else None
        complete[field] = sum(known) if requests and len(known) == len(requests) else None
    missing = retries + sum(any((usages.get(request) or {}).get(field) is None
                      for field in ("input_tokens", "output_tokens")) for request in requests)
    zero_input = sum((usages.get(request) or {}).get("input_tokens") == 0 for request in requests)
    if all(complete[field] is not None for field in TOKEN_FIELDS if field != "output_tokens"):
        require(complete["cache_read_input_tokens"] + complete["cache_creation_input_tokens"] <= complete["input_tokens"],
                "reported cached input exceeds total input tokens")
    if retries:
        complete = dict.fromkeys(TOKEN_FIELDS)
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
    return {"requests": attempts, "unreported_requests": missing,
            "zero_input_requests": zero_input, "complete": bool(requests) and missing == 0 and zero_input == 0, **complete,
            "reported_partial": partial,
            "uncached_input_tokens": (complete["input_tokens"] - complete["cache_read_input_tokens"]
                                      - complete["cache_creation_input_tokens"])
            if all(complete[field] is not None for field in TOKEN_FIELDS if field != "output_tokens") else None,
            "provider_reported_cost_usd": sum(costs.values()) if complete_cost else None,
            "provider_reported_cost_partial_usd": sum(costs.values()) if costs else None,
            "reported_providers": sorted(providers), "cost_ledger_basis": "conservative_upper_bound"}


def tool_batch_summary(sizes):
    return {"without_tools": sizes[0], "single_call": sizes[1],
            "multiple_calls": sum(count for size, count in sizes.items() if size > 1),
            "max_calls": max(sizes, default=0)}


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
        self.compactions = {}
        self.rejected_requests, self.overflow_recoveries = set(), set()
        self.regenerations = {}
        self.tool_calls_per_response = Counter()
        self.calls, self.results, self.result_events, self.tool_prepared, self.dispatched = {}, {}, {}, {}, set()
        self.segments, self.admitted_results = {}, set()
        self.context_candidates, self.context_sources, self.staged_tool_blocks = {}, {}, 0
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
        agent_turn = (owner, request[0])
        session_request = (*agent_turn, request[1])
        if kind in ("runtime.step_prepared", "runtime.compaction_started"):
            require(isinstance(request[1], str) and len(request[1]) <= 128 and len(self.session_steps) < 4096,
                    "prepared requests exceed the diagnostic identity/count limit")
            require(session_request not in self.session_steps, "Session contains duplicate prepared request identities")
            metadata = {field: event.get(field) for field in ("id", "agent", "turn", "connection", "model")}
            require(all(isinstance(metadata[field], str) and len(metadata[field]) <= 512 for field in ("connection", "model")),
                    "prepared request routing metadata is invalid or oversized")
            self.session_steps[session_request] = metadata
            if kind == "runtime.step_prepared" and agent_turn in self.regenerations:
                attempt, _ = self.regenerations[agent_turn]
                self.regenerations[agent_turn] = (attempt, True)
        elif kind == "runtime.model_retry":
            self.retry(event, agent_turn, session_request)
        elif kind == "model.response" and session_request in self.session_steps:
            self.session_steps[session_request]["stop_reason"] = event.get("stop_reason")
        elif kind == "tool.call" and session_request in self.session_steps:
            step = self.session_steps[session_request]
            require(not step.get("regenerated"), "discarded model response admitted a tool call")
            step["admitted_tools"] = True
            _, replaced = self.regenerations.pop(agent_turn, (1, True))
            require(replaced, "tool argument regeneration has no replacement request")
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
            self.stage_context(event)
        elif kind == "runtime.compaction_started":
            self.compactions[request] = {"before_bytes": event.get("before_bytes"),
                                         "request_bytes": event.get("request_bytes")}
            if (source := event.get("rejected_request_id")) is not None:
                rejected = (request[0], source)
                require(isinstance(source, str) and rejected in self.rejected_requests - self.overflow_recoveries,
                        "compaction recovery does not name a unique rejected request in the same turn")
                self.overflow_recoveries.add(rejected)
        elif kind == "runtime.compaction_finished" and event.get("status") == "completed":
            require(request in self.compactions and "after_bytes" not in self.compactions[request],
                    "compaction completion has no matching summary request")
            self.confirm_context(event)
            self.compactions[request].update({field: event.get(field) for field in
                                             ("after_bytes", "checkpointed_messages", "elapsed_ms")})
        elif kind == "runtime.step_prepared":
            require(isinstance(request[1], str) and request not in self.steps, "prepared request identity is missing or duplicated")
            self.confirm_context(event)
            require(isinstance(event.get("body_hash"), str) and re.fullmatch(r"[a-f0-9]{64}", event["body_hash"]),
                    "prepared request has no valid body fingerprint")
            require(not self.connection or event.get("connection") == self.connection,
                    "prepared request used a different connection")
            self.steps[request] = self.session_steps[session_request]
            self.settings.append(fingerprint(event))
            self.counts["prepared_steps"] += 1
        elif kind == "runtime.model_rejected":
            require(request in self.steps and request not in self.responses and request not in self.rejected_requests,
                    "rejected model request has no unique unanswered preparation")
            require(event.get("reason") == "context_too_large"
                    and event.get("attempt") == 1 + self.steps[request].get("retry_count", 0),
                    "rejected model request has an invalid reason or attempt")
            self.rejected_requests.add(request)
        elif kind in ("model.response", "runtime.model_usage", "runtime.provider_report"):
            if request in self.compactions and kind != "model.response":
                self.compactions[request]["usage"] = self.session_usages.get(session_request)
                return
            require(request in self.steps and request not in self.rejected_requests
                    and event.get("turn") == self.steps[request].get("turn"),
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
                               "args_hash": fingerprint(event.get("args")), "event_id": identity,
                               "id_hash": fingerprint(event.get("tool_call_id")), "tool_hash": fingerprint(event.get("tool"))}
            self.counts["tool_calls"] += 1
            self.tool_calls_per_response[request] += 1
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
            self.results[key] = fingerprint({field: event.get(field) for field in RESULT_VIEW_FIELDS})
            self.result_events[key] = identity
            self.counts["tool_results"] += 1
            structured = event.get("structured")
            if isinstance(structured, dict):
                require(structured.get("cleanup_incomplete") is not True, "tool result reports incomplete cleanup")
                if structured.get("kind") == "command_result":
                    self.outputs.append(output_reference(self.events_path, structured))
        elif kind == "turn.completed":
            self.counts["turns_completed"] += 1

    def retry(self, event, agent_turn, session_request):
        step = self.session_steps.get(session_request)
        if event.get("recovery") == "regenerate_tool_arguments":
            prior_attempt, replaced = self.regenerations.get(agent_turn, (1, True))
            attempt, maximum = event.get("attempt"), event.get("max_attempts")
            require(step and step.get("stop_reason") == "invalid_response"
                    and session_request in self.session_usages and not step.get("admitted_tools") and not step.get("regenerated")
                    and event.get("will_retry") is True and event.get("failed_kind") == "invalid_response"
                    and event.get("provider_stop_reason") == "tool_use" and replaced
                    and type(attempt) is int and attempt == prior_attempt + 1
                    and event.get("failed_attempt") == prior_attempt and type(maximum) is int and attempt <= maximum,
                    "tool argument regeneration has no eligible rejected response")
            step["regenerated"] = True
            self.regenerations[agent_turn] = (attempt, False)
            return
        require(step and not step.get("retry_stopped"), "retry has no active prepared request")
        phase = event.get("phase")
        retrying = phase in ("waiting", "started")
        require((retrying or phase == "stopped") and event.get("will_retry") is retrying,
                "invalid model retry phase")
        attempt, failed, maximum = (event.get(field) for field in ("attempt", "failed_attempt", "max_attempts"))
        require(all(type(value) is int and value > 0 for value in (attempt, failed, maximum))
                and failed == step.get("last_attempt", 1)
                and attempt == failed + int(retrying) and attempt <= maximum, "invalid model retry attempt")
        if phase == "stopped":
            step["retry_stopped"] = True
            return
        require(session_request not in self.session_usages, "retry has no active prepared request")
        if phase == "waiting":
            step["waiting_attempt"] = attempt
        else:
            require(step.pop("waiting_attempt", None) == attempt, "started retry was not scheduled")
            step["last_attempt"] = attempt
            step["retry_count"] = step.get("retry_count", 0) + 1

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

    def discard_context(self, segment):
        candidate = self.context_candidates.pop(segment, None)
        if candidate:
            self.staged_tool_blocks -= candidate["tool_blocks"]

    def context_source(self, event):
        source = event.get("source_head")
        require(source is None or type(source) is int and 0 <= source < event["id"],
                "context source watermark is invalid")
        return source

    def stage_context(self, event):
        segment, ordinal = event.get("segment"), event.get("ordinal")
        require(segment in self.segments and type(ordinal) is int and ordinal >= self.segments[segment],
                "admitted context rewrites a confirmed prefix or has a missing segment")
        source = self.context_source(event)
        previous = self.context_sources.get(segment)
        require(source is None or previous is None or source >= previous, "context source watermark moved backwards")
        candidate = self.context_candidates.get(segment)
        if candidate and (ordinal == self.segments[segment] or source != candidate["source"]):
            self.discard_context(segment)
            candidate = None
        if candidate is None:
            candidate = {"source": source, "messages": [], "tool_blocks": 0}
            self.context_candidates[segment] = candidate
        require(ordinal == self.segments[segment] + len(candidate["messages"]),
                "admitted context has a nonsequential candidate ordinal")
        blocks = []
        for block in event.get("message", {}).get("blocks", []):
            if "ToolUse" in block:
                use = block["ToolUse"]
                blocks.append(("use", fingerprint(use.get("id")), fingerprint(use.get("name")), fingerprint(use.get("args"))))
            if "ToolResult" in block:
                result = block["ToolResult"]
                blocks.append(("result", fingerprint(result.get("tool_call_id")),
                               fingerprint({field: result.get(field) for field in RESULT_VIEW_FIELDS})))
        self.staged_tool_blocks += len(blocks)
        require(self.staged_tool_blocks <= MAX_STAGED_TOOL_BLOCKS, "unconfirmed context tool blocks exceed the diagnostic limit")
        candidate["messages"].append((event["id"], blocks))
        candidate["tool_blocks"] += len(blocks)
        if source is not None:
            self.context_sources[segment] = source

    def confirm_context(self, event):
        segment, head = event.get("segment"), event.get("context_head")
        require(segment in self.segments and type(head) is int and head >= self.segments[segment],
                "prepared request rewrites a confirmed context head or has a missing segment")
        source = self.context_source(event)
        previous = self.context_sources.get(segment)
        require(source is None or previous is None or source >= previous, "context source watermark moved backwards")
        candidate = self.context_candidates.get(segment)
        if head > self.segments[segment]:
            require(candidate and head == self.segments[segment] + len(candidate["messages"]),
                    "prepared request does not reference its exact candidate context head")
            require(source is None or candidate["source"] is None or source == candidate["source"],
                    "prepared request confirms a different context source watermark")
            for identity, blocks in candidate["messages"]:
                self.admit(blocks, segment, identity)
        require(not self.context_pending[segment], "prepared context has an unpaired admitted tool use")
        self.segments[segment] = head
        self.discard_context(segment)
        if source is not None:
            self.context_sources[segment] = source

    def admit(self, blocks, segment, identity):
        used, pending = self.context_used[segment], self.context_pending[segment]
        for block in blocks:
            if block[0] == "use":
                _, call_id, name, args_hash = block
                candidates = [key for key, call in self.calls.items() if key not in used
                              and call["event_id"] < identity and call["id_hash"] == call_id
                              and call["tool_hash"] == name and call["args_hash"] == args_hash]
                require(candidates and call_id not in pending, "admitted tool use differs from its durable model call")
                pending[call_id] = candidates
            if block[0] == "result":
                _, call_id, digest = block
                matches = [key for key in pending.get(call_id, []) if self.results.get(key) == digest
                           and self.result_events[key] < identity]
                require(matches, "admitted tool result differs from its exact durable first view")
                used.add(matches[0])
                self.admitted_results.add(matches[0])
                pending.pop(call_id)

    def durable_cleanup_closed(self, report):
        snapshot = report.get("agent") or {}
        status = snapshot.get("status")
        return (report.get("session_id") == self.session_id
                and report.get("last_event_id") == self.last_id == self.closed_at
                and report.get("cleanup", {}).get("closed") is True and self.cleanup_failures == 0
                and self.settled is not None and self.settled[0] < self.closed_at
                and status in ("idle", "stopped", "interrupted", "waiting")
                and status == self.settled[1].get("status")
                and snapshot.get("pending_inputs") == len(self.pending) == 0)

    def finish(self, report, require_fixed_limits, require_output_limit):
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
        require(self.steps.keys() == self.responses.keys() | self.rejected_requests
                and self.responses.keys() == self.usages.keys()
                and self.rejected_requests == self.overflow_recoveries,
                "prepared requests lack a response and usage or a recovered context rejection")
        require(all(replaced for _, replaced in self.regenerations.values()), "tool argument regeneration has no replacement request")
        limits = report.get("limits", {})
        max_requests, max_steps = limits.get("max_requests"), limits.get("max_steps")
        max_output_tokens = limits.get("max_output_tokens")
        retries = limits.get("automatic_retries")
        require(("max_requests" in limits or max_steps is not None)
                and (max_requests is None or type(max_requests) is int and max_requests > 0)
                and (max_steps is None or type(max_steps) is int and max_steps > 0)
                and type(limits.get("max_request_bytes")) is int and limits["max_request_bytes"] >= 4096
                and "max_output_tokens" in limits
                and (max_output_tokens is None or type(max_output_tokens) is int and max_output_tokens > 0)
                and type(retries) is int and retries >= 0, "SDK report has invalid request/retry limits")
        if require_fixed_limits or require_output_limit:
            require(max_output_tokens is not None, "paid SDK evidence requires a finite output token limit")
        if require_fixed_limits:
            require(max_requests is not None and retries == 0,
                    "paid SDK evidence requires a finite request limit and disabled automatic retries")
        if max_steps is not None:
            require(all(count <= max_steps for count in Counter(step.get("turn") for step in self.steps.values()).values()),
                    "durable model requests exceeded the declared per-turn limit")
        compaction_requests = {request: self.session_steps[(self.agent, *request)] for request in self.compactions}
        compaction_usage = {request: facts["usage"] for request, facts in self.compactions.items() if "usage" in facts}
        usage = usage_summary(self.steps | compaction_requests, self.usages | compaction_usage, self.reports)
        normalized = snapshot.get("usage", {})
        require(normalized.get("model_requests") == usage["requests"]
                and normalized.get("unreported_requests") == usage["unreported_requests"],
                "SDK usage request counters differ from durable usage")
        require(all(normalized.get(field) == usage["reported_partial"][field] for field in TOKEN_FIELDS),
                "SDK token totals differ from durable normalized usage")
        return {"passed": True, "agent": self.agent, "session_id": self.session_id,
                "facts": dict(self.counts), "last_event_id": self.last_id, "cleanup_closed": True,
                "tool_batches": tool_batch_summary(Counter(self.tool_calls_per_response[key] for key in self.responses)),
                "output_references": self.outputs, "usage": usage, "usage_scope": "selected_agent",
                "session_usage": usage_summary(self.session_steps, self.session_usages, self.session_reports, True),
                "compactions": list(self.compactions.values()),
                "other_agents": sorted(self.created - {self.agent}), "limits": limits,
                "streaming": streaming_summary(report, len(self.steps), self.records, self.text_bytes, self.thinking_bytes),
                "models": sorted({step["model"] for step in self.steps.values()}),
                "connections": sorted({step["connection"] for step in self.steps.values()})}


def inspect_sdk(result_path, data_dir, connection=None, container_data=None, session_id=None, require_streaming=False,
                *, require_fixed_limits=False, require_output_limit=False, expected_max_command_seconds=None,
                expected_max_request_bytes=None):
    inspected = {"passed": False, "result_path": str(result_path), "data_dir": str(data_dir)}
    try:
        report = json_file(result_path)
        inspected["runtime_completed"] = report.get("runtime_completed")
        inspected["cleanup_closed"] = report.get("cleanup", {}).get("closed")
        if "task_deadline_elapsed" in report:
            require(type(report["task_deadline_elapsed"]) is bool, "invalid SDK task deadline evidence")
            inspected["task_deadline_elapsed"] = report["task_deadline_elapsed"]
        if expected_max_command_seconds is not None:
            limit = (report.get("limits") or {}).get("max_command_seconds")
            require(type(limit) is int and limit == expected_max_command_seconds,
                    "SDK report differs from the selected command deadline")
        if expected_max_request_bytes is not None:
            limit = (report.get("limits") or {}).get("max_request_bytes")
            require(type(limit) is int and limit == expected_max_request_bytes,
                    "SDK report differs from the selected request byte limit")
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
            while line := handle.readline(EVENT_LINE_BYTES + 1):
                require(len(line) <= EVENT_LINE_BYTES and line.endswith(b"\n"), "event log has an oversized record or incomplete tail")
                digest.update(line)
                if line.strip():
                    records += 1
                    require(records <= EVENT_RECORD_LIMIT, "event log exceeds the diagnostic record limit")
                    facts.accept(json.loads(line))
        inspected["durable_cleanup_closed"] = facts.durable_cleanup_closed(report)
        inspected.update(facts.finish(report, require_fixed_limits, require_output_limit), events_path=str(path),
                         events_sha256=digest.hexdigest(), records=records)
        if require_streaming:
            require(inspected["streaming"]["coverage_complete"], "required SDK streaming coverage is missing, empty or lagged")
    except (OSError, ValueError, TypeError, KeyError, AttributeError) as error:
        inspected["passed"] = False
        inspected["error"] = str(error)
    return inspected
