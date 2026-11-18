"""Selected provider settings, transport bounds, and accounting for live acceptance."""

import concurrent.futures
from dataclasses import InitVar, dataclass
from decimal import Decimal
import http.client
from http.server import BaseHTTPRequestHandler
import json
from pathlib import Path
import re
import threading
import time
from urllib.parse import urlencode

from run import Budget, ROOT


MILLION = Decimal(1_000_000)
DEEPSEEK_PRICING = "https://api-docs.deepseek.com/quick_start/pricing/"
DEEPSEEK = {
    "deepseek-flash": ("DeepSeek-V4.1-Flash-1017", "0.006", "0.30", "1.20"),
    "deepseek-v4-pro": ("DeepSeek-V4-Pro-0813", "0.044", "1.32", "3.96"),
}
KNOWN_EFFORTS = ("minimal", "low", "medium", "high", "xhigh", "max")
REQUIRED_WORKSPACE_TOOLS = {"run_command", "workspace_script", "read_output", "save_value",
                            "read_file", "edit_file", "create_agent"}


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
        return response.status, json.loads(response.read(1_048_576))["data"]
    finally:
        connection.close()


def usd(value):
    price = Decimal(str(value))
    if not price.is_finite() or price < 0:
        raise ValueError("Invalid unit price in provider metadata")
    return price


@dataclass(frozen=True)
class RunLimits:
    requests: int | None
    request_bytes: int
    output_tokens: int
    byte_ceiling: InitVar[int] = 262144
    output_ceiling: InitVar[int] = 32768

    def __post_init__(self, byte_ceiling, output_ceiling):
        request_limit_valid = self.requests is None or 1 <= self.requests <= 48
        if not (request_limit_valid and 1 <= self.request_bytes <= byte_ceiling
                and 1 <= self.output_tokens <= output_ceiling):
            raise ValueError("Invalid request, byte, or output token limit")


@dataclass(frozen=True)
class Route:
    service: str
    host: str
    connection: str
    wire: str
    model: str
    effort: str
    upstream: str
    upstream_name: str
    upstream_model: str
    context: int
    completion_limit: int
    prompt_rate: Decimal
    cache_read_rate: Decimal
    cache_write_rate: Decimal
    output_rate: Decimal
    price_source: str
    excluded_upstreams: tuple[str, ...] = ()

    @property
    def credential(self):
        return f"KUKU_TEST_{self.service.upper()}_API_KEY"

    @property
    def router(self):
        return self.service == "openrouter"

    @property
    def path(self):
        return "/responses" if self.router else "/chat/completions"

    @property
    def input_ceiling(self):
        return max(self.prompt_rate, self.cache_read_rate, self.cache_write_rate)

    def request_ceiling(self, output_tokens):
        if not 0 < output_tokens <= self.completion_limit:
            raise ValueError("Output token limit exceeds the selected upstream's model limit")
        return self.context * self.input_ceiling + output_tokens * self.output_rate

    def reserve(self, limits):
        if limits.requests is None:
            raise ValueError("An aggregate provider reservation requires a finite request limit")
        return limits.requests * self.request_ceiling(limits.output_tokens)

    def evidence(self, limits):
        return {
            "service": self.service, "provider": self.host, "connection": self.connection,
            "wire": self.wire, "model": self.model, "reasoning_effort": self.effort,
            "upstream": self.upstream, "upstream_provider": self.upstream_name,
            "catalog_upstream_model": self.upstream_model, "provider_context_tokens": self.context,
            "max_requests_including_retries": limits.requests, "max_bytes": limits.request_bytes,
            "max_output_tokens": limits.output_tokens, "price_source": self.price_source,
            "price_usd_per_million_tokens": {
                "prompt": str(self.prompt_rate * MILLION), "cache_read": str(self.cache_read_rate * MILLION),
                "cache_write": str(self.cache_write_rate * MILLION), "completion": str(self.output_rate * MILLION),
            },
            "reservation_usd": str(self.reserve(limits)) if limits.requests is not None else None,
        }

    def config(self, endpoint, limits, *, context_window=None):
        context_window = min(self.context, 65536) if context_window is None else context_window
        prefix = 'default_model = "balanced"\n'
        quote = json.dumps
        for tier in ("balanced", "strong", "light"):
            prefix += f"""
[model.{tier}]
provider = {quote(self.connection)}
model = {quote(self.model)}
think = {quote(self.effort if self.effort in ("low", "medium", "high") else "high")}
reasoning_effort = {quote(self.effort)}
context_window = {context_window}
max_output_tokens = {limits.output_tokens}
"""
        template = f"""
[provider.{self.connection}]
format = {quote(self.wire)}
profile = {quote(self.service)}
base_url = {quote(endpoint)}
api_key = "$KUKU_INTENT_FIXTURE_TOKEN"
"""
        if self.router:
            template += f"""
reasoning_efforts = [{quote(self.effort)}]
[provider.{self.connection}.routing]
order = [{quote(self.upstream)}]
only = [{quote(self.upstream)}]
allow_fallbacks = false
require_parameters = true
[provider.{self.connection}.routing.max_price]
prompt = {float(self.prompt_rate * MILLION)}
completion = {float(self.output_rate * MILLION)}
"""
        return prefix + template + """
[discovery]
auto_discover = false
"""


