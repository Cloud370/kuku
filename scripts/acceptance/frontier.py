"""Host-side provider boundary for the public FrontierHarness cases."""

import contextlib
from decimal import Decimal, ROUND_CEILING
from email.utils import parsedate_to_datetime
import errno
import hmac
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from ipaddress import IPv4Address
import json
import math
import os
from pathlib import Path
import re
import secrets as random_secrets
import socket
import subprocess
import tempfile
import threading
import time
from types import SimpleNamespace

from paid_model import RequestBoundary, RunLimits, route_for, usd, valid_counts
from run import Budget, benchmark_attempt, money, write_json


ROUTER_MODEL_VERSION = "DeepSeek-V4.1-Flash-1017"
ROUTER_MODEL = "deepseek/deepseek-v4.1-flash-1017"
WIRE_BYTES = 4 * 1024 * 1024
PROGRESS_INTERVAL_SECONDS = 2
SDK_SHUTDOWN_SECONDS = 45
TOKEN_VARIABLE = "KUKU_INTENT_FIXTURE_TOKEN"
DEEPSEEK_COUNTS = ("prompt_tokens", "completion_tokens", "prompt_cache_hit_tokens", "prompt_cache_miss_tokens")
ROUTER_COUNTS = ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_write_input_tokens", "reasoning_tokens")
RESPONSE_STATUS = {"response.completed", "response.incomplete", "response.failed", "stop", "length",
                   "tool_calls", "content_filter", "insufficient_system_resource"}
CHAT_FIELDS = {"model", "messages", "stream", "stream_options", "max_tokens", "temperature", "tools",
               "thinking", "reasoning_effort"}
RESPONSES_FIELDS = {"model", "input", "instructions", "stream", "max_output_tokens", "temperature", "tools",
                    "reasoning", "store", "provider", "session_id"}
DIRECT_RESPONSES_FIELDS = (RESPONSES_FIELDS - {"provider"}) | {
    "parallel_tool_calls", "tool_choice", "include", "prompt_cache_key", "text", "client_metadata",
}
RESPONSE_DELTAS = {"response.output_text.delta": "text", "response.refusal.delta": "text",
                   "response.reasoning_text.delta": "reasoning", "response.reasoning_summary_text.delta": "reasoning",
                   "response.function_call_arguments.delta": "tool_argument", "response.custom_tool_call_input.delta": "tool_argument"}
PROXY_TIMING = ("request_to_response_headers_seconds", "request_to_first_body_byte_seconds",
                "request_to_first_output_delta_seconds", "request_to_first_text_delta_seconds",
                "request_to_first_reasoning_delta_seconds", "request_to_first_tool_argument_delta_seconds",
                "request_to_last_output_delta_seconds", "request_to_stream_end_seconds")
UPSTREAM_ERRORS = (
    (r"training violation \(account settings\)", "account_data_policy", "The account data policy excluded the requested endpoint"),
    (r"no endpoints? found.*(?:data policy|data collection|data privacy)", "data_policy", "No endpoint matched the account data policy"),
    (r"no endpoints? found.*(?:data region|region requirement)", "data_region", "No endpoint matched the requested data region"),
    (r"no endpoints? found.*(?:requested parameters|support.*parameter)", "request_parameters", "No endpoint accepted the requested parameters"),
    (r"no endpoints? found.*(?:maximum price|max_price|price limit)", "price_ceiling", "No endpoint met the price ceiling"),
    (r"no endpoints? found", "no_eligible_endpoint", "No eligible endpoint was available"),
    (r"invalid model(?: id)?|model not found", "invalid_model", "The provider did not recognize the model"),
    (r"max(?:imum)? context length|context length exceeded", "context_limit", "The provider rejected the context length"),
)
UPSTREAM_HTTP_ERRORS = {
    429: ("rate_limited", "The upstream provider rate limited this request"),
    503: ("provider_unavailable", "The upstream provider is temporarily unavailable"),
}
LOCAL_LIMIT_CODES = {"case closed": "case_closed", "request limit": "request_limit",
                     "case USD cap": "case_budget_exhausted"}
DAY = r"(?:Mon|Tue|Wed|Thu|Fri|Sat|Sun)"
LONG_DAY = r"(?:Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday)"
MONTH = r"(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)"
TIME_OF_DAY = r"[0-9]{2}:[0-9]{2}:[0-9]{2}"
HTTP_DATE = re.compile(
    rf"(?:{DAY}, [0-9]{{2}} {MONTH} [0-9]{{4}} {TIME_OF_DAY} GMT"
    rf"|{LONG_DAY}, [0-9]{{2}}-{MONTH}-[0-9]{{2}} {TIME_OF_DAY} GMT"
    rf"|{DAY} {MONTH} (?:[0-9]{{2}}| [0-9]) {TIME_OF_DAY} [0-9]{{4}})"
)
RECEIPT_SOURCES = {"response_id": 1, "response_generation_id": 2, "metadata": 3, "header": 4}
OPAQUE_ID = re.compile(r"[A-Za-z0-9._-]{1,160}")


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Duplicate JSON field")
        result[key] = value
    return result


