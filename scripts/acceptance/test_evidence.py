import copy
import hashlib
import json
from pathlib import Path, PureWindowsPath
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from benchmark_pi import inspect_pi
from evidence import comparison_path, inspect_sdk, SessionFacts


class PiEvidenceTests(unittest.TestCase):
    def test_original_prompt_and_completed_tool_continuation_are_required(self):
        messages = [
            {"role": "user", "content": [{"type": "text", "text": "original prompt"}]},
            {"role": "assistant", "content": [{"type": "toolCall", "id": "fixture"},
                                               {"type": "toolCall", "id": "other"}], "stopReason": "toolUse"},
            {"role": "toolResult", "toolCallId": "fixture"},
            {"role": "toolResult", "toolCallId": "other"},
            {"role": "assistant", "content": [], "stopReason": "stop"},
        ]
        with tempfile.TemporaryDirectory() as temporary:
            data = Path(temporary)
            session = data / "sessions" / "fixture.jsonl"
            session.parent.mkdir()

            def write(entries):
                session.write_text("".join(json.dumps({"type": "message", "message": entry}) + "\n" for entry in entries))

            write(messages)
            report = inspect_pi(data, "original prompt")
            self.assertTrue(report["passed"])
            self.assertEqual(report["tool_batches"],
                             {"without_tools": 1, "single_call": 0, "multiple_calls": 1, "max_calls": 2})
            self.assertFalse(inspect_pi(data, "different prompt")["passed"])
            write(messages[:2] + messages[3:])
            self.assertFalse(inspect_pi(data, "original prompt")["passed"])


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
                       "max_request_bytes": 65536, "max_context_tokens": 131072, "max_output_tokens": 4096,
                       "automatic_retries": 0},
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

    def inspect(self, **options):
        self.write()
        return inspect_sdk(self.report_path, self.data, "fixture", "/kuku/data", "fixture", **options)