def route_for(name, *, model=None, effort="low", upstream=None, key=None, verify_catalog=False):
    if name == "deepseek":
        model = model or "deepseek-flash"
        if model not in DEEPSEEK or effort not in ("low", "high", "max") or upstream not in (None, "deepseek"):
            raise ValueError("Choose a priced direct DeepSeek model, upstream deepseek, and effort low/high/max")
        host = "api.deepseek.com"
        status, models = upstream_json(host, "/models", key)
        if status != 200:
            raise RuntimeError(f"Direct DeepSeek model metadata returned HTTP {status}")
        selected, = [candidate for candidate in models if candidate["id"] == model]
        version, hit, miss, output = DEEPSEEK[model]
        levels = selected.get("effort", {}).get("supported_levels", [])
        if effort not in levels:
            raise ValueError(f"Direct DeepSeek did not list {effort!r} for {model!r}")
        return Route(name, host, "deepseek-chat", "openai-chat", model, effort, "deepseek", "DeepSeek",
                     version, selected["context_window"], selected.get("max_output_tokens") or 384000,
                     usd(miss) / MILLION, usd(hit) / MILLION, Decimal(0), usd(output) / MILLION, DEEPSEEK_PRICING)
    if name != "openrouter":
        raise ValueError("Unknown paid acceptance service")
    model = model or "openai/gpt-6-luna"
    upstream = upstream or "openai"
    if (not re.fullmatch(r"[A-Za-z0-9._-]+/[A-Za-z0-9._:-]+", model)
            or not re.fullmatch(r"[A-Za-z0-9._-]+(?:/[A-Za-z0-9._-]+)*", upstream)
            or effort not in KNOWN_EFFORTS):
        raise ValueError("Choose an explicit OpenRouter model, endpoint tag and reasoning effort")
    host = "openrouter.ai"
    path = f"/api/v1/models/{model}/endpoints"
    status, data = upstream_json(host, path)
    if status != 200:
        raise RuntimeError(f"OpenRouter endpoint metadata returned HTTP {status}")
    endpoints = [endpoint for endpoint in data["endpoints"] if endpoint["tag"] == upstream]
    if len(endpoints) != 1 or not {"tools", "reasoning_effort"} <= set(endpoints[0]["supported_parameters"]):
        raise ValueError("The selected OpenRouter endpoint must declare tools and reasoning_effort")
    endpoint, = endpoints
    excluded = ()
    if verify_catalog:
        status, selected = upstream_json(host, f"/api/v1/model/{model}")
        if (status != 200 or selected.get("id") != model or data.get("id") != model
                or effort not in (selected.get("reasoning") or {}).get("supported_efforts", [])
                or endpoint.get("status") != 0 or "max_tokens" not in endpoint["supported_parameters"]):
            raise ValueError("The selected OpenRouter endpoint or model does not confirm the requested settings")
        excluded = tuple(candidate["tag"] for candidate in data["endpoints"]
                         if candidate["tag"].startswith(upstream + "/"))
    pricing = endpoint["pricing"]
    tiers = [pricing, *pricing.get("overrides", [])]

    def maximum(field):
        return max(usd(tier.get(field, pricing.get(field, "0"))) for tier in tiers)

    if maximum("request"):
        raise ValueError("This acceptance boundary requires a per-token endpoint without a fixed request fee")
    return Route(name, host, "openrouter-" + upstream.replace("/", "-"), "openai-responses", model, effort,
                 upstream, endpoint["provider_name"], endpoint["name"].split(" | ")[-1],
                 endpoint["context_length"], endpoint["max_completion_tokens"],
                 maximum("prompt"), maximum("input_cache_read"), maximum("input_cache_write"),
                 maximum("completion"), "https://" + host + path, excluded)