def upstream_error(reply):
    kind, wording = UPSTREAM_HTTP_ERRORS.get(reply.status, ("http_error", "The upstream HTTP request failed"))
    result = {"code": reply.status, "kind": kind, "message": wording}
    if reply.headers.get_content_type() != "application/json":
        return result
    try:
        body = reply.read(8193)
        if len(body) > 8192:
            return result
        payload = json.loads(body, object_pairs_hook=unique_object)
        error = payload.get("error", {})
        message = error.get("message")
    except (AttributeError, OSError, TypeError, ValueError, RecursionError, http.client.HTTPException):
        return result
    metadata = error.get("metadata")
    provider_code = metadata.get("provider_code") if isinstance(metadata, dict) else None
    quota_codes = (error.get("code"), error.get("type"), error.get("error_type"), payload.get("error_type"), provider_code)
    if "insufficient_quota" in quota_codes:
        return {"code": "insufficient_quota", "kind": "insufficient_quota",
                "message": "The upstream provider reports insufficient quota"}
    if type(error.get("code")) is int and 400 <= error["code"] <= 599:
        result["code"] = error["code"]
    if reply.status not in UPSTREAM_HTTP_ERRORS and isinstance(message, str):
        for pattern, kind, wording in UPSTREAM_ERRORS:
            if re.search(pattern, message, re.IGNORECASE):
                result.update(kind=kind, message=wording)
                break
    return result


def retry_after_header(reply):
    values = reply.headers.get_all("Retry-After", [])
    if len(values) != 1:
        return None
    value = values[0].strip(" \t")
    if re.fullmatch(r"[0-9]+", value):
        return value
    if not HTTP_DATE.fullmatch(value):
        return None
    try:
        parsedate_to_datetime(value)
    except (ValueError, OverflowError):
        return None
    return value


def send_json_error(client, status, code, message, *, retry_after=None):
    body = json.dumps({"error": {"code": code, "message": message}}, separators=(",", ":")).encode()
    with contextlib.suppress(OSError):
        client.send_response(status)
        client.send_header("Content-Type", "application/json")
        client.send_header("Content-Length", str(len(body)))
        if retry_after is not None:
            client.send_header("Retry-After", retry_after)
        client.end_headers()
        client.wfile.write(body)


def text_input(items, responses, *, allow_apply_patch=False):
    if not isinstance(items, list) or not items:
        return False
    for item in items:
        if not isinstance(item, dict):
            return False
        content = item.get("content")
        if responses:
            kind = item.get("type")
            if kind in ("custom_tool_call", "custom_tool_call_output") and allow_apply_patch:
                if kind == "custom_tool_call" and item.get("name") != "apply_patch":
                    return False
            elif kind not in (None, "message", "reasoning", "function_call", "function_call_output"):
                return False
        if isinstance(content, list):
            if responses:
                allowed = ("input_text", "output_text", "reasoning_text", "summary_text", "refusal")
                if not all(isinstance(part, dict) and part.get("type") in allowed for part in content):
                    return False
            elif not all(isinstance(part, dict) and set(part) == {"type", "text"}
                         and part["type"] == "text" and isinstance(part["text"], str) for part in content):
                return False
            continue
        if content is not None and not isinstance(content, str):
            return False
    return True


def observe_tool_calls(observation, event, responses):
    if responses:
        if event.get("type") == "response.completed":
            output = (event.get("response") or {}).get("output")
            if isinstance(output, list) and all(isinstance(item, dict) for item in output):
                observation["emitted_tool_calls"] = sum(
                    item.get("type") in ("function_call", "custom_tool_call") for item in output)
        return
    indices = observation.setdefault("chat_tool_indices", set())
    if indices is None:
        return
    for choice in event.get("choices", []):
        for call in (choice.get("delta") or {}).get("tool_calls") or []:
            index = call.get("index") if isinstance(call, dict) else None
            if type(index) is not int or index < 0:
                observation["chat_tool_indices"] = None
                return
            indices.add(index)


def emitted_tool_calls(observation, responses):
    status = observation.get("response_status")
    if responses:
        return observation.get("emitted_tool_calls") if status == "response.completed" else None
    if observation.get("stream_done") and status in ("stop", "tool_calls"):
        indices = observation.get("chat_tool_indices")
        if indices is not None and (indices or status == "stop"):
            return len(indices)
    return None


def output_deltas(event, responses):
    if responses:
        kind = RESPONSE_DELTAS.get(event.get("type"))
        if kind and isinstance(event.get("delta"), str) and event["delta"]:
            yield kind
        return
    for choice in event.get("choices", []):
        delta = choice.get("delta") or {}
        for kind, key in (("text", "content"), ("reasoning", "reasoning_content")):
            if isinstance(delta.get(key), str) and delta[key]:
                yield kind
        if any(isinstance((call.get("function") or {}).get("arguments"), str) and call["function"]["arguments"]
               for call in delta.get("tool_calls", [])):
            yield "tool_argument"


def duration(value):
    return value if type(value) in (int, float) and 0 <= value <= 10**9 and math.isfinite(value) else None


def throughput(tokens, seconds):
    return round(tokens / seconds, 3) if tokens is not None and seconds and seconds > 0 else None


def reported_cost(value):
    if type(value) not in (int, float, str):
        return None
    try:
        return usd(value)
    except (ArithmeticError, ValueError):
        return None


def observe_receipt_id(observation, value, source):
    if not isinstance(value, str) or not OPAQUE_ID.fullmatch(value):
        return
    if source == "response_id" and value.startswith("gen-"):
        source = "response_generation_id"
    if RECEIPT_SOURCES[source] > RECEIPT_SOURCES.get(observation.get("receipt_id_source"), 0):
        observation.update(generation_id=value, receipt_id_source=source)