class EvidenceTests(unittest.TestCase):
    def test_request_byte_limit_must_match_the_selected_sdk_setting(self):
        limits = self.fixture.report["limits"]
        for reported in (None, True, 65535):
            with self.subTest(reported=reported):
                limits["max_request_bytes"] = reported
                evidence = self.fixture.inspect(expected_max_request_bytes=65536)
                self.assertFalse(evidence["passed"])
                self.assertIn("selected request byte limit", evidence["error"])
        limits["max_request_bytes"] = 65536
        self.assertTrue(self.fixture.inspect(expected_max_request_bytes=65536)["passed"])

    def test_command_deadline_must_match_the_selected_sdk_setting(self):
        for setting in (None, True, 59):
            with self.subTest(setting=setting):
                self.fixture.report["limits"]["max_command_seconds"] = setting
                evidence = self.fixture.inspect(expected_max_command_seconds=60)
                self.assertFalse(evidence["passed"])
                self.assertIn("selected command deadline", evidence["error"])
        self.fixture.report["limits"]["max_command_seconds"] = 60
        self.assertTrue(self.fixture.inspect(expected_max_command_seconds=60)["passed"])

    def test_metered_limits_allow_declared_automatic_retries(self):
        limits = self.fixture.report["limits"]
        limits.pop("max_steps")
        limits["max_requests"] = None
        for retries in (1, 5):
            limits["automatic_retries"] = retries
            evidence = self.fixture.inspect()
            self.assertTrue(evidence["passed"], evidence.get("error"))
            self.assertIsNone(evidence["limits"]["max_requests"])
            self.assertEqual(evidence["limits"]["max_request_bytes"], 65536)
            self.assertEqual(evidence["limits"]["max_context_tokens"], 131072)
            self.assertEqual(evidence["limits"]["automatic_retries"], retries)
        self.assertFalse(self.fixture.inspect(require_fixed_limits=True)["passed"])
        limits["max_requests"] = 22
        self.assertFalse(self.fixture.inspect(require_fixed_limits=True)["passed"])
        limits["automatic_retries"] = 0
        self.assertTrue(self.fixture.inspect(require_fixed_limits=True)["passed"])

    def test_ordinary_inspection_allows_no_host_output_ceiling(self):
        self.fixture.report["limits"].update(max_requests=12, max_output_tokens=None)
        evidence = self.fixture.inspect()
        self.assertTrue(evidence["passed"], evidence.get("error"))
        self.assertIsNone(evidence["limits"]["max_output_tokens"])
        for options in ({"require_fixed_limits": True}, {"require_output_limit": True}):
            evidence = self.fixture.inspect(**options)
            self.assertFalse(evidence["passed"])
            self.assertIn("requires a finite output token limit", evidence["error"])

    def test_windows_extended_paths_preserve_drive_and_share_containment(self):
        def within(path, root):
            return comparison_path(PureWindowsPath(path)).is_relative_to(comparison_path(PureWindowsPath(root)))

        self.assertTrue(within(r"\\?\W:\accept\sessions\events.jsonl", r"w:\accept"))
        self.assertTrue(within(r"w:\accept\events.jsonl", r"\\?\W:\accept"))
        self.assertFalse(within(r"\\?\W:\accept-other\events.jsonl", r"w:\accept"))
        self.assertFalse(within(r"\\?\C:\accept\events.jsonl", r"w:\accept"))
        self.assertTrue(within(r"\\?\UNC\host\share\data\events.jsonl", r"\\host\share\data"))
        self.assertFalse(within(r"\\?\UNC\host\other\data\events.jsonl", r"\\host\share\data"))
        self.assertFalse(within(r"\\?\UNC\other\share\data\events.jsonl", r"\\host\share\data"))

    def test_receipt_list_can_be_a_valid_tool_result(self):
        for event in self.fixture.records:
            if event["kind"] == "tool.call":
                event.update(tool="list_agents", args={"wait_for": "reviewer"})
            elif event["kind"] == "runtime.tool_prepared":
                event["definition"]["schema"]["name"] = "list_agents"
            elif event["kind"] == "tool.result":
                event["structured"] = [{"address": "reviewer", "status": "idle", "last_output": "checked"}]
            elif event["kind"] == "runtime.context_admitted":
                for block in event["message"]["blocks"]:
                    if "ToolUse" in block:
                        block["ToolUse"].update(name="list_agents", args={"wait_for": "reviewer"})
        result = self.fixture.inspect()
        self.assertTrue(result["passed"], result)

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
        self.assertEqual(result["tool_batches"],
                         {"without_tools": 1, "single_call": 1, "multiple_calls": 0, "max_calls": 1})

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

    def test_pre_stream_retry_counts_an_attempt_without_a_second_prepared_step(self):
        fixture = self.fixture
        index = next(i for i, event in enumerate(fixture.records) if event["kind"] == "runtime.model_usage")
        retry = {"kind": "runtime.model_retry", "agent": "main", "turn": fixture.turn,
                 "request_id": "request-1", "attempt": 2, "failed_attempt": 1, "max_attempts": 6, "will_retry": True}
        fixture.records[index:index] = [{**retry, "phase": "waiting"}, {**retry, "phase": "started"}]
        fixture.renumber()
        fixture.report["limits"]["automatic_retries"] = 1
        fixture.totals.update(model_requests=3, unreported_requests=1)
        result = fixture.inspect()
        self.assertTrue(result["passed"], result)
        self.assertEqual(2, result["facts"]["prepared_steps"])
        for usage in (result["usage"], result["session_usage"]):
            self.assertEqual(3, usage["requests"])
            self.assertEqual(1, usage["unreported_requests"])
            self.assertIsNone(usage["input_tokens"])
            self.assertEqual(200, usage["reported_partial"]["input_tokens"])

    def test_terminal_first_attempt_has_a_valid_stop_fact_before_or_after_usage(self):
        fixture = self.fixture
        fixture.report["runtime_completed"] = False
        original = copy.deepcopy(fixture.records)
        usage = next(i for i, event in enumerate(original) if event["kind"] == "runtime.model_usage")
        for already_reported in (False, True):
            with self.subTest(already_reported=already_reported):
                stop = {"kind": "runtime.model_retry", "agent": "main", "turn": fixture.turn,
                        "request_id": "request-1", "attempt": 1, "failed_attempt": 1, "max_attempts": 6,
                        "phase": "stopped", "will_retry": False}
                index = usage + int(already_reported)
                fixture.records = copy.deepcopy(original)
                fixture.records[index:index] = [stop]
                fixture.renumber()
                evidence = fixture.inspect()
                self.assertFalse(evidence["passed"])
                self.assertEqual("SDK runtime did not complete successfully", evidence["error"])

    def test_closed_timeout_reports_cleanup_independent_of_runtime_success(self):
        fixture = self.fixture
        fixture.report.update(runtime_completed=False, error="task exceeded 1s")
        fixture.report["agent"]["status"] = "stopped"
        settled = next(e["record"] for e in fixture.records if e.get("record", {}).get("operation") == "turn_settled")
        settled["status"] = "stopped"
        evidence = fixture.inspect()
        self.assertFalse(evidence["passed"])
        self.assertTrue(evidence["durable_cleanup_closed"])
        fixture.add("diagnostic.after_close")
        fixture.renumber()
        evidence = fixture.inspect()
        self.assertFalse(evidence["durable_cleanup_closed"])

    def test_completed_tool_argument_regeneration_counts_both_prepared_requests(self):
        fixture = self.fixture
        index = next(i for i, event in enumerate(fixture.records) if event["kind"] == "runtime.step_prepared")
        rejected = "request-rejected"
        prepared, usage, response = [{**event, "request_id": rejected} for event in fixture.records[index:index + 3]]
        response["stop_reason"] = "invalid_response"
        retry = {"kind": "runtime.model_retry", "agent": "main", "turn": fixture.turn, "request_id": rejected,
                 "recovery": "regenerate_tool_arguments", "will_retry": True, "failed_kind": "invalid_response",
                 "provider_stop_reason": "tool_use", "attempt": 2}
        fixture.records[index:index] = [prepared, usage, response, retry]
        fixture.renumber()
        fixture.report["provider_settings"].insert(0, prepared)
        fixture.report["facts"].update(prepared_steps=3, model_responses=3)
        fixture.report["limits"]["automatic_retries"] = 1
        fixture.totals.update(model_requests=3, input_tokens=300, output_tokens=60, cache_read_input_tokens=30)
        result = fixture.inspect()
        self.assertTrue(result["passed"], result)
        for accounting in (result["usage"], result["session_usage"]):
            self.assertEqual(3, accounting["requests"])
            self.assertEqual(0, accounting["unreported_requests"])
            self.assertEqual(300, accounting["input_tokens"])

        response["stop_reason"] = "end_turn"
        self.assertIn("no eligible rejected response", fixture.inspect()["error"])
        response["stop_reason"] = "invalid_response"
        call = next(event for event in fixture.records if event["kind"] == "tool.call")
        call["request_id"] = rejected
        self.assertIn("discarded model response admitted a tool call", fixture.inspect()["error"])

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
