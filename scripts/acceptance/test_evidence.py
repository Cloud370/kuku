import copy
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from evidence import inspect_sdk, SessionFacts


class Fixture:
    def __init__(self, directory):
        self.directory = directory
        self.data = directory / "session-data"
        self.events = self.data / "p" / "app" / "sessions" / "fixture" / "events.jsonl"
        self.report_path = directory / "sdk-result.json"
        self.records = []
        self.add("session.created", session_id="fixture")
        self.config = {"connection": "fixture", "max_output_tokens": 4096, "no_agents": True}
        self.operation("agent_created", address="main", config=self.config)
        self.operation("input_delivered", input={"command_id": "input", "target": "main"})
        self.turn = self.operation("inputs_accepted", address="main", input_ids=["input"])["id"]
        self.add("turn.started", conversation="main", turn=self.turn)
        self.segment = self.add("runtime.context_segment", agent="main")["id"]
        self.admit(0, {"role": "User", "blocks": [{"Text": "private workload must stay in original facts"}]})
        first = self.step("request-1", 1)
        self.model("request-1", "tool_use", "")
        self.args = {"program": "printf", "args": ["done"], "cwd": ".", "timeout": 3}
        self.add("tool.call", conversation="main", turn=self.turn, request_id="request-1",
                 tool_call_id="call-1", tool="run_command", args=self.args, index=0)
        self.add("runtime.tool_prepared", agent="main", turn=self.turn, tool_call_id="call-1", args=self.args,
                 definition={"schema": {"name": "run_command"}})
        self.add("runtime.tool_dispatched", agent="main", turn=self.turn, tool_call_id="call-1")
        self.output_id = hashlib.sha256(f"main:{self.turn}:call-1".encode()).hexdigest()
        self.output = self.events.parent / "outputs" / f"{self.output_id}.output"
        self.output.parent.mkdir(parents=True)
        metadata = {"complete": True, "stored_bytes": 5, "stdout_bytes": 5, "stderr_bytes": 0,
                    "quota_truncated": False, "cleanup_incomplete": False, "summary": []}
        raw = json.dumps(metadata, separators=(",", ":")).encode()
        header = b"KUKUOUT1" + len(raw).to_bytes(4, "little") + raw
        self.output.write_bytes(header.ljust(4096, b"\0") + b"\1" + (5).to_bytes(4, "little") + b"done\n")
        self.result = {"tool_call_id": "call-1", "status": "ok", "summary": "command finished", "truncated": False,
                       "model_content": "done\n", "structured": {"kind": "command_result", "exit_code": 0,
                       "output_id": self.output_id, "output_ref": {"id": self.output_id, "offset": 0, "bytes": 5},
                       "stdout_bytes": 5, "stderr_bytes": 0, "quota_truncated": False,
                       "cleanup_incomplete": False, "capture_complete": True}}
        self.add("tool.result", conversation="main", turn=self.turn, **self.result)
        self.admit(1, {"role": "Assistant", "blocks": [{"ToolUse": {"id": "call-1", "name": "run_command", "args": self.args}}]})
        self.admit(2, {"role": "User", "blocks": [{"ToolResult": copy.deepcopy(self.result)}]})
        second = self.step("request-2", 3)
        self.model("request-2", "end_turn", "done")
        self.add("turn.completed", conversation="main", turn=self.turn)
        self.totals = {"input_tokens": 200, "output_tokens": 40, "cache_read_input_tokens": 20,
                       "cache_creation_input_tokens": 0, "model_requests": 2, "unreported_requests": 0}
        self.operation("turn_settled", address="main", status="idle", output="done", error=None,
                       turn=self.turn, usage=self.totals)
        self.operation("session_closed")
        self.report = {"session_id": "fixture", "agent": {"address": "main", "config": self.config,
                       "status": "idle", "pending_inputs": 0, "turn": self.turn, "last_output": "done", "usage": self.totals},
                       "events_path": "/kuku/data/p/app/sessions/fixture/events.jsonl", "last_event_id": len(self.records),
                       "facts": {"tool_calls": 1, "tool_results": 1, "model_responses": 2, "prepared_steps": 2, "turns_completed": 1},
                       "provider_settings": [first, second], "provider_reports": [], "limits": {"max_steps": 12,
                       "max_request_bytes": 65536, "max_output_tokens": 4096, "automatic_retries": 0},
                       "runtime_completed": True, "error": None, "cleanup": {"closed": True}}

    def add(self, kind, **fields):
        event = {"id": len(self.records) + 1, "kind": kind, "ts": "fixture-time", **fields}
        self.records.append(event)
        return event

    def operation(self, operation, **fields):
        return self.add("runtime.operation", record={"operation": operation, **fields})

    def admit(self, ordinal, message):
        return self.add("runtime.context_admitted", agent="main", segment=self.segment, ordinal=ordinal, message=message)

    def step(self, request, head):
        return self.add("runtime.step_prepared", agent="main", turn=self.turn, request_id=request, segment=self.segment,
                        context_head=head, connection="fixture", model="fixture-model", url="http://127.0.0.1/chat/completions",
                        body_hash="a" * 64, settings={"reasoning_effort": "low"})

    def model(self, request, stop_reason, text):
        self.add("runtime.model_usage", agent="main", turn=self.turn, request_id=request,
                 usage={"input_tokens": 100, "output_tokens": 20, "cache_read_input_tokens": 10, "cache_creation_input_tokens": 0})
        self.add("model.response", conversation="main", turn=self.turn, request_id=request,
                 text=text, stop_reason=stop_reason, input_tokens_total=100, output_tokens_total=20)

    def write(self):
        self.events.write_text("".join(json.dumps(event) + "\n" for event in self.records))
        self.report_path.write_text(json.dumps(self.report))

    def renumber(self):
        for identity, event in enumerate(self.records, 1):
            event["id"] = identity
        self.report["last_event_id"] = len(self.records)

    def unprepared_retry(self, source_heads=True, partial=False):
        admitted = [event for event in self.records if event["kind"] == "runtime.context_admitted" and event["ordinal"] > 0]
        index = self.records.index(admitted[0])
        abandoned = copy.deepcopy(admitted)
        abandoned[1]["message"]["blocks"][0]["ToolResult"]["model_content"] = "unconfirmed dynamic handoff notice"
        abandoned.append(dict(copy.deepcopy(abandoned[0]), ordinal=3,
                              message={"role": "User", "blocks": [{"Text": "abandoned runtime suffix"}]}))
        self.records[index:index] = abandoned[:1] if partial else abandoned
        self.renumber()
        if source_heads:
            previous = self.records[index - 1]["id"]
            for event in abandoned[:1] if partial else abandoned:
                event["source_head"] = previous
            current = previous if source_heads == "same" else (abandoned[0] if partial else abandoned[-1])["id"]
            for event in admitted:
                event["source_head"] = current
            self.report["provider_settings"][-1]["source_head"] = current
        return abandoned, admitted

    def repeat_turn(self):
        self.records.pop()
        self.operation("input_delivered", input={"command_id": "input-2", "target": "main"})
        self.turn = self.operation("inputs_accepted", address="main", input_ids=["input-2"])["id"]
        self.add("turn.started", conversation="main", turn=self.turn)
        self.admit(3, {"role": "Assistant", "blocks": [{"Text": "done"}]})
        self.admit(4, {"role": "User", "blocks": [{"Text": "second turn"}]})
        first = self.step("request-1", 5)
        self.model("request-1", "tool_use", "")
        self.add("tool.call", conversation="main", turn=self.turn, request_id="request-1",
                 tool_call_id="call-1", tool="run_command", args=self.args, index=0)
        self.add("runtime.tool_prepared", agent="main", turn=self.turn, tool_call_id="call-1",
                 definition={"schema": {"name": "run_command"}})
        self.add("runtime.tool_dispatched", agent="main", turn=self.turn, tool_call_id="call-1")
        result = dict(self.result, model_content="second durable view")
        self.add("tool.result", conversation="main", turn=self.turn, **result)
        self.admit(5, {"role": "Assistant", "blocks": [{"ToolUse": {"id": "call-1", "name": "run_command", "args": self.args}}]})
        self.admit(6, {"role": "User", "blocks": [{"ToolResult": copy.deepcopy(result)}]})
        second = self.step("request-2", 7)
        self.model("request-2", "end_turn", "done")
        self.add("turn.completed", conversation="main", turn=self.turn)
        self.totals.update(input_tokens=400, output_tokens=80, cache_read_input_tokens=40, model_requests=4)
        self.operation("turn_settled", address="main", status="idle", output="done", error=None,
                       turn=self.turn, usage=self.totals)
        for turn in (4, self.turn):
            for request in ("request-1", "request-2"):
                event = self.add("runtime.provider_report", agent="main", turn=turn,
                                 request_id=request, report={"cost_usd": 0.01, "provider": "fixture"})
                self.report["provider_reports"].append(event)
        self.operation("session_closed")
        self.report["last_event_id"] = len(self.records)
        self.report["provider_settings"].extend([first, second])
        self.report["facts"] = {key: value * 2 for key, value in self.report["facts"].items()}
        self.report["agent"]["turn"] = self.turn

    def inspect(self):
        self.write()
        return inspect_sdk(self.report_path, self.data, "fixture", "/kuku/data", "fixture")


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.fixture = Fixture(Path(self.temporary.name))

    def tearDown(self):
        self.temporary.cleanup()

    def test_original_facts_and_retained_output_establish_sdk_execution(self):
        evidence = self.fixture.inspect()
        self.assertTrue(evidence["passed"], evidence)
        self.assertEqual(evidence["usage"]["input_tokens"], 200)
        self.assertEqual(evidence["usage"]["uncached_input_tokens"], 180)
        self.assertEqual(evidence["output_references"][0]["stored_bytes"], 5)
        self.assertNotIn("private workload", json.dumps(evidence))
        self.assertIsNone(evidence["usage"]["provider_reported_cost_usd"])

    def test_unprepared_same_segment_suffix_is_replaced_only_when_the_retry_is_prepared(self):
        for source in (True, False, "same"):
            with self.subTest(source=source):
                self.fixture = Fixture(Path(self.temporary.name) / str(source))
                self.fixture.unprepared_retry(source_heads=source)
                evidence = self.fixture.inspect()
                self.assertTrue(evidence["passed"], evidence)
                self.assertEqual(evidence["facts"]["prepared_steps"], 2)
                self.assertEqual(evidence["facts"]["tool_results"], 1)

    def test_partial_unprepared_tool_use_does_not_leave_a_pending_pair(self):
        self.fixture.unprepared_retry(partial=True)
        evidence = self.fixture.inspect()
        self.assertTrue(evidence["passed"], evidence)

    def test_unconfirmed_tool_fingerprints_are_bounded_and_reclaimed_on_retry(self):
        for limit, passes in ((1, False), (2, True)):
            with self.subTest(limit=limit):
                self.fixture = Fixture(Path(self.temporary.name) / str(limit))
                self.fixture.unprepared_retry()
                with patch("evidence.MAX_STAGED_TOOL_BLOCKS", limit):
                    evidence = self.fixture.inspect()
                self.assertEqual(evidence["passed"], passes, evidence)
                if not passes:
                    self.assertIn("diagnostic limit", evidence["error"])

    def test_unprepared_tail_is_not_validated_as_a_model_view(self):
        candidate = copy.deepcopy(next(event for event in self.fixture.records if event["kind"] == "runtime.context_admitted"))
        candidate.update(ordinal=3, message={"blocks": [{"ToolUse": {"id": "never-called", "name": "run_command", "args": {}}}]})
        self.fixture.records.insert(-1, candidate)
        self.fixture.renumber()
        evidence = self.fixture.inspect()
        self.assertTrue(evidence["passed"], evidence)

    def test_unchanged_prepared_head_discards_an_unconfirmed_suffix(self):
        segment = next(event for event in self.fixture.records if event["kind"] == "runtime.context_segment")
        candidate = copy.deepcopy(next(event for event in self.fixture.records if event["kind"] == "runtime.context_admitted"))
        candidate["message"] = {"blocks": [{"ToolUse": {"id": "never-called", "name": "run_command", "args": {}}}]}
        prepared = dict(copy.deepcopy(self.fixture.report["provider_settings"][0]), context_head=0)
        facts = SessionFacts("main", self.fixture.events, "fixture")
        for event in (segment, candidate, prepared):
            facts.accept(event)
        self.assertEqual(facts.segments[segment["id"]], 0)
        self.assertFalse(facts.context_candidates)
        self.assertFalse(facts.admitted_results)

    def test_abandoned_tool_results_do_not_count_as_committed_model_context(self):
        abandoned, admitted = self.fixture.unprepared_retry()
        abandoned[1]["message"]["blocks"][0]["ToolResult"] = copy.deepcopy(self.fixture.result)
        for event in admitted:
            event["message"] = {"role": "User", "blocks": [{"Text": "retry omitted the tool pair"}]}
        evidence = self.fixture.inspect()
        self.assertFalse(evidence["passed"])
        self.assertIn("not admitted", evidence["error"])

    def test_retry_cannot_rewrite_a_previously_prepared_prefix(self):
        abandoned, _ = self.fixture.unprepared_retry()
        abandoned[0]["ordinal"] = 0
        evidence = self.fixture.inspect()
        self.assertFalse(evidence["passed"])
        self.assertIn("confirmed prefix", evidence["error"])

    def test_prepared_context_must_confirm_the_exact_candidate_and_source(self):
        for mutation, error in (("head", "exact candidate"), ("source", "source watermark"), ("source-regression", "source watermark")):
            with self.subTest(mutation=mutation):
                self.fixture = Fixture(Path(self.temporary.name) / mutation)
                abandoned, admitted = self.fixture.unprepared_retry()
                prepared = self.fixture.report["provider_settings"][-1]
                if mutation == "head":
                    prepared["context_head"] -= 1
                elif mutation == "source":
                    prepared["source_head"] = abandoned[0]["source_head"]
                else:
                    admitted[0]["source_head"] = abandoned[0]["source_head"] - 1
                evidence = self.fixture.inspect()
                self.assertFalse(evidence["passed"])
                self.assertIn(error, evidence["error"])

    def test_deferred_confirmation_cannot_use_tool_facts_written_after_the_admission(self):
        for kind, ordinal, error in (("tool.call", 1, "durable model call"), ("tool.result", 2, "first view")):
            with self.subTest(kind=kind):
                self.fixture = Fixture(Path(self.temporary.name) / kind)
                record = next(event for event in self.fixture.records if event["kind"] == kind)
                admitted = next(event for event in self.fixture.records if event["kind"] == "runtime.context_admitted"
                                and event["ordinal"] == ordinal)
                if kind == "tool.call":
                    self.fixture.records.remove(admitted)
                    self.fixture.records.insert(self.fixture.records.index(record), admitted)
                else:
                    self.fixture.records.remove(record)
                    self.fixture.records.insert(self.fixture.records.index(admitted) + 1, record)
                self.fixture.renumber()
                evidence = self.fixture.inspect()
                self.assertFalse(evidence["passed"])
                self.assertIn(error, evidence["error"])

    def test_sdk_self_report_cannot_replace_mismatching_durable_counters(self):
        self.fixture.report["facts"]["tool_results"] = 9
        result = self.fixture.inspect()
        self.assertFalse(result["passed"])
        self.assertIn("counters", result["error"])

    def test_turn_scoped_request_ids_and_billing_do_not_collide(self):
        self.fixture.repeat_turn()
        result = self.fixture.inspect()
        self.assertTrue(result["passed"], result)
        self.assertEqual(result["usage"]["requests"], 4)
        self.assertAlmostEqual(result["usage"]["provider_reported_cost_usd"], 0.04)
        self.assertAlmostEqual(result["session_usage"]["provider_reported_cost_usd"], 0.04)

    def test_reused_tool_identity_cannot_admit_the_previous_turns_result(self):
        self.fixture.repeat_turn()
        admitted = next(event for event in self.fixture.records if event["kind"] == "runtime.context_admitted"
                        and event["ordinal"] == 6)
        admitted["message"]["blocks"][0]["ToolResult"] = copy.deepcopy(self.fixture.result)
        result = self.fixture.inspect()
        self.assertFalse(result["passed"])
        self.assertIn("first view", result["error"])

    def test_session_billing_includes_another_agents_same_request_identity(self):
        self.fixture.records.pop()
        self.fixture.operation("agent_created", address="reviewer", config={})
        self.fixture.add("runtime.step_prepared", agent="reviewer", turn=4, request_id="request-1",
                         connection="second-connection", model="fixture-model")
        self.fixture.add("runtime.model_usage", agent="reviewer", turn=4, request_id="request-1",
                         usage={"input_tokens": 50, "output_tokens": 10, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0})
        self.fixture.operation("session_closed")
        self.fixture.report["last_event_id"] = len(self.fixture.records)
        result = self.fixture.inspect()
        self.assertTrue(result["passed"], result)
        self.assertEqual(result["usage"]["input_tokens"], 200)
        self.assertEqual(result["session_usage"]["input_tokens"], 250)
        self.assertEqual(result["other_agents"], ["reviewer"])

    def streaming(self):
        self.fixture.report["elapsed_ms"] = 10
        self.fixture.report["streaming"] = {"model_text_chunks": 1, "model_thinking_chunks": 0,
            "tool_output_chunks": 1, "model_text_bytes": 4, "model_thinking_bytes": 0, "tool_output_bytes": 5,
            "first_model_text_ms": 5, "first_model_thinking_ms": None, "first_tool_output_ms": 3,
            "first_model_request_ms": 1, "model_request_events": 2, "committed_events": 20, "lagged_updates": 0}

    def test_streaming_observations_are_checked_against_durable_count_and_byte_bounds(self):
        self.streaming()
        result = self.fixture.inspect()
        self.assertTrue(result["passed"], result)
        self.assertTrue(result["streaming"]["coverage_complete"])
        self.fixture.report["streaming"]["model_text_bytes"] = 99
        self.assertFalse(self.fixture.inspect()["passed"])

    def test_streaming_first_output_time_cannot_exceed_the_run(self):
        self.streaming()
        self.fixture.report["streaming"]["first_model_text_ms"] = 11
        self.assertFalse(self.fixture.inspect()["passed"])

    def test_explicit_streaming_selection_rejects_missing_or_lagged_coverage(self):
        self.fixture.write()
        result = inspect_sdk(self.fixture.report_path, self.fixture.data, "fixture", "/kuku/data", require_streaming=True)
        self.assertFalse(result["passed"])
        self.streaming()
        self.fixture.report["streaming"]["lagged_updates"] = 1
        self.fixture.write()
        result = inspect_sdk(self.fixture.report_path, self.fixture.data, "fixture", "/kuku/data", require_streaming=True)
        self.assertFalse(result["passed"])
        self.assertTrue(result["streaming"]["validated"])

    def test_report_cannot_substitute_a_different_session_identity(self):
        self.fixture.report["session_id"] = "another-session"
        self.assertFalse(self.fixture.inspect()["passed"])

    def test_exact_admitted_tool_view_cannot_be_rewritten(self):
        admitted = next(event for event in self.fixture.records if event["kind"] == "runtime.context_admitted"
                        and event["ordinal"] == 2)
        admitted["message"]["blocks"][0]["ToolResult"]["model_content"] = "rewritten"
        result = self.fixture.inspect()
        self.assertFalse(result["passed"])
        self.assertIn("first view", result["error"])

    def test_model_view_ignores_diagnostic_structure_but_still_validates_raw_output_references(self):
        admitted = next(event for event in self.fixture.records if event["kind"] == "runtime.context_admitted"
                        and event["ordinal"] == 2)
        admitted["message"]["blocks"][0]["ToolResult"]["structured"] = None
        result = self.fixture.inspect()
        self.assertTrue(result["passed"], result)
        self.assertEqual(result["output_references"][0]["stored_bytes"], 5)
        self.fixture.output.unlink()
        self.assertFalse(self.fixture.inspect()["passed"])

    def test_all_canonical_result_view_fields_remain_strict(self):
        admitted = next(event for event in self.fixture.records if event["kind"] == "runtime.context_admitted"
                        and event["ordinal"] == 2)
        original = copy.deepcopy(admitted["message"]["blocks"][0]["ToolResult"])
        for field, value in (("tool_call_id", "other"), ("status", "error"), ("summary", "changed"),
                             ("model_content", "changed"), ("truncated", True)):
            with self.subTest(field=field):
                changed = dict(original, **{field: value, "structured": None})
                admitted["message"]["blocks"][0]["ToolResult"] = changed
                self.assertFalse(self.fixture.inspect()["passed"])

    def test_missing_retained_output_cannot_pass(self):
        self.fixture.output.unlink()
        self.assertFalse(self.fixture.inspect()["passed"])

    def test_missing_durable_tool_result_cannot_be_replaced_by_an_admitted_view(self):
        self.fixture.records = [event for event in self.fixture.records if event["kind"] != "tool.result"]
        self.assertFalse(self.fixture.inspect()["passed"])

    def test_partially_written_retained_output_cannot_pass(self):
        self.fixture.output.write_bytes(self.fixture.output.read_bytes()[:-1])
        result = self.fixture.inspect()
        self.assertFalse(result["passed"])
        self.assertIn("incomplete frame", result["error"])

    def test_missing_usage_stays_unknown_and_keeps_conservative_accounting(self):
        usage = next(event for event in self.fixture.records if event["kind"] == "runtime.model_usage"
                     and event["request_id"] == "request-2")
        usage["usage"] = None
        self.fixture.totals.update(input_tokens=100, output_tokens=20, cache_read_input_tokens=10, unreported_requests=1)
        result = self.fixture.inspect()
        self.assertTrue(result["passed"], result)
        self.assertFalse(result["usage"]["complete"])
        self.assertEqual(result["usage"]["unreported_requests"], 1)
        self.assertIsNone(result["usage"]["input_tokens"])
        self.assertIsNone(result["usage"]["provider_reported_cost_usd"])
        self.assertEqual(result["usage"]["cost_ledger_basis"], "conservative_upper_bound")

    def test_zero_usage_for_nonempty_requests_does_not_establish_free_work(self):
        for event in self.fixture.records:
            if event["kind"] == "runtime.model_usage":
                event["usage"] = {field: 0 for field in event["usage"]}
        self.fixture.totals.update(input_tokens=0, output_tokens=0, cache_read_input_tokens=0)
        result = self.fixture.inspect()
        self.assertTrue(result["passed"], result)
        self.assertFalse(result["usage"]["complete"])
        self.assertEqual(result["usage"]["zero_input_requests"], 2)
        self.assertIsNone(result["usage"]["provider_reported_cost_usd"])

    def test_sdk_cleanup_and_durable_cursor_are_required(self):
        self.fixture.report["cleanup"]["closed"] = False
        result = self.fixture.inspect()
        self.assertFalse(result["passed"])
        self.assertFalse(result["cleanup_closed"])
        self.fixture.report["cleanup"]["closed"] = True
        self.fixture.report["last_event_id"] += 1
        self.assertIn("cursor", self.fixture.inspect()["error"])

    def test_report_cannot_read_events_outside_the_data_mount(self):
        self.fixture.report["events_path"] = "/kuku/data/../../private/events.jsonl"
        self.assertFalse(self.fixture.inspect()["passed"])

    def test_incomplete_event_tail_cannot_pass_closed_sdk_acceptance(self):
        self.fixture.write()
        with self.fixture.events.open("ab") as handle:
            handle.write(b'{"id":999')
        result = inspect_sdk(self.fixture.report_path, self.fixture.data, "fixture", "/kuku/data")
        self.assertFalse(result["passed"])
        self.assertIn("incomplete tail", result["error"])

    def test_inspect_entrypoint_needs_no_credentials_or_budget(self):
        self.fixture.write()
        process = subprocess.run([sys.executable, str(Path(__file__).with_name("run.py")),
                                  "--output", str(Path(self.temporary.name) / "inspection"),
                                  "inspect", "--sdk-result", str(self.fixture.report_path),
                                  "--session-data", str(self.fixture.data), "--connection", "fixture",
                                  "--container-data", "/kuku/data"], capture_output=True, text=True)
        self.assertEqual(process.returncode, 0, process.stdout + process.stderr)
        self.assertTrue(json.loads(process.stdout)["passed"])


if __name__ == "__main__":
    unittest.main()