def paid_budget(path, service):
    ledger = path.resolve(strict=True)
    tracker = ROOT / "benchmarks" / "rewrite-state.json"
    if tracker.is_file():
        configured = json.loads(tracker.read_text(encoding="utf-8")).get("paid_acceptance_ledgers", {})
        if service in configured and ledger != Path(configured[service]).resolve():
            raise ValueError(f"Select the {service} ledger for this worktree: {configured[service]}")
    return Budget(ledger)


class RequestBoundary:
    def __init__(self, route, limits, upstream_key, loopback_key, *, workspace=False):
        self.route = route
        self.limits = limits
        self.key = upstream_key
        self.loopback_key = loopback_key
        self.required_tools = REQUIRED_WORKSPACE_TOOLS if workspace else set()
        self.lock = threading.Lock()
        self.phase = "preflight"
        self.attempts = []
        self.rejections = []
        self.armed = False

    def reserve(self, budget, run_id, case):
        route, limits = self.route, self.limits
        budget.reserve(run_id, float(route.reserve(limits)),
                       f"{case} on {route.model} via {route.host}/{route.upstream} ({route.effort}). "
                       f"Up to {limits.requests} requests including retries, {route.context} input tokens "
                       f"and {limits.output_tokens} output tokens each, using {route.price_source}.")
        with self.lock:
            self.armed = True

    def checked_output(self, payload):
        route = self.route
        output = payload.get("max_output_tokens") if route.router else payload.get("max_tokens", payload.get("max_completion_tokens"))
        if (payload.get("model") != route.model or payload.get("stream") is not True
                or type(output) is not int or not 0 < output <= self.limits.output_tokens
                or payload.get("service_tier") not in (None, "default")
                or payload.get("n", 1) != 1 or payload.get("best_of", 1) != 1
                or any(field in payload for field in ("models", "plugins", "web_search_options"))):
            return None
        tools = payload.get("tools", [])
        if not all(tool.get("type") == "function" for tool in tools):
            return None
        names = [(tool if route.router else tool.get("function", {})).get("name") for tool in tools]
        if not all(isinstance(name, str) and name for name in names):
            return None
        if route.router:
            pin = payload.get("provider", {})
            ceiling = pin.get("max_price", {})
            if (pin.get("order") != [route.upstream] or pin.get("only") != [route.upstream]
                    or pin.get("allow_fallbacks") is not False or pin.get("require_parameters") is not True
                    or not 0 < usd(ceiling.get("prompt", 0)) <= route.prompt_rate * MILLION
                    or not 0 < usd(ceiling.get("completion", 0)) <= route.output_rate * MILLION
                    or payload.get("store") is not False or (payload.get("reasoning") or {}).get("effort") != route.effort):
                return None
        elif payload.get("thinking") != {"type": "enabled"} or payload.get("reasoning_effort") != route.effort:
            return None
        return output, sorted(names)

    def observe(self, attempt, line, *, event=None):
        if not line.startswith(b"data:") or not any(marker in line for marker in (
            b'"usage"', b'"tool_calls"', b'"response.output_item.done"'
        )):
            return
        try:
            event = json.loads(line[5:]) if event is None else event
            if self.route.router:
                if event.get("type") == "response.output_item.done" and event.get("item", {}).get("type") == "function_call":
                    attempt.setdefault("streamed_tool_names", []).append(event["item"]["name"])
                if event.get("type") not in {"response.completed", "response.incomplete"}:
                    return
                attempt["response_status"] = event["type"]
                event = event["response"]
                generation = event.get("id", "")
                if generation.startswith("gen-") and not attempt.get("generation_id"):
                    attempt["generation_id"] = generation
            else:
                for choice in event.get("choices", []):
                    if choice.get("finish_reason"):
                        attempt["response_status"] = choice["finish_reason"]
                    for call in choice.get("delta", {}).get("tool_calls", []):
                        if name := call.get("function", {}).get("name"):
                            attempt.setdefault("streamed_tool_names", []).append(name)
            usage = event.get("usage")
            if not isinstance(usage, dict):
                return
            attempt["model_received"] = event.get("model")
            attempt["reported_reasoning_effort"] = event.get("reasoning_effort")
            fields = (("input_tokens", "output_tokens", "cost") if self.route.router
                      else ("prompt_tokens", "completion_tokens", "prompt_cache_hit_tokens", "prompt_cache_miss_tokens"))
            attempt["usage"] = {name: usage[name] for name in fields if type(usage.get(name)) in (int, float)}
            if self.route.router:
                attempt["usage"]["cache_read_input_tokens"] = (usage.get("input_tokens_details") or {}).get("cached_tokens")
                attempt["reported_upstream"] = event.get("provider")
        except (KeyError, TypeError, ValueError):
            return

    def reject(self, reason):
        if len(self.rejections) < 64:
            self.rejections.append({"case": self.phase, "reason": reason})

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
                    if self.path != route.path or not 0 < length <= boundary.limits.request_bytes or not authorized:
                        boundary.reject("path, byte limit or loopback authentication")
                        return self.send_error(400, "outside the acceptance transport boundary")
                    body = self.rfile.read(length)
                    payload = json.loads(body)
                    checked = boundary.checked_output(payload)
                except (AttributeError, ArithmeticError, TypeError, ValueError):
                    boundary.reject("invalid request")
                    return self.send_error(400, "invalid request for the acceptance transport")
                if checked is None:
                    boundary.reject("model, route, effort or output limit")
                    return self.send_error(400, "outside the selected model, route, effort or output limit")
                output, tools = checked
                with boundary.lock:
                    request_limit = boundary.limits.requests
                    if not boundary.armed or request_limit is None or len(boundary.attempts) >= request_limit:
                        boundary.reject("unreserved request or request limit")
                        return self.send_error(429, "acceptance request limit reached or no budget reservation")
                    if not boundary.attempts and not boundary.required_tools <= set(tools):
                        boundary.reject("default workspace tools missing from first request")
                        return self.send_error(400, "the full workspace tool set is required")
                    attempt = {"case": boundary.phase, "request_bytes": length, "max_output_tokens": output,
                               "model": route.model, "reasoning_effort_sent": route.effort,
                               "upstream_requested": route.upstream, "offered_tools": tools}
                    if route.router:
                        create = next((tool for tool in payload.get("tools", []) if tool.get("name") == "create_agent"), None)
                        if create is not None:
                            attempt["create_agent_strict_disabled"] = create.get("strict") is False
                            attempt["create_agent_required_fields"] = create.get("parameters", {}).get("required", [])
                    boundary.attempts.append(attempt)
                boundary.forward(self, attempt, body)

        return Forward

    def forward(self, client, attempt, body):
        route = self.route
        connection = http.client.HTTPSConnection(route.host, timeout=75)
        started = time.monotonic()
        try:
            path = ("/api/v1" if route.router else "") + route.path
            connection.request("POST", path, body, headers={"Authorization": "Bearer " + self.key,
                               "Content-Type": "application/json", "Accept": "text/event-stream"})
            reply = connection.getresponse()
            attempt["http_status"] = reply.status
            if route.router:
                attempt["generation_id"] = reply.getheader("X-Generation-Id")
            client.send_response(reply.status)
            client.send_header("Content-Type", reply.getheader("Content-Type", "application/json"))
            client.end_headers()
            while line := reply.readline(1_048_576):
                self.observe(attempt, line)
                try:
                    client.wfile.write(line)
                    client.wfile.flush()
                except ConnectionError:
                    continue
            attempt["response_ended"] = True
            if reply.status == 200 and not route.router:
                attempt["reported_upstream"] = route.upstream_name
        except (OSError, http.client.HTTPException) as error:
            attempt["transport_error"] = type(error).__name__
        finally:
            attempt["seconds"] = round(time.monotonic() - started, 4)
            connection.close()

    def accounting(self):
        if not self.attempts or any(not row.get("response_ended") or row.get("http_status") != 200 for row in self.attempts):
            return {"complete": False}
        try:
            if self.route.router:
                costs = []
                for row in self.attempts:
                    usage = row.get("usage", {})
                    receipt = row.get("receipt", {}) if row.get("receipt_matches_route") else {}
                    if not valid_counts(usage, "input_tokens", "output_tokens"):
                        usage = receipt
                    if not valid_counts(usage, "input_tokens", "output_tokens") or not usage["input_tokens"]:
                        return {"complete": False}
                    costs.append(usd(usage.get("cost", receipt.get("total_cost"))))
                return {"complete": True, "basis": "provider_reported", "cost_usd": str(sum(costs))}
            cost = Decimal(0)
            route = self.route
            for row in self.attempts:
                usage = row["usage"]
                if not valid_counts(usage, "prompt_cache_hit_tokens", "prompt_cache_miss_tokens",
                                    "prompt_tokens", "completion_tokens"):
                    return {"complete": False}
                hit, miss = usage["prompt_cache_hit_tokens"], usage["prompt_cache_miss_tokens"]
                if hit + miss != usage["prompt_tokens"] or not usage["prompt_tokens"]:
                    return {"complete": False}
                cost += hit * route.cache_read_rate + miss * route.prompt_rate + usage["completion_tokens"] * route.output_rate
            return {"complete": True, "basis": "token_estimate", "cost_usd": str(cost), "rates": "DeepSeek peak"}
        except (ArithmeticError, KeyError, ValueError):
            return {"complete": False}

    def confirm_upstream(self):
        if not self.attempts:
            return None
        if not self.route.router:
            observed = [row.get("model_received") for row in self.attempts]
            names = {self.route.model, self.route.upstream_model}
            return False if any(name not in names and name is not None for name in observed) else (
                None if None in observed else True
            )
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
            confirmations = list(pool.map(self.confirm_generation, self.attempts))
        return False if False in confirmations else (None if None in confirmations else True)

    def confirm_generation(self, attempt, *, delays=(0, 1, 2, 4), require_matching_cost=True):
        if not attempt.get("generation_id"):
            attempt["receipt_lookup"] = "missing_generation_id"
            return None
        route = self.route
        query = urlencode({"id": attempt["generation_id"]})
        for delay in delays:
            if delay:
                time.sleep(delay)
            try:
                status, data = upstream_json(route.host, "/api/v1/generation?" + query, self.key)
            except (OSError, http.client.HTTPException) as error:
                attempt["receipt_lookup"] = type(error).__name__
                return None
            attempt["receipt_lookup"] = status
            if status == 200:
                break
            if status != 404:
                return None
        else:
            return None
        attempt["receipt"] = {
            **{field: data.get(field) for field in ("model", "provider_name", "service_tier", "total_cost", "cancelled")},
            "input_tokens": data.get("native_tokens_prompt"), "output_tokens": data.get("native_tokens_completion"),
            "cache_read_input_tokens": data.get("native_tokens_cached"),
            "cache_write_input_tokens": data.get("native_tokens_cache_write"),
            "reasoning_tokens": data.get("native_tokens_reasoning"),
            "latency_ms": data.get("latency"), "generation_time_ms": data.get("generation_time"),
        }
        attempt["reported_upstream"] = data.get("provider_name")
        stream_cost = attempt.get("usage", {}).get("cost")
        matched = (data.get("id") == attempt["generation_id"] and data.get("provider_name") == route.upstream_name
                   and data.get("model") in (route.upstream_model, route.model)
                   and (not require_matching_cost or stream_cost is None or usd(data["total_cost"]) == usd(stream_cost)))
        attempt["receipt_matches_route"] = matched
        return matched


def valid_counts(usage, *names):
    return all(type(usage.get(name)) is int and usage[name] >= 0 for name in names)
