"""Host-side provider boundary for the public FrontierHarness cases."""

import contextlib
from decimal import Decimal, ROUND_CEILING
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


MODEL_VERSION = "DeepSeek-V4.1-Flash-1017"
ROUTER_MODEL = "deepseek/deepseek-v4.1-flash-1017"
WIRE_BYTES = 4 * 1024 * 1024
TOKEN_VARIABLE = "KUKU_INTENT_FIXTURE_TOKEN"
DEEPSEEK_COUNTS = ("prompt_tokens", "completion_tokens", "prompt_cache_hit_tokens", "prompt_cache_miss_tokens")
ROUTER_COUNTS = ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_write_input_tokens", "reasoning_tokens")
RESPONSE_STATUS = {"response.completed", "response.incomplete", "response.failed", "stop", "length",
                   "tool_calls", "content_filter", "insufficient_system_resource"}
CHAT_FIELDS = {"model", "messages", "stream", "stream_options", "max_tokens", "temperature", "tools",
               "thinking", "reasoning_effort"}
RESPONSES_FIELDS = {"model", "input", "instructions", "stream", "max_output_tokens", "temperature", "tools",
                    "reasoning", "store", "provider", "session_id"}
RESPONSE_DELTAS = {"response.output_text.delta": "text", "response.refusal.delta": "text",
                   "response.reasoning_text.delta": "reasoning", "response.reasoning_summary_text.delta": "reasoning",
                   "response.function_call_arguments.delta": "tool_argument"}
PROXY_TIMING = ("request_to_response_headers_seconds", "request_to_first_body_byte_seconds",
                "request_to_first_output_delta_seconds", "request_to_first_text_delta_seconds",
                "request_to_first_reasoning_delta_seconds", "request_to_first_tool_argument_delta_seconds",
                "request_to_last_output_delta_seconds", "request_to_stream_end_seconds")


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Duplicate JSON field")
        result[key] = value
    return result


def text_input(items, router):
    if not isinstance(items, list) or not items:
        return False
    for item in items:
        if not isinstance(item, dict):
            return False
        content = item.get("content")
        if router:
            if item.get("type") not in (None, "message", "reasoning", "function_call", "function_call_output"):
                return False
            if isinstance(content, list):
                if all(isinstance(part, dict) and part.get("type") in (
                    "input_text", "output_text", "reasoning_text", "summary_text", "refusal",
                ) for part in content):
                    continue
                return False
        if content is not None and not isinstance(content, str):
            return False
    return True