def selected_provider(metadata):
    endpoints = metadata.get("endpoints")
    available = endpoints.get("available") if isinstance(endpoints, dict) else None
    if not isinstance(available, list):
        return None
    selected = [candidate.get("provider") for candidate in available
                if isinstance(candidate, dict) and candidate.get("selected") is True]
    return selected[0] if len(selected) == 1 and isinstance(selected[0], str) else None


def timing_facts(observation, complete_output_tokens=None, receipt_output_tokens=None):
    facts = {key: observation.get("timing", {}).get(key) for key in PROXY_TIMING}
    start = facts["request_to_first_output_delta_seconds"]
    end = facts["request_to_stream_end_seconds"]
    receipt = observation.get("receipt", {}) if observation.get("receipt_matches_route") else {}
    generation_ms = duration(receipt.get("generation_time_ms"))
    facts.update(
        stream_completion_tokens=complete_output_tokens,
        completion_tokens_per_stream_second=throughput(complete_output_tokens, end - start if end is not None and start is not None else None),
        completion_tokens_per_end_to_end_second=throughput(complete_output_tokens, end),
        openrouter_receipt_latency_ms=duration(receipt.get("latency_ms")),
        openrouter_receipt_generation_time_ms=generation_ms,
        openrouter_receipt_completion_tokens=receipt_output_tokens,
        openrouter_receipt_completion_tokens_per_second=throughput(receipt_output_tokens, generation_ms / 1000 if generation_ms is not None else None),
    )
    return facts


