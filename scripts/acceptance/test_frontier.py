import concurrent.futures
from decimal import Decimal
from email.message import Message
from io import BytesIO
import json
import os
from pathlib import Path
import re
import stat
import tempfile
import threading
from types import SimpleNamespace
import unittest
from unittest import mock

from frontier import CaseMeter, MODEL_VERSION, ROUTER_MODEL, TOKEN_VARIABLE, WIRE_BYTES, run_live
from interactive_fixture import local_request, serve
from paid_model import Route, RunLimits, route_for
from run import Budget


KEY = "fixture-key-stays-on-host"
TOKEN = "fixture-key-only-for-loopback"
DIRECT = Route("deepseek", "api.deepseek.com", "deepseek-chat", "openai-chat", "deepseek-flash", "max",
               "deepseek", "DeepSeek", MODEL_VERSION, 1048576, 393216, Decimal("0.0000003"),
               Decimal("0.000000006"), Decimal(0), Decimal("0.0000012"), "https://example.invalid/prices")
ROUTER = Route("openrouter", "openrouter.ai", "openrouter-deepseek", "openai-responses", ROUTER_MODEL, "max",
               "deepseek", "DeepSeek", "deepseek/deepseek-v4.1-flash-20261017", 1048576, 393216,
               DIRECT.prompt_rate, DIRECT.cache_read_rate, DIRECT.cache_write_rate, DIRECT.output_rate, DIRECT.price_source)
DIRECT_PAYLOAD = {"model": DIRECT.model, "stream": True, "max_tokens": 131072,
                  "thinking": {"type": "enabled"}, "reasoning_effort": "max", "stream_options": {"include_usage": True},
                  "messages": [{"role": "user", "content": "fixture input text"}]}
ROUTER_PAYLOAD = {"model": ROUTER.model, "stream": True, "store": False, "max_output_tokens": 131072,
                  "reasoning": {"effort": "max", "summary": "auto"},
                  "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "fixture input text"}]}],
                  "provider": {"order": [ROUTER.upstream], "only": [ROUTER.upstream], "allow_fallbacks": False,
                               "require_parameters": True, "max_price": {"prompt": 0.3, "completion": 1.2}}}


def request(endpoint, route, payload, token=TOKEN):
    body = payload if isinstance(payload, bytes) else json.dumps(payload).encode()
    return local_request(endpoint, "POST", route.path, body=body,
                         headers={"Content-Type": "application/json", "Authorization": "Bearer " + token})


def reply_to_client(client, code):
    client.send_response(code)
    client.end_headers()


class StreamReply(BytesIO):
    def __init__(self, *events):
        super().__init__(b"".join(b"data: " + json.dumps(event).encode() + b"\n\n" for event in events))
        self.status = 200
        self.headers = Message()
        self.headers["Content-Type"] = "text/event-stream"

    def getheader(self, name):
        return self.headers.get(name)


class FrontierTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)

    def meter(self, route=DIRECT, output=131072, requests=22, cap="0.75"):
        limits = RunLimits(requests, WIRE_BYTES, output, byte_ceiling=WIRE_BYTES, output_ceiling=route.completion_limit)
        meter = CaseMeter(route, limits, KEY, TOKEN, Decimal(cap), "fixture-case", 300, self.directory / "meter.json")
        meter.arm()
        return meter

    def test_request_ceiling_is_durable_and_concurrent_requests_cannot_overrun_case(self):
        meter = self.meter()
        started = threading.Event()
        release = threading.Event()

        def missing_usage(client, _body):
            reply_to_client(client, 200)
            started.set()
            self.assertTrue(release.wait(2))
            return {"status": "completed", "response_ended": True, "http_status": 200}

        with mock.patch.object(meter, "transfer", side_effect=missing_usage) as forward, serve(meter.handler()) as endpoint:
            with concurrent.futures.ThreadPoolExecutor(max_workers=3) as executor:
                futures = [executor.submit(request, endpoint, DIRECT, DIRECT_PAYLOAD) for _ in range(3)]
                self.assertTrue(started.wait(2))
                try:
                    before_send = json.loads(meter.evidence.read_text())
                    expected = str(DIRECT.request_ceiling(131072))
                    self.assertEqual(before_send["attempts"][0]["reserved_usd"], expected)
                    self.assertEqual(before_send["accounting"]["accounted_usd"], expected)
                    self.assertEqual(forward.call_count, 1)
                finally:
                    release.set()
                self.assertEqual(sorted(future.result().status for future in futures), [200, 429, 429])
        self.assertEqual(meter.accounting()["unknown_requests"], 1)
        self.assertEqual([row["reason"] for row in meter.rejections], ["case USD cap", "case USD cap"])

    def test_partial_usage_and_connection_failure_are_distinct(self):
        meter = self.meter(requests=3)
        complete = {"status": "completed", "response_ended": True, "http_status": 200, "stream_done": True,
                    "model_received": DIRECT.model, "usage": dict(prompt_tokens=100, completion_tokens=150,
                                                                    prompt_cache_hit_tokens=60, prompt_cache_miss_tokens=40)}
        outcomes = iter((
            {"status": "connection_failed", "response_ended": False},
            complete,
            {**complete, "usage": {"prompt_tokens": 100, "completion_tokens": 150}},
        ))

        def transfer(client, _body):
            observation = next(outcomes)
            reply_to_client(client, observation.get("http_status", 502))
            return observation

        with mock.patch.object(meter, "transfer", side_effect=transfer), serve(meter.handler()) as endpoint:
            self.assertEqual([request(endpoint, DIRECT, DIRECT_PAYLOAD).status for _ in range(4)], [502, 200, 200, 429])
        accounting = meter.accounting()
        self.assertEqual(accounting["upstream_requests"], 2)
        self.assertEqual(accounting["no_upstream_charge_requests"], 1)
        self.assertEqual(accounting["estimated_requests"], 1)
        self.assertEqual(accounting["unknown_requests"], 1)
        self.assertEqual(Decimal(accounting["token_estimate_usd"]), Decimal("0.00019236"))
        self.assertIsNone(accounting["provider_billed_usd"])
        self.assertEqual(meter.rejections[-1]["reason"], "request limit")

    def test_auth_and_invalid_charge_affecting_fields_never_send(self):
        meter = self.meter()
        with mock.patch.object(meter, "transfer") as forward, serve(meter.handler()) as endpoint:
            self.assertEqual(request(endpoint, DIRECT, DIRECT_PAYLOAD, token="another-run-token").status, 401)
            invalid = (
                {**DIRECT_PAYLOAD, "max_tokens": 131073},
                {**DIRECT_PAYLOAD, "max_completion_tokens": 131073},
                {**DIRECT_PAYLOAD, "n": 2},
                {**DIRECT_PAYLOAD, "reasoning_effort": "xhigh"},
                {**DIRECT_PAYLOAD, "messages": [{"role": "user", "content": [{"type": "image_url"}]}]},
                json.dumps(DIRECT_PAYLOAD).encode()[:-1] + b', "max_tokens": 131072}',
            )
            self.assertEqual([request(endpoint, DIRECT, item).status for item in invalid], [400] * len(invalid))
            forward.assert_not_called()
        self.assertEqual(meter.attempts, [])

    def test_cancelled_inflight_request_keeps_full_ceiling(self):
        meter = self.meter()
        started = threading.Event()
        release = threading.Event()

        def late_usage(client, _body):
            reply_to_client(client, 200)
            started.set()
            self.assertTrue(release.wait(2))
            return {"status": "completed", "response_ended": True, "stream_done": True, "http_status": 200,
                    "model_received": DIRECT.model,
                    "usage": dict(prompt_tokens=1, completion_tokens=1, prompt_cache_hit_tokens=0, prompt_cache_miss_tokens=1)}

        with mock.patch.object(meter, "transfer", side_effect=late_usage), serve(meter.handler()) as endpoint:
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as executor:
                future = executor.submit(request, endpoint, DIRECT, DIRECT_PAYLOAD)
                self.assertTrue(started.wait(2))
                meter.disarm()
                release.set()
                self.assertEqual(future.result().status, 200)
            self.assertEqual(request(endpoint, DIRECT, DIRECT_PAYLOAD).status, 429)
        self.assertEqual(meter.attempts[0]["status"], "cancelled")
        self.assertEqual(meter.attempts[0]["basis"], "request_upper_bound")
        self.assertEqual(meter.spent, DIRECT.request_ceiling(131072))

    def test_openrouter_receipt_and_stream_timings_stay_sanitized(self):
        meter = self.meter(ROUTER)
        generation = "gen-existing-fixture"
        response = {"id": generation, "model": ROUTER.upstream_model, "reasoning": {"effort": "max"}}
        receipt = {"id": generation, "model": ROUTER.upstream_model, "provider_name": "DeepSeek", "total_cost": 0.00042,
                   "native_tokens_prompt": 1234, "native_tokens_cached": 234, "native_tokens_completion": 789,
                   "native_tokens_reasoning": 56, "latency": 203, "generation_time": 403}
        events = (
            {"type": "response.created", "response": response},
            {"type": "response.output_text.delta", "delta": ""},
            {"type": "response.reasoning_text.delta", "delta": "private reasoning text"},
            {"type": "response.function_call_arguments.delta", "delta": "private tool arguments"},
            {"type": "response.output_text.delta", "delta": "private reply text"},
            {"type": "response.completed", "response": {**response, "usage": {
                "input_tokens": 12, "output_tokens": 14, "input_tokens_details": {"cache_write_tokens": 0},
            }}},
        )
        connection = mock.Mock()
        connection.getresponse.return_value = StreamReply(*events)
        with mock.patch("frontier.http.client.HTTPSConnection", return_value=connection), \
                mock.patch("paid_model.upstream_json", return_value=(200, receipt)) as lookup, serve(meter.handler()) as endpoint:
            local = request(endpoint, ROUTER, ROUTER_PAYLOAD)
        self.assertEqual(local.status, 200)
        self.assertIn(b"private reply text", local.body)
        self.assertFalse(KEY.encode() in local.body)
        args, kwargs = connection.request.call_args
        self.assertEqual(args[:2], ("POST", "/api/v1/responses"))
        self.assertEqual(kwargs["headers"]["Authorization"], "Bearer " + KEY)
        self.assertEqual(json.loads(args[2])["reasoning"]["effort"], "max")
        lookup.assert_called_once_with(ROUTER.host, f"/api/v1/generation?id={generation}", KEY)
        evidence = meter.evidence.read_text()
        for private in (KEY, TOKEN, generation, "fixture input text", "private reasoning text", "private tool arguments",
                        "private reply text", str(self.directory)):
            self.assertNotIn(private, evidence)
        row = meter.attempts[0]
        self.assertTrue(row["receipt_matches_route"])
        self.assertTrue(row["provider_matches_pin"])
        self.assertEqual(row["usage"]["cache_read_input_tokens"], 234)
        self.assertEqual(row["usage"]["cache_write_input_tokens"], 0)
        self.assertEqual(row["provider_reported_usd"], "0.00042")
        self.assertEqual(meter.accounting()["provider_billed_usd"], "0.00042")
        timing = row["timing"]
        first_output = timing["request_to_first_output_delta_seconds"]
        self.assertEqual(timing["request_to_first_reasoning_delta_seconds"], first_output)
        self.assertLessEqual(timing["request_to_first_body_byte_seconds"], first_output)
        self.assertGreaterEqual(timing["request_to_first_text_delta_seconds"], first_output)
        self.assertGreaterEqual(timing["request_to_first_tool_argument_delta_seconds"], first_output)
        self.assertEqual(timing["openrouter_receipt_latency_ms"], 203)
        self.assertEqual(timing["openrouter_receipt_completion_tokens_per_second"], round(789 / 0.403, 3))
        self.assertEqual(timing["stream_completion_tokens"], 789)

    def test_upstream_http_error_keeps_ceiling_and_excludes_raw_error(self):
        meter = self.meter(ROUTER)
        private_message = "fixture message and " + KEY
        reply = BytesIO(json.dumps({"error": {"code": 404, "message": (
            "0 endpoints out of 1 requested are available matching your guardrail restrictions and data policy. "
            "Paid model training violation (account settings): 1 endpoint excluded " + private_message
        )}}).encode())
        reply.status = 404
        reply.headers = Message()
        reply.headers["Content-Type"] = "application/json"
        connection = mock.Mock()
        connection.getresponse.return_value = reply
        with mock.patch("frontier.http.client.HTTPSConnection", return_value=connection), serve(meter.handler()) as endpoint:
            response = request(endpoint, ROUTER, ROUTER_PAYLOAD)
        self.assertEqual(response.status, 404)
        self.assertNotIn(private_message.encode(), response.body)
        self.assertNotIn(private_message, meter.evidence.read_text())
        row = meter.attempts[0]
        self.assertEqual(row["upstream_error"]["kind"], "account_data_policy")
        self.assertEqual(row["upstream_error"]["code"], 404)
        self.assertEqual(row["basis"], "request_upper_bound")

    def test_complete_openrouter_stream_settles_cost_when_receipt_is_not_visible(self):
        meter = self.meter(ROUTER)
        generation = "gen-fixturePendingReceipt"
        validated = {"id": generation, "model": ROUTER.model, "reasoning": {"effort": "max"}, "usage": {
            "input_tokens": 1000, "output_tokens": 120, "cost": 0.00024,
            "input_tokens_details": {"cached_tokens": 640},
        }}
        connection = mock.Mock()
        connection.getresponse.side_effect = [StreamReply(
            {"type": "response.created", "response": {"id": generation}},
            {"type": "response.completed", "response": validated},
        ) for _ in range(3)]
        with mock.patch("frontier.http.client.HTTPSConnection", return_value=connection), \
                mock.patch("paid_model.upstream_json", return_value=(404, None)) as lookup, serve(meter.handler()) as endpoint:
            self.assertEqual([request(endpoint, ROUTER, ROUTER_PAYLOAD).status for _ in range(3)], [200, 200, 200])
        self.assertEqual(lookup.call_count, 3)
        self.assertEqual(meter.spent, Decimal("0.00072"))
        for row in meter.attempts:
            self.assertEqual(row["provider_cost_source"], "stream_usage")
            self.assertEqual(row["receipt_lookup"], 404)
            self.assertIsNone(row["provider_matches_pin"])
            self.assertIsNone(row["receipt_matches_route"])
            self.assertNotIn("reported_upstream", row)
        self.assertEqual(meter.accounting()["provider_cost_sources"]["stream_usage"], {"requests": 3, "usd": "0.00072"})

    def test_openrouter_uses_explicit_generation_id_instead_of_response_id(self):
        meter = self.meter(ROUTER)
        response_id, generation_id = "resp_fixtureOnly", "opaque-generation-fixture"
        response = {"id": response_id, "model": ROUTER.model, "usage": {
            "input_tokens": 1000, "output_tokens": 120, "cost": 0.00024,
        }}
        connection = mock.Mock()
        connection.getresponse.return_value = StreamReply(
            {"type": "response.created", "response": {"id": response_id}},
            {"type": "response.completed", "response": response, "openrouter_metadata": {
                "generation_id": generation_id, "endpoints": {"available": [
                    {"provider": "Elsewhere", "selected": False}, {"provider": "DeepSeek", "selected": True},
                ]},
            }},
        )
        with mock.patch("frontier.http.client.HTTPSConnection", return_value=connection), \
                mock.patch("paid_model.upstream_json", return_value=(404, None)) as lookup, serve(meter.handler()) as endpoint:
            self.assertEqual(request(endpoint, ROUTER, ROUTER_PAYLOAD).status, 200)
        lookup.assert_called_once_with(ROUTER.host, f"/api/v1/generation?id={generation_id}", KEY)
        self.assertEqual(meter.attempts[0]["receipt_id_source"], "metadata")
        self.assertEqual(meter.attempts[0]["reported_upstream"], "DeepSeek")
        self.assertTrue(meter.attempts[0]["provider_matches_pin"])
        self.assertEqual(meter.attempts[0]["upstream_confirmation_source"], "stream_metadata")
        self.assertIsNone(meter.attempts[0]["receipt_matches_route"])
        public = meter.evidence.read_text()
        self.assertNotIn(generation_id, public)
        private = self.directory / ".frontier-receipts.jsonl"
        self.assertEqual(json.loads(private.read_text()), {"request": 1, "id": generation_id, "source": "metadata"})
        if os.name == "posix":
            self.assertEqual(stat.S_IMODE(private.stat().st_mode), 0o600)

    def test_partial_openrouter_stream_and_conflicting_receipt_do_not_release_ceiling(self):
        response = {"id": "gen-fixtureConflict", "model": ROUTER.model, "usage": {
            "input_tokens": 1000, "output_tokens": 120, "cost": 0.0002,
        }}
        receipt = {"id": response["id"], "model": ROUTER.upstream_model, "provider_name": "DeepSeek", "total_cost": 0.0003}
        for ended, lookup_result in ((False, (404, None)), (True, (200, receipt))):
            with self.subTest(ended=ended):
                meter = self.meter(ROUTER)
                connection = mock.Mock()
                connection.getresponse.return_value = StreamReply({"type": "response.completed" if ended else "response.created",
                                                                    "response": response})
                with mock.patch("frontier.http.client.HTTPSConnection", return_value=connection), \
                        mock.patch("paid_model.upstream_json", return_value=lookup_result), serve(meter.handler()) as endpoint:
                    self.assertEqual(request(endpoint, ROUTER, ROUTER_PAYLOAD).status, 200)
                self.assertEqual(meter.spent, ROUTER.request_ceiling(131072))
                self.assertEqual(meter.reconciled, not ended)
                self.assertEqual(meter.attempts[0]["basis"], "request_upper_bound")

    def test_live_runner_reserves_shared_ledger_before_launch_and_uses_fresh_tokens(self):
        ledger = Budget(self.directory / "shared.json")
        ledger.initialize("concurrent local fixture", 1.5, 0)
        binary = self.directory / "sdk-fixture"
        binary.write_bytes(b"fixture binary identity comes from runtime copy")
        case = {"id": "fixture-case", "timeout_sec": 900}
        barrier = threading.Barrier(2)
        exposures = []

        def fake_attempt(caller, _case, _source, _image, directory, mode, env, *, sdk_timeout_sec, on_sdk_finished):
            self.assertEqual(mode, "live")
            self.assertEqual(set(env), {TOKEN_VARIABLE})
            config = caller.config.read_text()
            token = env[TOKEN_VARIABLE]
            self.assertNotIn(KEY, config)
            self.assertNotIn(token, config)
            self.assertIn('api_key = "$' + TOKEN_VARIABLE + '"', config)
            self.assertIn(f"context_window = {ROUTER.context}", config)
            flags = dict(zip(caller.sdk_args[::2], caller.sdk_args[1::2]))
            self.assertEqual(flags["--max-request-bytes"], str(WIRE_BYTES))
            self.assertEqual(flags["--max-context-tokens"], str(ROUTER.context))
            self.assertEqual(flags["--max-output-tokens"], directory.name)
            self.assertEqual(sdk_timeout_sec, 945)
            endpoint = re.search(r'base_url = "(http://[^\"]+)"', config)[1]
            exposures.append((token, endpoint))
            barrier.wait(2)
            self.assertEqual(ledger.committed(json.loads(ledger.path.read_text())), 1.5)
            barrier.wait(2)
            on_sdk_finished()
            self.assertEqual(request(endpoint, ROUTER, ROUTER_PAYLOAD, token).status, 429)
            return {"passed": True, "cleanup_passed": True, "runtime_files": {"session_task": "b" * 64}}

        def run(output):
            directory = self.directory / str(output)
            directory.mkdir()
            args = SimpleNamespace(provider="openrouter", upstream=None, sdk_binary=binary, ledger=ledger.path,
                                   max_requests=22, max_output_tokens=output, case_cap_usd=0.75, agent_timeout_sec=None,
                                   require_streaming=True, run_id=None)
            result = run_live(args, case, directory, "fake-image", directory, {ROUTER.credential: KEY, DIRECT.credential: "unused"})
            return result, json.loads((directory / "meter.json").read_text())

        with mock.patch("frontier.route_for", return_value=ROUTER), mock.patch("frontier.bridge_gateway", return_value="127.0.0.1"), \
                mock.patch("frontier.benchmark_attempt", side_effect=fake_attempt):
            with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:
                outputs = list(executor.map(run, (131072, 262144)))
        self.assertEqual(len({token for token, _ in exposures}), 2)
        for token, endpoint in exposures:
            self.assertNotIn(token, {TOKEN, KEY})
            with self.assertRaises(ConnectionError):
                request(endpoint, ROUTER, ROUTER_PAYLOAD, token)
        paid = json.loads(ledger.path.read_text())
        self.assertEqual(ledger.committed(paid), 0)
        self.assertTrue(all(row["basis"] == "no_upstream_requests" for row in paid["entries"]))
        for result, meter in outputs:
            self.assertEqual(result["meter"], "meter.json")
            self.assertEqual(meter["sdk_binary_sha256"], "b" * 64)

    def test_cap_below_one_provider_ceiling_fails_before_docker_or_reservation(self):
        ledger = Budget(self.directory / "shared.json")
        ledger.initialize("fixture", 5, 0)
        binary = self.directory / "sdk-fixture"
        binary.write_bytes(b"sdk")
        args = SimpleNamespace(sdk_binary=binary, ledger=ledger.path, max_output_tokens=262144, max_requests=22,
                               case_cap_usd=0.50, agent_timeout_sec=60, require_streaming=True)
        with mock.patch("frontier.route_for", return_value=ROUTER), mock.patch("frontier.bridge_gateway") as gateway, \
                mock.patch("frontier.benchmark_attempt") as benchmark:
            with self.assertRaisesRegex(ValueError, "cannot reserve one request"):
                run_live(args, {"id": "fixture-case", "timeout_sec": 900}, self.directory, "fake-image", self.directory,
                         {ROUTER.credential: KEY})
        gateway.assert_not_called()
        benchmark.assert_not_called()
        self.assertEqual(json.loads(ledger.path.read_text())["entries"], [])
        evidence = json.loads((self.directory / "meter.json").read_text())
        self.assertEqual(evidence["outcome"], "cap_below_request_ceiling")
        self.assertEqual(evidence["accounting"]["upstream_requests"], 0)

    def test_route_verifies_exact_max_and_peak_prices_during_off_peak(self):
        endpoint = {"tag": "baseten", "name": "BaseTen | " + ROUTER.upstream_model, "provider_name": "BaseTen",
                    "status": 0, "context_length": ROUTER.context, "max_completion_tokens": 262144,
                    "supported_parameters": ["tools", "reasoning_effort", "max_tokens"],
                    "pricing": {"prompt": "0.00000015", "completion": "0.0000006", "overrides": [
                        {"prompt": "0.0000003", "completion": "0.0000012"},
                    ]}}
        endpoints = {"id": ROUTER_MODEL, "endpoints": [endpoint, {**endpoint, "tag": "baseten/fast"}]}
        selected = {"id": ROUTER_MODEL, "reasoning": {"supported_efforts": ["max", "high", "low"]}}
        with mock.patch("paid_model.upstream_json", side_effect=[(200, endpoints), (200, selected)]):
            route = route_for("openrouter", model=ROUTER_MODEL, effort="max", upstream="baseten", verify_catalog=True)
        self.assertEqual(route.excluded_upstreams, ("baseten/fast",))
        self.assertEqual(route.request_ceiling(131072), Decimal("0.4718592"))
        self.assertEqual(route.request_ceiling(262144), Decimal("0.6291456"))
        with self.assertRaises(ValueError):
            route.request_ceiling(262145)
        selected["reasoning"]["supported_efforts"] = ["high", "xhigh"]
        with mock.patch("paid_model.upstream_json", side_effect=[(200, endpoints), (200, selected)]):
            with self.assertRaisesRegex(ValueError, "does not confirm"):
                route_for("openrouter", model=ROUTER_MODEL, effort="max", upstream="baseten", verify_catalog=True)


if __name__ == "__main__":
    unittest.main()