def output_deltas(event, router):
    if router:
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
    def __init__(self, route, limits, key, token, cap, case_id, timeout, evidence):
        super().__init__(route, limits, key, token)
        self.phase = case_id
        self.cap = cap
        self.timeout = timeout
        self.evidence = evidence
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
        router = self.route.router
        if (not isinstance(payload, dict) or payload.keys() - (RESPONSES_FIELDS if router else CHAT_FIELDS)
                or not text_input(payload.get("input" if router else "messages"), router)):
            return None
        if router:
            effort = {"effort": self.route.effort}
            if (payload.get("reasoning") not in (effort, {**effort, "summary": "auto"})
                    or set(payload.get("provider", {})) != {"order", "only", "allow_fallbacks", "require_parameters", "max_price"}
                    or set(payload["provider"]["max_price"]) != {"prompt", "completion"}):
                return None
        elif payload.get("stream_options") != {"include_usage": True}:
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
                self.send_error(status, reason)

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
            if len(self.attempts) >= self.limits.requests:
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
                return client.deny(429, reason)
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
            if arrival is not None:
                timing = observation["timing"]
                for kind in output_deltas(event, self.route.router):
                    timing.setdefault(f"request_to_first_{kind}_delta_seconds", arrival)
                    timing.setdefault("request_to_first_output_delta_seconds", arrival)
                    timing["request_to_last_output_delta_seconds"] = arrival
            if self.route.router:
                response = event.get("response")
                if isinstance(response, dict):
                    metadata = response.get("openrouter_metadata") or {}
                    generation = metadata.get("generation_id") or response.get("id")
                    if generation and not observation.get("generation_id"):
                        observation["generation_id"] = generation
                    if effort := (response.get("reasoning") or {}).get("effort"):
                        observation["reported_reasoning_effort"] = effort
                    if event.get("type") == "response.failed":
                        observation["response_status"] = "response.failed"
                    usage = response.get("usage") or {}
                    if usage and isinstance(observation.get("usage"), dict):
                        details = usage.get("input_tokens_details") or {}
                        observation["usage"]["cache_write_input_tokens"] = details.get("cache_write_tokens")
                        observation["usage"]["reasoning_tokens"] = (usage.get("output_tokens_details") or {}).get("reasoning_tokens")
        except (AttributeError, ArithmeticError, TypeError, ValueError, RecursionError):
            pass

    def transfer(self, client, body):
        route = self.route
        observation = {"status": "connection_failed", "response_ended": False, "timing": {}}
        started = time.monotonic()
        with self.lock:
            self.active_observation = observation
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
                connection.request("POST", ("/api/v1" if route.router else "") + route.path, body, headers=headers)
                with connection.getresponse() as reply:
                    observation["timing"]["request_to_response_headers_seconds"] = round(time.monotonic() - started, 6)
                    self.relay(client, reply, observation, started)
            except (OSError, http.client.HTTPException):
                pass
            finally:
                with self.lock:
                    self.active_socket = None
                observation["seconds"] = round(time.monotonic() - started, 4)
        generation = observation.get("generation_id")
        if route.router and not self.closed.is_set() and isinstance(generation, str) and re.fullmatch(
            r"(?:gen-|resp[_-])[A-Za-z0-9_-]{1,152}", generation,
        ):
            try:
                self.confirm_generation(observation, delays=(0, 0.25, 0.75), require_matching_cost=False)
            except (ArithmeticError, AttributeError, KeyError, TypeError, ValueError):
                observation["receipt_lookup"] = "invalid_response"
        return observation

    def relay(self, client, reply, observation, started):
        observation["http_status"] = reply.status
        if reply.status != 200:
            observation["status"] = "http_error"
            with contextlib.suppress(OSError):
                client.send_error(reply.status, "upstream provider request failed")
            return
        if reply.headers.get_content_type() != "text/event-stream":
            observation["status"] = "unexpected_content_type"
            with contextlib.suppress(OSError):
                client.send_error(502, "upstream response is not a stream")
            return
        if self.route.router and (generation := reply.getheader("X-Generation-Id")):
            observation["generation_id"] = generation
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
            if downstream_open:
                try:
                    client.wfile.write(line.replace(key, b"[REDACTED]"))
                    client.wfile.flush()
                except OSError:
                    downstream_open = False

    def account(self, row, observation):
        route = self.route
        router = route.router
        row.update({field: observation[field] for field in ("http_status", "response_ended", "seconds", "status")
                    if field in observation})
        if observation.get("response_status") in RESPONSE_STATUS:
            row["response_status"] = observation["response_status"]
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
            reported = observation.get("reported_upstream")
            row["provider_matches_pin"] = reported == route.upstream_name if reported is not None else None
            if reported == route.upstream_name:
                row["reported_upstream"] = reported
            route_mismatch |= (reported is not None and reported != route.upstream_name
                               or row["receipt_matches_route"] is False)
        elif accepted_model:
            row["reported_upstream"] = route.upstream_name
        usage = observation.get("usage", {})
        fields = ROUTER_COUNTS if router else DEEPSEEK_COUNTS
        if row.get("receipt_matches_route"):
            usage = {field: receipt.get(field) if valid_counts(receipt, field) else usage.get(field)
                     for field in fields}
        counts = {field: usage[field] for field in fields if valid_counts(usage, field)}
        if counts:
            row["usage"] = counts
        if row["status"] == "connection_failed":
            charge, basis = Decimal(0), "no_upstream_charge"
        elif router and row.get("receipt_matches_route") and receipt.get("total_cost") is not None:
            try:
                charge = usd(receipt["total_cost"])
            except (ArithmeticError, ValueError):
                pass
            else:
                basis = "provider_reported"
                row["provider_reported_usd"] = str(charge)
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
        if route_mismatch or exceeded:
            self.reconciled = False
            basis, charge = "request_upper_bound", ceiling
            row["route_mismatch"] = route_mismatch
            row["exceeded_reserved_bound"] = exceeded
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
        fields = ROUTER_COUNTS if self.route.router else DEEPSEEK_COUNTS
        counts = [row["usage"] for row in self.attempts if row.get("usage") and not row.get("route_mismatch")]
        measured = {field: [usage[field] for usage in counts if field in usage] for field in fields}
        complete = self.reconciled and not unknown
        return {"complete": complete, "reconciled": self.reconciled, "upstream_requests": len(self.attempts) - len(no_charge),
                "provider_reported_requests": len(observed), "estimated_requests": len(estimated),
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
            "service": route.service, "wire": route.wire, "provider": route.host, "model_version": MODEL_VERSION,
            "model_requested": route.model, "effort": route.effort, "upstream": route.upstream,
            "upstream_provider": route.upstream_name, "allow_fallbacks": False,
            "excluded_upstreams": route.excluded_upstreams, "catalog_upstream_model": route.upstream_model,
            "case_cap_usd": str(self.cap), "price_source": route.price_source,
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
        except BlockingIOError:
            if time.monotonic() >= deadline:
                raise
            time.sleep(0.025)


def run_live(args, case, source, image, directory, secrets):
    provider = getattr(args, "provider", "openrouter")
    upstream = getattr(args, "upstream", None)
    key = secrets.get(f"KUKU_TEST_{provider.upper()}_API_KEY")
    if not key:
        raise ValueError(f"Frontier live requires the {provider} credential")
    router = provider == "openrouter"
    route = route_for(provider, model=ROUTER_MODEL if router else "deepseek-flash", effort="max",
                      upstream=(upstream or "deepseek"), key=key, verify_catalog=router)
    limits = RunLimits(args.max_requests, WIRE_BYTES, args.max_output_tokens, byte_ceiling=WIRE_BYTES,
                       output_ceiling=route.completion_limit)
    timeout = args.agent_timeout_sec if args.agent_timeout_sec is not None else case["timeout_sec"]
    if timeout <= 0 or not args.sdk_binary.is_file():
        raise ValueError("Frontier live requires a positive SDK timeout and a built --sdk-binary")
    cap = Decimal(str(money(args.case_cap_usd)))
    token = random_secrets.token_urlsafe(32)
    meter = CaseMeter(route, limits, key, token, cap, case["id"], timeout, directory / "meter.json")
    if route.request_ceiling(limits.output_tokens) > cap:
        meter.outcome = "cap_below_request_ceiling"
        meter.save()
        raise ValueError("Case USD cap cannot reserve one request at the selected provider limits")
    budget = Budget(args.ledger.resolve(strict=True))
    run_id = getattr(args, "run_id", None) or directory.name
    result = None
    with tempfile.TemporaryDirectory(dir=directory, prefix=".frontier-") as scratch:
        gateway = bridge_gateway()
        with BridgeServer((gateway, 0), meter.handler()) as server:
            config = Path(scratch) / "config.toml"
            config.write_text(route.config(f"http://{gateway}:{server.server_port}", limits,
                                          context_window=route.context), encoding="utf-8")
            caller = SimpleNamespace(sdk_binary=args.sdk_binary, config=config, connection=route.connection,
                                     require_streaming=args.require_streaming, sdk_args=[
                                         "--reasoning-effort", route.effort, "--timeout", str(timeout),
                                         "--max-requests", str(limits.requests), "--max-steps", str(limits.requests),
                                         "--max-output-tokens", str(limits.output_tokens), "--max-agents", "16",
                                         "--max-context-tokens", str(route.context), "--max-request-bytes", str(WIRE_BYTES),
                                     ])
            ledger_edit(budget.reserve, run_id, float(cap),
                        f"{case['id']} single-case cap on {route.model} via {route.host}/{route.upstream}")
            thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
            thread.start()
            try:
                meter.arm()
                result = benchmark_attempt(caller, case, source, image, directory, "live", {TOKEN_VARIABLE: token},
                                           sdk_timeout_sec=timeout + 45, on_sdk_finished=meter.stop_admissions)
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
        else:
            basis = "provider_reported" if route.router else "token_estimate"
        charge = meter.spent.quantize(Decimal("0.000000001"), rounding=ROUND_CEILING)
        ledger_edit(budget.settle, run_id, float(charge), basis,
                    os.path.relpath(meter.evidence, budget.path.parent) + "#accounting")
    result["meter"] = "meter.json"
    return result