class CaseMeter(RequestBoundary):
    def __init__(self, route, limits, key, token, cap, case_id, timeout, evidence, *, default_output_limit=False):
        super().__init__(route, limits, key, token)
        self.phase = case_id
        self.cap = cap
        self.timeout = timeout
        self.evidence = evidence
        self.progress_evidence = evidence.with_name("meter-progress.json")
        self.default_output_limit = default_output_limit
        self.serial = threading.Lock()
        self.closed = threading.Event()
        self.drained = threading.Event()
        self.drained.set()
        self.active = None
        self.active_observation = None
        self.active_socket = None
        self.spent = Decimal(0)
        self.reconciled = True
        self.outcome = "pending"

    def arm(self):
        with self.lock:
            self.armed = True
            self.outcome = "running"
            self.save()

    def stop_admissions(self):
        with self.lock:
            self.armed = False

    def disarm(self):
        with self.lock:
            self.armed = False
            self.closed.set()
            if self.active is not None:
                self.active["status"] = "cancelled"
                self.active["timing"] = timing_facts(self.active_observation or {})
                self.active = None
                self.active_observation = None
            if self.active_socket is not None:
                with contextlib.suppress(OSError):
                    self.active_socket.shutdown(socket.SHUT_RDWR)
                self.active_socket = None

    def checked_output(self, payload):
        route = self.route
        fields = RESPONSES_FIELDS if route.router else DIRECT_RESPONSES_FIELDS if route.responses else CHAT_FIELDS
        if (not isinstance(payload, dict) or payload.keys() - fields
                or not text_input(payload.get("input" if route.responses else "messages"), route.responses,
                                  allow_apply_patch=route.responses and not route.router)):
            return None
        if route.router:
            effort = {"effort": route.effort}
            if (payload.get("reasoning") not in (effort, {**effort, "summary": "auto"})
                    or set(payload.get("provider", {})) != {"order", "only", "allow_fallbacks", "require_parameters", "max_price"}
                    or set(payload["provider"]["max_price"]) != {"prompt", "completion"}):
                return None
        elif not route.responses and payload.get("stream_options") != {"include_usage": True}:
            return None
        return super().checked_output(payload)

    def handler(self):
        meter = self
        authorization = ("Bearer " + self.loopback_key).encode("ascii")

        class LocalRequest(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def deny(self, status, reason):
                with meter.lock:
                    meter.reject(reason)
                send_json_error(self, status, LOCAL_LIMIT_CODES.get(reason, status), reason)

            def do_POST(self):
                authorizations = self.headers.get_all("Authorization", [])
                if (len(authorizations) != 1
                        or not hmac.compare_digest(authorizations[0].encode("latin-1"), authorization)):
                    return self.deny(401, "authentication")
                lengths = self.headers.get_all("Content-Length", [])
                if (self.path != meter.route.path or len(lengths) != 1
                        or not re.fullmatch(r"[1-9][0-9]{0,7}", lengths[0])
                        or not 0 < int(lengths[0]) <= meter.limits.request_bytes
                        or self.headers.get_content_type() != "application/json"
                        or "Transfer-Encoding" in self.headers or "Content-Encoding" in self.headers):
                    return self.deny(400, "path or body limit")
                try:
                    length = int(lengths[0])
                    incoming = self.rfile.read(length)
                    if len(incoming) != length:
                        return self.deny(400, "incomplete request")
                    payload = json.loads(incoming.decode("utf-8"), object_pairs_hook=unique_object)
                    if meter.default_output_limit and isinstance(payload, dict):
                        payload.setdefault("max_output_tokens", meter.limits.output_tokens)
                    checked = meter.checked_output(payload)
                    if checked is None:
                        return self.deny(400, "model, route, effort or output limit")
                    if meter.route.excluded_upstreams:
                        payload["provider"]["ignore"] = meter.route.excluded_upstreams
                    body = json.dumps(payload, allow_nan=False, ensure_ascii=False, separators=(",", ":")).encode()
                except (ArithmeticError, AttributeError, TypeError, ValueError, RecursionError):
                    return self.deny(400, "invalid request")
                if len(body) > meter.limits.request_bytes:
                    return self.deny(400, "path or body limit")
                meter.forward(self, length, checked[0], body)

        return LocalRequest

    def admit(self, length, output):
        with self.lock:
            ceiling = self.route.request_ceiling(output)
            if not self.armed or not self.reconciled:
                return None, "case closed"
            request_limit = self.limits.requests
            if request_limit is not None and len(self.attempts) >= request_limit:
                return None, "request limit"
            if self.spent + ceiling > self.cap:
                return None, "case USD cap"
            row = {"request": len(self.attempts) + 1, "request_bytes": length, "max_output_tokens": output,
                   "upstream_requested": self.route.upstream,
                   "status": "pending", "reserved_usd": str(ceiling), "accounted_usd": str(ceiling),
                   "basis": "request_upper_bound"}
            self.attempts.append(row)
            self.active = row
            self.drained.clear()
            self.spent += ceiling
            self.save()
            return row, None

    def forward(self, client, length, output, body):
        with self.serial:
            row, reason = self.admit(length, output)
            if row is None:
                return client.deny(403, reason)
            try:
                observation = self.transfer(client, body)
                with self.lock:
                    if self.active is row:
                        self.account(row, observation)
                        self.active = None
                        self.active_observation = None
                        self.save()
            finally:
                self.drained.set()

    def observe(self, observation, line, *, arrival=None):
        if not line.startswith(b"data:"):
            return
        if line[5:].strip() == b"[DONE]":
            observation["stream_done"] = True
            return
        try:
            event = json.loads(line[5:])
            super().observe(observation, line, event=event)
            observe_tool_calls(observation, event, self.route.responses)
            if arrival is not None:
                timing = observation["timing"]
                for kind in output_deltas(event, self.route.responses):
                    timing.setdefault(f"request_to_first_{kind}_delta_seconds", arrival)
                    timing.setdefault("request_to_first_output_delta_seconds", arrival)
                    timing["request_to_last_output_delta_seconds"] = arrival
                    observation["last_output_kind"] = kind
                    observation["output_deltas"] = observation.get("output_deltas", 0) + 1
            if self.route.responses:
                response = event.get("response")
                if isinstance(response, dict):
                    if effort := (response.get("reasoning") or {}).get("effort"):
                        observation["reported_reasoning_effort"] = effort
                    if event.get("type") == "response.failed":
                        observation["response_status"] = "response.failed"
                    if not self.route.router:
                        self.observe_direct_responses_usage(observation, event["type"], response)
                        return
                    for metadata in (event.get("openrouter_metadata"), response.get("openrouter_metadata")):
                        if isinstance(metadata, dict):
                            observe_receipt_id(observation, metadata.get("generation_id"), "metadata")
                            if event.get("type") in ("response.completed", "response.incomplete"):
                                if provider := selected_provider(metadata):
                                    observation["stream_selected_provider"] = provider
                    observe_receipt_id(observation, response.get("id"), "response_id")
                    usage = response.get("usage") or {}
                    if usage and isinstance(observation.get("usage"), dict):
                        details = usage.get("input_tokens_details") or {}
                        observation["usage"]["cache_write_input_tokens"] = details.get("cache_write_tokens")
                        observation["usage"]["reasoning_tokens"] = (usage.get("output_tokens_details") or {}).get("reasoning_tokens")
        except (AttributeError, ArithmeticError, TypeError, ValueError, RecursionError):
            pass

    def observe_direct_responses_usage(self, observation, event_type, response):
        if event_type not in ("response.completed", "response.incomplete"):
            return
        usage = response.get("usage") or {}
        details = usage.get("input_tokens_details") or {}
        if valid_counts(usage, "input_tokens", "output_tokens") and valid_counts(details, "cached_tokens"):
            observation["usage"] = dict(zip(DEEPSEEK_COUNTS, (
                usage["input_tokens"], usage["output_tokens"], details["cached_tokens"],
                usage["input_tokens"] - details["cached_tokens"],
            )))
            observation["stream_done"] = True
            output_details = usage.get("output_tokens_details")
            reasoning = output_details.get("reasoning_tokens") if isinstance(output_details, dict) else None
            if type(reasoning) is int and 0 <= reasoning <= usage["output_tokens"]:
                observation["reasoning_output_tokens"] = reasoning

    def publish_progress(self, observation, stage, elapsed, *, force=False):
        previous = observation.get("progress_elapsed_seconds")
        if not force and previous is not None and elapsed - previous < PROGRESS_INTERVAL_SECONDS:
            return
        with self.lock:
            if self.active is None or self.active_observation is not observation:
                return
            observation["progress_elapsed_seconds"] = elapsed
            timing = observation["timing"]
            sample = {
                "schema_version": 1, "case": self.phase, "request": self.active["request"], "stage": stage,
                "updated_unix_seconds": round(time.time(), 3), "elapsed_seconds": round(elapsed, 6),
                "http_status": observation.get("http_status"), "stream_lines": observation.get("stream_lines", 0),
                "stream_bytes": observation.get("stream_bytes", 0), "output_deltas": observation.get("output_deltas", 0),
                "last_stream_line_seconds": observation.get("last_stream_line_seconds"),
                "last_output_delta_seconds": timing.get("request_to_last_output_delta_seconds"),
                "last_output_kind": observation.get("last_output_kind"),
            }
            with contextlib.suppress(OSError):
                temporary = self.progress_evidence.with_suffix(".tmp")
                temporary.write_text(json.dumps(sample, separators=(",", ":")) + "\n", encoding="utf-8")
                temporary.replace(self.progress_evidence)

    def transfer(self, client, body):
        route = self.route
        observation = {"status": "connection_failed", "response_ended": False, "timing": {},
                       "stream_lines": 0, "stream_bytes": 0, "output_deltas": 0}
        started = time.monotonic()
        with self.lock:
            self.active_observation = observation
        self.publish_progress(observation, "connecting", 0, force=True)
        with contextlib.closing(http.client.HTTPSConnection(route.host, timeout=75)) as connection:
            try:
                connection.connect()
                with self.lock:
                    if not self.armed:
                        return observation
                    self.active_socket = connection.sock
                headers = {"Authorization": "Bearer " + self.key, "Content-Type": "application/json",
                           "Accept": "text/event-stream"}
                if route.router:
                    headers["X-OpenRouter-Metadata"] = "enabled"
                observation["status"] = "transport_error"
                self.publish_progress(observation, "awaiting_headers", time.monotonic() - started, force=True)
                connection.request("POST", ("/api/v1" if route.router else "") + route.path, body, headers=headers)
                with connection.getresponse() as reply:
                    observation["timing"]["request_to_response_headers_seconds"] = round(time.monotonic() - started, 6)
                    self.relay(client, reply, observation, started)
            except (OSError, http.client.HTTPException):
                if "http_status" not in observation and not self.closed.is_set():
                    observation["proxy_http_status"] = 503
                    reason = ("could not connect to upstream provider" if observation["status"] == "connection_failed"
                              else "upstream transport failed before response headers")
                    send_json_error(client, 503, 503, reason)
            finally:
                with self.lock:
                    self.active_socket = None
                observation["seconds"] = round(time.monotonic() - started, 4)
                self.publish_progress(observation, "response_closed", observation["seconds"], force=True)
        if route.router and not self.closed.is_set():
            self.lookup_receipt(observation)
        return observation

    def lookup_receipt(self, observation):
        source = observation.get("receipt_id_source")
        if source is None:
            observation["receipt_lookup"] = "missing_generation_id"
            return
        with self.lock:
            if self.active is not None:
                path = self.evidence.with_name(".frontier-receipts.jsonl")
                with contextlib.suppress(OSError):
                    fd = os.open(path, os.O_CREAT | os.O_WRONLY | os.O_APPEND, 0o600)
                    with os.fdopen(fd, "a", encoding="utf-8") as handle:
                        handle.write(json.dumps({"request": self.active["request"], "id": observation["generation_id"],
                                                 "source": source}) + "\n")
        if source == "response_id":
            observation["receipt_lookup"] = "response_id_only"
            return
        try:
            self.confirm_generation(observation, delays=(0,), require_matching_cost=False)
        except (ArithmeticError, AttributeError, KeyError, TypeError, ValueError):
            observation["receipt_lookup"] = "invalid_response"

    def relay(self, client, reply, observation, started):
        observation["http_status"] = reply.status
        self.publish_progress(observation, "headers_received", time.monotonic() - started, force=True)
        if reply.status != 200:
            observation["status"] = "http_error"
            error = observation["upstream_error"] = upstream_error(reply)
            delay = retry_after_header(reply) if reply.status in UPSTREAM_HTTP_ERRORS else None
            if delay is not None:
                observation["retry_after"] = delay
            send_json_error(client, reply.status, error["code"], error["message"], retry_after=delay)
            return
        if reply.headers.get_content_type() != "text/event-stream":
            observation["status"] = "unexpected_content_type"
            with contextlib.suppress(OSError):
                client.send_error(502, "upstream response is not a stream")
            return
        if self.route.router:
            observe_receipt_id(observation, reply.getheader("X-Generation-Id"), "header")
        downstream_open = True
        try:
            client.send_response(200)
            client.send_header("Content-Type", "text/event-stream")
            client.end_headers()
        except OSError:
            downstream_open = False
        key = self.key.encode()
        first_byte = reply.read1(1)
        timing = observation["timing"]
        if first_byte:
            timing["request_to_first_body_byte_seconds"] = round(time.monotonic() - started, 6)
        while not self.closed.is_set():
            line = first_byte if first_byte == b"\n" else first_byte + reply.readline(WIRE_BYTES + 1 - len(first_byte))
            first_byte = b""
            arrival = round(time.monotonic() - started, 6)
            if len(line) > WIRE_BYTES:
                observation["status"] = "response_line_limit"
                return
            if not line:
                timing["request_to_stream_end_seconds"] = arrival
                observation["response_ended"] = True
                observation["status"] = "completed" if downstream_open else "client_disconnected"
                return
            self.observe(observation, line, arrival=arrival)
            observation["stream_lines"] = observation.get("stream_lines", 0) + 1
            observation["stream_bytes"] = observation.get("stream_bytes", 0) + len(line)
            observation["last_stream_line_seconds"] = arrival
            self.publish_progress(observation, "streaming", arrival, force=observation["stream_lines"] == 1)
            if downstream_open:
                try:
                    client.wfile.write(line.replace(key, b"[REDACTED]"))
                    client.wfile.flush()
                except OSError:
                    downstream_open = False

    def account(self, row, observation):
        route = self.route
        router = route.router
        relay_fields = ("http_status", "proxy_http_status", "response_ended", "seconds", "status", "upstream_error", "retry_after")
        row.update({field: observation[field] for field in relay_fields if field in observation})
        if observation.get("response_status") in RESPONSE_STATUS:
            row["response_status"] = observation["response_status"]
        if "reasoning_output_tokens" in observation:
            row["reasoning_output_tokens"] = observation["reasoning_output_tokens"]
        ceiling = Decimal(row["reserved_usd"])
        basis = "request_upper_bound"
        charge = ceiling
        receipt = observation.get("receipt", {})
        received = receipt.get("model") or observation.get("model_received")
        accepted_model = received in (route.model, route.upstream_model)
        if accepted_model:
            row["model_received"] = received
        route_mismatch = received is not None and not accepted_model
        reported_effort = observation.get("reported_reasoning_effort")
        if reported_effort in ("none", "minimal", "low", "medium", "high", "xhigh", "max"):
            row["reported_reasoning_effort"] = reported_effort
            route_mismatch |= reported_effort != route.effort
        if router:
            row["receipt_matches_route"] = observation.get("receipt_matches_route")
            row["receipt_lookup"] = observation.get("receipt_lookup")
            row["receipt_id_source"] = observation.get("receipt_id_source")
            stream_provider = observation.get("stream_selected_provider")
            reported = observation.get("reported_upstream") or stream_provider
            accepted_providers = {route.upstream_name.casefold(), route.upstream.casefold()}
            row["provider_matches_pin"] = reported.casefold() in accepted_providers if isinstance(reported, str) else None
            if row["provider_matches_pin"]:
                row["reported_upstream"] = route.upstream_name
                row["upstream_confirmation_source"] = ("generation_receipt" if row["receipt_matches_route"]
                                                       else "stream_metadata" if stream_provider else "stream")
            route_mismatch |= (row["provider_matches_pin"] is False or row["receipt_matches_route"] is False
                               or (stream_provider is not None and stream_provider.casefold() not in accepted_providers))
        elif accepted_model:
            row["reported_upstream"] = route.upstream_name
        stream_usage = observation.get("usage", {})
        usage = stream_usage
        fields = ROUTER_COUNTS if router else DEEPSEEK_COUNTS
        if row.get("receipt_matches_route"):
            usage = {field: receipt.get(field) if valid_counts(receipt, field) else usage.get(field)
                     for field in fields}
        counts = {field: usage[field] for field in fields if valid_counts(usage, field)}
        if counts:
            row["usage"] = counts
        cost_mismatch = False
        if row["status"] == "connection_failed":
            charge, basis = Decimal(0), "no_upstream_charge"
        elif router:
            stream_complete = (row.get("http_status") == 200 and row.get("response_ended")
                               and row.get("response_status") in ("response.completed", "response.incomplete")
                               and observation.get("model_received") in (route.model, route.upstream_model)
                               and valid_counts(stream_usage, "input_tokens", "output_tokens")
                               and stream_usage["input_tokens"] > 0)
            stream_cost = reported_cost(stream_usage.get("cost")) if stream_complete else None
            receipt_cost = reported_cost(receipt.get("total_cost")) if row.get("receipt_matches_route") else None
            if receipt_cost is not None and stream_cost is not None:
                cost_mismatch = receipt_cost != stream_cost
                row["stream_cost_matches_receipt"] = not cost_mismatch
            if receipt_cost is not None or stream_cost is not None:
                charge = receipt_cost if receipt_cost is not None else stream_cost
                basis = "provider_reported"
                row["provider_reported_usd"] = str(charge)
                row["provider_cost_source"] = "generation_receipt" if receipt_cost is not None else "stream_usage"
                if type(receipt.get("cancelled")) is bool:
                    row["provider_cancelled"] = receipt["cancelled"]
        elif (not router and accepted_model and observation.get("stream_done") and row.get("response_ended")
              and valid_counts(counts, *DEEPSEEK_COUNTS)):
            prompt, output, hit, miss = (counts[field] for field in DEEPSEEK_COUNTS)
            if prompt and prompt == hit + miss:
                charge = hit * route.cache_read_rate + miss * route.prompt_rate + output * route.output_rate
                basis = "token_estimate"
        prompt_field, output_field = fields[:2]
        exceeded = (charge > ceiling or counts.get(prompt_field, 0) > route.context
                    or counts.get(output_field, 0) > row["max_output_tokens"])
        if route_mismatch or exceeded or cost_mismatch:
            self.reconciled = False
            basis, charge = "request_upper_bound", ceiling
            row["route_mismatch"] = route_mismatch
            row["exceeded_reserved_bound"] = exceeded
        if accepted_model and not route_mismatch and row.get("http_status") == 200 and row.get("response_ended"):
            count = emitted_tool_calls(observation, route.responses)
            if count is not None:
                row["emitted_tool_calls"] = count
        row.update(basis=basis, accounted_usd=str(charge))
        complete_tokens = counts.get(output_field) if basis in ("provider_reported", "token_estimate") and not row.get("provider_cancelled") else None
        receipt_tokens = receipt[output_field] if row.get("receipt_matches_route") and valid_counts(receipt, output_field) else None
        row["timing"] = timing_facts(observation, complete_tokens, receipt_tokens)
        self.spent += charge - ceiling

    def accounting(self):
        by_basis = {name: [row for row in self.attempts if row["basis"] == name]
                    for name in ("provider_reported", "token_estimate", "request_upper_bound", "no_upstream_charge")}
        totals = {name: sum((Decimal(row["accounted_usd"]) for row in rows), Decimal(0))
                  for name, rows in by_basis.items()}
        observed, estimated, unknown, no_charge = by_basis.values()
        by_source = {source: [row for row in observed if row["provider_cost_source"] == source]
                     for source in ("generation_receipt", "stream_usage")}
        fields = ROUTER_COUNTS if self.route.router else DEEPSEEK_COUNTS
        counts = [row["usage"] for row in self.attempts if row.get("usage") and not row.get("route_mismatch")]
        measured = {field: [usage[field] for usage in counts if field in usage] for field in fields}
        complete = self.reconciled and not unknown
        return {"complete": complete, "reconciled": self.reconciled, "upstream_requests": len(self.attempts) - len(no_charge),
                "provider_reported_requests": len(observed), "estimated_requests": len(estimated),
                "provider_cost_sources": {source: {"requests": len(rows), "usd": str(sum(
                    (Decimal(row["accounted_usd"]) for row in rows), Decimal(0),
                ))} for source, rows in by_source.items()},
                "unknown_requests": len(unknown), "no_upstream_charge_requests": len(no_charge),
                "provider_reported_usd": str(totals["provider_reported"]),
                "provider_billed_usd": str(totals["provider_reported"]) if self.route.router and complete and observed else None,
                "token_estimate_usd": str(totals["token_estimate"]),
                "unknown_usage_upper_bound_usd": str(totals["request_upper_bound"]), "accounted_usd": str(self.spent),
                "usage": {field: sum(samples) if samples else None for field, samples in measured.items()},
                "usage_request_counts": {field: len(samples) for field, samples in measured.items()}}

    def save(self, result=None):
        route = self.route
        binary = (result or {}).get("runtime_files", {}).get("session_task")
        write_json(self.evidence, {
            "schema_version": 1, "case": self.phase, "outcome": self.outcome,
            "service": route.service, "wire": route.wire, "provider": route.host,
            "model_version": ROUTER_MODEL_VERSION if route.router else route.upstream_model,
            "model_requested": route.model, "effort": route.effort, "upstream": route.upstream,
            "upstream_provider": route.upstream_name, "allow_fallbacks": False,
            "excluded_upstreams": route.excluded_upstreams, "catalog_upstream_model": route.upstream_model,
            "case_cap_usd": str(self.cap), "price_source": route.price_source,
            "default_output_limit_applied_if_absent": self.default_output_limit,
            "price_usd_per_million_tokens": route.evidence(self.limits)["price_usd_per_million_tokens"],
            "limits": {"max_requests": self.limits.requests, "max_request_bytes": self.limits.request_bytes,
                       "max_output_tokens": self.limits.output_tokens, "agent_timeout_sec": self.timeout,
                       "provider_context_tokens": route.context, "sdk_context_tokens": route.context,
                       "provider_max_output_tokens": route.completion_limit},
            "sdk_binary_sha256": binary if isinstance(binary, str) and re.fullmatch(r"[a-f0-9]{64}", binary) else None,
            "attempts": self.attempts, "rejections": self.rejections, "accounting": self.accounting(),
        })


class BridgeServer(ThreadingHTTPServer):
    def __init__(self, address, handler):
        self.slots = threading.BoundedSemaphore(32)
        super().__init__(address, handler)

    def get_request(self):
        request, address = super().get_request()
        request.settimeout(10)
        return request, address

    def process_request(self, request, address):
        if not self.slots.acquire(blocking=False):
            return self.shutdown_request(request)
        try:
            super().process_request(request, address)
        except BaseException:
            self.slots.release()
            raise

    def process_request_thread(self, request, address):
        try:
            super().process_request_thread(request, address)
        finally:
            self.slots.release()


def bridge_gateway():
    data = subprocess.check_output(["docker", "network", "inspect", "bridge"], text=True, timeout=10)
    network, = json.loads(data)
    if (network.get("Name"), network.get("Driver"), network.get("Scope")) != ("bridge", "bridge", "local"):
        raise ValueError("Frontier live requires the local Docker bridge")
    for entry in network["IPAM"]["Config"]:
        try:
            address = IPv4Address(entry.get("Gateway"))
        except ValueError:
            continue
        if address.is_private and not (address.is_unspecified or address.is_loopback or address.is_link_local
                                      or address.is_reserved or address.is_multicast):
            return str(address)
    raise ValueError("Docker bridge has no local IPv4 gateway")


def ledger_edit(operation, *arguments):
    deadline = time.monotonic() + 10
    while True:
        try:
            return operation(*arguments)
        except OSError as error:
            contended = isinstance(error, BlockingIOError) or (os.name == "nt" and error.errno in (errno.EACCES, errno.EDEADLK))
            if not contended or time.monotonic() >= deadline:
                raise
            time.sleep(0.025)


def run_live(args, case, source, image, directory, secrets):
    from benchmark_source import MAX_FRONTIER_REQUEST_BYTES, frontier_request_bytes

    requested_bytes = getattr(args, "max_request_bytes", None)
    pi_prefix = getattr(args, "pi_prefix", None)
    codex_prefix = getattr(args, "codex_prefix", None)
    node_binary = getattr(args, "node_binary", None)
    if pi_prefix is not None:
        from benchmark_pi import pi_config, pi_install
        pi_install(pi_prefix, node_binary)
    if codex_prefix is not None:
        from benchmark_codex import codex_config, codex_install
        codex_install(codex_prefix, "deepseek-flash")
    provider = getattr(args, "provider", "openrouter")
    upstream = getattr(args, "upstream", None)
    key = secrets.get(f"KUKU_TEST_{provider.upper()}_API_KEY")
    if not key:
        raise ValueError(f"Frontier live requires the {provider} credential")
    router = provider == "openrouter"
    effort = getattr(args, "reasoning_effort", None) or "max"
    route = route_for(provider, model=ROUTER_MODEL if router else getattr(args, "model", None), effort=effort,
                      upstream=(upstream or "deepseek"), key=key, verify_catalog=router,
                      wire="openai-responses" if codex_prefix else getattr(args, "wire", None))
    output_tokens = route.completion_limit if args.max_output_tokens is None else args.max_output_tokens
    if output_tokens is None:
        raise ValueError("The selected provider route did not declare an output token limit. Set --max-output-tokens.")
    limits = RunLimits(args.max_requests, frontier_request_bytes(requested_bytes), output_tokens,
                       byte_ceiling=MAX_FRONTIER_REQUEST_BYTES,
                       output_ceiling=route.completion_limit)
    timeout = args.agent_timeout_sec if args.agent_timeout_sec is not None else case["timeout_sec"]
    if timeout <= 0 or (not (pi_prefix or codex_prefix) and not args.sdk_binary.is_file()):
        raise ValueError("Frontier live requires a positive timeout and an installed runner")
    cap = Decimal(str(money(args.case_cap_usd)))
    token = random_secrets.token_urlsafe(32)
    meter = CaseMeter(route, limits, key, token, cap, case["id"], timeout, directory / "meter.json",
                      default_output_limit=codex_prefix is not None)
    request_ceiling = route.request_ceiling(limits.output_tokens)
    if request_ceiling > cap:
        meter.outcome = "cap_below_request_ceiling"
        meter.save()
        raise ValueError(f"Case USD cap cannot reserve one request at the selected provider limits ({request_ceiling} USD required)")
    budget = Budget(args.ledger.resolve(strict=True))
    run_id = getattr(args, "run_id", None) or directory.name
    result = None
    with tempfile.TemporaryDirectory(dir=directory, prefix=".frontier-") as scratch:
        gateway = bridge_gateway()
        with BridgeServer((gateway, 0), meter.handler()) as server:
            endpoint = f"http://{gateway}:{server.server_port}"
            config = Path(scratch) / ("models.json" if pi_prefix else "config.toml")
            if pi_prefix:
                write_json(config, pi_config(route, endpoint, limits, TOKEN_VARIABLE))
            elif codex_prefix:
                config.write_text(codex_config(route, endpoint, TOKEN_VARIABLE), encoding="utf-8")
            else:
                config.write_text(route.config(endpoint, limits, context_window=route.context), encoding="utf-8")
            sdk_args = ["--reasoning-effort", route.effort, "--timeout", str(timeout),
                        "--max-output-tokens", str(limits.output_tokens), "--max-agents", "16",
                        "--max-context-tokens", str(route.context), "--max-request-bytes", str(limits.request_bytes)]
            if limits.requests is not None:
                sdk_args.extend(["--max-requests", str(limits.requests)])
            max_command_seconds = getattr(args, "max_command_seconds", None)
            if max_command_seconds is not None:
                sdk_args.extend(["--max-command-seconds", str(max_command_seconds)])
            caller = SimpleNamespace(sdk_binary=args.sdk_binary, config=config, connection=route.connection, model=route.model,
                                     require_streaming=args.require_streaming, sdk_args=sdk_args, suite="frontier",
                                     max_command_seconds=max_command_seconds,
                                     max_request_bytes=requested_bytes,
                                     prompts_dir=getattr(args, "prompts_dir", None),
                                     cpus=getattr(args, "cpus", 1),
                                     pi_prefix=pi_prefix, node_binary=node_binary, pi_model=route.model, pi_effort=route.effort,
                                     codex_prefix=codex_prefix, codex_model=route.model,
                                     require_resource_evidence=getattr(args, "require_resource_evidence", False))
            ledger_edit(budget.reserve, run_id, float(cap),
                        f"{case['id']} single-case cap on {route.model} ({route.effort}) via {route.host}/{route.upstream}")
            thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
            thread.start()
            try:
                meter.arm()
                hard_timeout = timeout if pi_prefix or codex_prefix else timeout + SDK_SHUTDOWN_SECONDS
                result = benchmark_attempt(caller, case, source, image, directory, "live", {TOKEN_VARIABLE: token},
                                           sdk_timeout_sec=hard_timeout,
                                           on_sdk_finished=meter.stop_admissions)
            finally:
                meter.stop_admissions()
                meter.drained.wait(timeout=1.5)
                meter.disarm()
                server.shutdown()
                thread.join()
                meter.outcome = "interrupted" if result is None else "passed" if result["passed"] else "failed"
                if not meter.reconciled:
                    meter.outcome = "unreconciled"
                with meter.lock:
                    meter.save(result)
    if not meter.reconciled:
        result.update(passed=False, failure_kind="provider_boundary")
    elif result.get("cleanup_passed"):
        accounting = meter.accounting()
        if accounting["upstream_requests"] == 0:
            basis = "no_upstream_requests"
        elif accounting["unknown_requests"]:
            basis = "metered_request_upper_bound"
        elif route.router and accounting["provider_cost_sources"]["stream_usage"]["requests"]:
            basis = ("provider_reported_stream_and_receipt" if accounting["provider_cost_sources"]["generation_receipt"]["requests"]
                     else "provider_reported_stream")
        else:
            basis = "provider_reported" if route.router else "token_estimate"
        charge = meter.spent.quantize(Decimal("0.000000001"), rounding=ROUND_CEILING)
        ledger_edit(budget.settle, run_id, float(charge), basis,
                    os.path.relpath(meter.evidence, budget.path.parent) + "#accounting")
    result["meter"] = "meter.json"
    return result
