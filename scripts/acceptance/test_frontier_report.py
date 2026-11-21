import json
from hashlib import sha256
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from benchmark_compare import artifact_observation, compare, compare_effort, compare_prompts
from benchmark_resources import CASE_RESOURCE_LIMITS
from frontier_report import (CASES, DATASET, DELAYED_RECEIPTS, MODEL_ID, PRIVATE_RECEIPTS,
                            SDK_REQUEST_BYTES_ERROR, TASK_COMMIT, EvidenceError, build_report, cohort_efficiency, main, official_report,
                             resource_summary, session_summary)


SECRET = 'do-not-publish-local-identity'
SDK_SHA = '1' * 64
GIT_COMMIT = '3' * 40
ROUTER_MODEL = 'deepseek/deepseek-v4.1-flash-1017'


def official_feed():
    fields = ['task', 'model', 'harness', 'attempt', 'outcome', 'lookup', 'cost_usd', 'run_seconds']
    feed = {
        'settings': {'dataset': DATASET}, 'complete': True, 'attempts': 4, 'running': [],
        'harnesses': [{'id': 'pi', 'version': '1.4.0'}, *({'id': f'other-{n}'} for n in range(21))],
        'models': [{'id': MODEL_ID, 'provider': 'baseten', 'effort': 'max',
                    'pricing': {'input': 0.3, 'cached_input': 0.007, 'output': 1.2}}],
        'tasks': [{'name': case} for case in CASES],
        'finished_fields': fields, 'outcomes': ['pass', 'fail', 'timeout', 'infra'],
        'finished': [[task, 0, harness, attempt, 0, 0, 0.1, attempt * 10]
                     for task in range(4) for harness in range(22) for attempt in range(1, 5)],
    }
    feed['finished'][1][5] = 1
    feed['finished'][2][5] = 2
    feed['finished'][3][4] = 3
    feed['finished'][4][4:6] = [2, 1]
    return feed


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding='utf-8')


class PairedReportTests(unittest.TestCase):
    def test_artifact_observation_uses_complete_matching_mutations(self):
        digests = {'/app/a': SDK_SHA, '/app/b': '2' * 64}
        attempt = {'task_passed': True, 'task_artifacts': [
            {'task_path': path, 'sha256': digest, 'status': 'retained'} for path, digest in digests.items()
        ]}
        write_a, write_b = (
            {'canonical_path': path, 'commit_state': 'committed', 'content_hash_after': 'sha256:' + digest}
            for path, digest in digests.items()
        )

        def event(kind, second, **fields):
            return {'kind': kind, 'ts': f'2026-01-01T00:00:{second:02d}.123456789Z', **fields}

        with tempfile.TemporaryDirectory() as temporary:
            live = Path(temporary)
            journal = live / 'session-data' / 'p' / 'app' / 'sessions' / 'fixture' / 'events.jsonl'
            journal.parent.mkdir(parents=True)
            prepared = event('runtime.step_prepared', 1)
            committed = event('tool.result', 3, status='ok', structured=write_a)
            rejected = event('tool.result', 4, status='error', structured=write_b)
            scripted = event('tool.result', 7, status='ok', structured={
                'kind': 'workspace_script_result', 'actions': [{'status': 'ok', 'result': write_b}],
            })
            completed = event('turn.completed', 10)

            def write(events):
                journal.write_text(''.join(json.dumps(row) + '\n' for row in events))

            write([prepared, committed, rejected, completed])
            incomplete = artifact_observation(live, attempt)
            self.assertEqual(incomplete['matched_artifacts'], 1)
            self.assertIsNone(incomplete['seconds_from_first_prepared_request'])

            write([prepared, committed, rejected, scripted, completed])
            self.assertEqual(artifact_observation(live, attempt), {
                'clock': 'session_journal', 'retained_artifacts': 2, 'matched_artifacts': 2,
                'seconds_from_first_prepared_request': 6.0, 'seconds_to_last_completed_turn': 3.0,
            })
            self.assertIsNone(artifact_observation(live, {**attempt, 'task_passed': False}))

    def test_effort_pair_checks_model_runtime_and_prompt(self):
        provider = {'model_version': MODEL_ID, 'effort': 'max'}
        provenance = {'kuku_binary_sha256': SDK_SHA, 'kuku_runtime_sha256': SDK_SHA,
                      'prompt_overrides_sha256': None}
        left = ({'case': 'regex-log', 'provider': provider}, {'provenance': provenance})
        right = ({'case': 'regex-log', 'provider': {**provider, 'effort': 'high'}},
                 {'provenance': dict(provenance)})
        paths = (Path('max'), Path('high'))
        with patch('benchmark_compare.attempt', side_effect=[left, right]):
            report = compare_effort(*paths)
        self.assertEqual(report['configuration']['provider'], {'model_version': MODEL_ID})
        self.assertEqual([arm['reasoning_effort'] for arm in report['attempts']], ['max', 'high'])
        for pair, error in (
            ([left, left], 'recorded model version and distinct efforts'),
            ([left, ({**right[0], 'provider': {**right[0]['provider'], 'model_version': 'other'}}, right[1])],
             'configuration differs in provider'),
            ([left, (right[0], {'provenance': {**provenance, 'prompt_overrides_sha256': '2' * 64}})],
             'same Prompt'),
            ([left, (right[0], {'provenance': {**provenance, 'kuku_runtime_sha256': '2' * 64}})],
             'same SDK binary and packaged runtime'),
        ):
            with self.subTest(error=error), patch('benchmark_compare.attempt', side_effect=pair):
                with self.assertRaisesRegex(EvidenceError, error):
                    compare_effort(*paths)
        with patch('benchmark_compare.attempt', side_effect=[left, right]):
            with self.assertRaisesRegex(EvidenceError, 'configuration differs in provider'):
                compare_prompts(*paths)

    def test_prompt_pair_requires_unchanged_runtime_and_distinct_snapshots(self):
        provenance = {'kuku_binary_sha256': SDK_SHA, 'kuku_runtime_sha256': SDK_SHA,
                      'prompt_overrides_sha256': '2' * 64}
        left = ({'case': 'regex-log'}, {'provenance': provenance})
        right = ({'case': 'regex-log'}, {'provenance': {**provenance, 'prompt_overrides_sha256': '3' * 64}})
        with patch('benchmark_compare.attempt', side_effect=[left, right]):
            report = compare_prompts(Path('old'), Path('new'))
        self.assertEqual([arm['arm'] for arm in report['attempts']], ['baseline', 'candidate'])
        with patch('benchmark_compare.attempt', side_effect=[left, left]):
            with self.assertRaisesRegex(EvidenceError, 'distinct recorded Prompt'):
                compare_prompts(Path('old'), Path('new'))
        right[1]['provenance']['kuku_runtime_sha256'] = '4' * 64
        with patch('benchmark_compare.attempt', side_effect=[left, right]):
            with self.assertRaisesRegex(EvidenceError, 'same SDK binary and packaged runtime'):
                compare_prompts(Path('old'), Path('new'))

    def test_exports_matched_evidence_even_for_verifier_failure(self):
        case = 'regex-log'
        benchmark = {'case': {'id': case}, 'suite': 'frontier', 'source': {'revision': TASK_COMMIT},
                     'image_id': 'sha256:' + SDK_SHA, 'task_files_sha256': {'instruction.md': SDK_SHA}}
        meter = {'case': case, 'service': 'deepseek', 'wire': 'openai-chat', 'provider': 'api.deepseek.com',
                 'model_requested': 'deepseek-flash', 'effort': 'max', 'case_cap_usd': '0.75',
                 'limits': {'max_request_bytes': 4194304, 'max_output_tokens': 131072, 'agent_timeout_sec': 900,
                            'provider_context_tokens': 1048576, 'max_requests': None}}
        with tempfile.TemporaryDirectory() as temporary:
            kuku, pi = (Path(temporary) / name for name in ('kuku', 'pi'))
            for directory in (kuku, pi):
                write_json(directory / 'result.json', {'group': 'benchmark', 'benchmark': benchmark,
                                                        'attempts': [{'harness': directory.name, 'mode': 'live',
                                                                      'resource_requirements': CASE_RESOURCE_LIMITS,
                                                                      'reward': 0, 'session_data': SECRET}]})
                write_json(directory / 'meter.json', meter)
            report = compare(kuku, pi)
            self.assertEqual([result['reward'] for result in report['attempts']], [0, 0])
            self.assertNotIn(SECRET, json.dumps(report))
            journal = kuku / 'live' / 'session-data' / 'p' / 'app' / 'sessions' / 'fixture' / 'events.jsonl'
            journal.parent.mkdir(parents=True)
            journal.write_text(json.dumps({'kind': 'tool.call', 'tool': 'read_output', 'args': {'id': SECRET}}) + '\n')
            rows = [
                {'seconds': 0.25, 'request_bytes': 40, 'body': SECRET},
                {'seconds': 1.25, 'request_bytes': 42, 'timing': {'request_to_first_output_delta_seconds': 0.5}},
                {'seconds': 2.0, 'request_bytes': 45, 'timing': {'request_to_first_output_delta_seconds': 1.0}},
            ]
            write_json(kuku / 'meter.json', {**meter, 'attempts': rows,
                                           'rejections': [{'reason': 'case USD cap', 'private': SECRET}]})
            write_json(pi / 'meter.json', {**meter, 'attempts': [{**rows[1], 'seconds': None}]})
            report = compare(kuku, pi)
            self.assertEqual(report['attempts'][0]['provider_relay'], {
                'admitted_attempts': 3, 'timed_attempts': 3, 'elapsed_seconds': 3.5, 'request_body_bytes': 127,
                'first_output_samples': 2, 'first_output_p50_seconds': 0.75,
            })
            self.assertIsNone(report['attempts'][1]['provider_relay']['elapsed_seconds'])
            self.assertEqual(report['attempts'][0]['tool_calls'], 1)
            self.assertEqual(report['attempts'][0]['tool_call_source'], 'session_journal')
            self.assertEqual(report['attempts'][0]['case_cap_rejections'], 1)
            self.assertNotIn(SECRET, json.dumps(report))
            write_json(pi / 'meter.json', {**meter, 'case_cap_usd': '1.00'})
            with self.assertRaisesRegex(EvidenceError, 'case_cap_usd'):
                compare(kuku, pi)
            write_json(pi / 'meter.json', meter)
            pi_result = json.loads((pi / 'result.json').read_text())
            pi_result['attempts'][0]['resource_requirements']['cpus'] = 2
            write_json(pi / 'result.json', pi_result)
            with self.assertRaisesRegex(EvidenceError, 'container_limits'):
                compare(kuku, pi)


def local_evidence(directory, passed, *, ledger=False):
    live = directory / 'live'
    write_json(live / 'result.json', {
        'mode': 'live', 'passed': passed, 'task_passed': passed, 'reward': int(passed),
        'session_id': SECRET, 'failure_kind': SECRET,
        'runtime_files': {'session_task': SDK_SHA},
        'execution': {'seconds': 3.2, 'timed_out': False, 'command': [SECRET]},
        'verification': {'seconds': 1.3, 'timed_out': False, 'log': SECRET},
        'verifier_summary': {'tests': 3, 'passed': 3 if passed else 0, 'test_names': [SECRET]},
        'sdk_evidence': {'events_path': SECRET},
    })
    usage = {'prompt_tokens': 15, 'completion_tokens': 7, 'prompt_cache_hit_tokens': 10,
             'prompt_cache_miss_tokens': 5, 'reasoning_tokens': 4}
    meter = {
        'model_version': 'DeepSeek-V4.1-Flash-1017', 'model_requested': 'deepseek-flash',
        'upstream': 'api.deepseek.com', 'effort': 'max', 'case_cap_usd': '0.40',
        'limits': {'max_requests': 2, 'token_identity': SECRET},
        'attempts': [{'response_status': 'stop', 'usage': usage, 'max_output_tokens': 8192, 'case': directory.name},
                     {'response_status': SECRET, 'http_status': 502}],
        'rejections': [{'reason': SECRET}], 'amount_includes_unknown_upper_bounds': True,
        'charged_peak_estimate_or_upper_bound_usd': '0.120001',
    }
    if ledger:
        meter['sdk_binary_sha256'] = SDK_SHA
        meter['accounting'] = {
            'usage': usage, 'upstream_requests': 2, 'estimated_requests': 1, 'unknown_requests': 1,
            'token_estimate_usd': '0.000001', 'unknown_usage_upper_bound_usd': '0.12',
            'accounted_usd': '0.120001', 'provider_billed_usd': None,
        }
    write_json(directory / 'meter.json', meter)
    events = live / 'session-data' / 'p' / 'app' / 'sessions' / SECRET / 'events.jsonl'
    events.parent.mkdir(parents=True)
    events.write_text(''.join(json.dumps(event) + '\n' for event in [
        {'kind': 'tool.call', 'args': SECRET},
        {'kind': 'tool.result', 'status': 'error', 'summary': SECRET},
        {'kind': 'tool.result', 'status': 'blocked', 'summary': SECRET},
        {'kind': 'turn.completed'} if passed else {'kind': 'turn.interrupted', 'reason': 'length'},
    ]), encoding='utf-8')


def router_evidence(directory, *overrides):
    local_evidence(directory, True)
    amounts = str(len(overrides) * 5 / 1000)
    rows = [{'request': index, 'upstream_requested': 'baseten', 'status': 'completed', 'http_status': 200,
             'basis': 'provider_reported', 'provider_reported_usd': '0.005', 'provider_cost_source': 'stream_usage',
             'receipt_lookup': 404, 'model_received': ROUTER_MODEL, 'usage': {'input_tokens': 20, 'output_tokens': 10},
             **extra} for index, extra in enumerate(overrides, 1)]
    write_json(directory / 'meter.json', {
        'case': directory.name, 'service': 'openrouter', 'wire': 'openai-responses', 'model_requested': ROUTER_MODEL,
        'upstream': 'baseten', 'upstream_provider': 'BaseTen', 'allow_fallbacks': False, 'attempts': rows,
        'accounting': {'upstream_requests': len(rows), 'provider_reported_requests': len(rows),
                       'provider_reported_usd': amounts, 'provider_billed_usd': amounts, 'reconciled': True,
                       'provider_cost_sources': {'stream_usage': {'requests': len(rows), 'usd': amounts}}},
    })
    ids = [f'gen-{SECRET}-{index}' for index in range(1, len(rows) + 1)]
    (directory / PRIVATE_RECEIPTS).write_text(''.join(
        json.dumps({'request': index, 'id': identifier, 'source': 'header'}) + '\n'
        for index, identifier in enumerate(ids, 1)
    ), encoding='utf-8')
    return ids


def generation_receipt(identifier, **changes):
    return {'id': identifier, 'provider_name': 'BaseTen', 'model': ROUTER_MODEL, 'total_cost': 0.005,
            'generation_time': 2_000, 'latency': 3_500, 'native_tokens_completion': 10, 'unknown': SECRET, **changes}


class FrontierReportTests(unittest.TestCase):
    def test_official_time_per_pass_includes_failed_and_audited_attempts(self):
        attempts = [
            dict(outcome='pass', lookup=0, cost_usd=0.1, run_seconds=10),
            dict(outcome='pass', lookup=1, cost_usd=0.1, run_seconds=20),
            dict(outcome='timeout', lookup=0, cost_usd=0.1, run_seconds=90),
            dict(outcome='infra', lookup=0, cost_usd=0.1, run_seconds=80),
        ]
        self.assertEqual(cohort_efficiency(attempts)['agent_verifier_seconds_per_pass'], 120)
        attempts[1]['run_seconds'] = None
        self.assertIsNone(cohort_efficiency(attempts)['agent_verifier_seconds_per_pass'])

    def test_workspace_script_counts_distinguish_local_js_host_actions_and_missing_receipts(self):
        with tempfile.TemporaryDirectory() as temporary:
            live_root = Path(temporary) / 'live'
            live_root.mkdir()
            live = Path(temporary) / 'live-alias'
            try:
                live.symlink_to(live_root, target_is_directory=True)
            except (OSError, NotImplementedError):
                live = live_root
            path = live / 'session-data/p/app/sessions/sample/events.jsonl'
            path.parent.mkdir(parents=True)

            def counts():
                tools, _ = session_summary(live, {})
                return tools['workspace_script_calls'], tools['workspace_script_host_actions']

            self.assertEqual((None, None), counts())
            with path.open('w', encoding='utf-8') as journal:
                def event(kind, **fields):
                    journal.write(json.dumps({'kind': kind, **fields}) + '\n')
                    journal.flush()

                event('tool.call', tool='run_command')
                self.assertEqual((0, 0), counts())
                event('tool.call', tool='workspace_script', tool_call_id='a')
                event('tool.result', tool_call_id='a', conversation='reviewer', status='ok',
                      structured={'kind': 'workspace_script_result', 'actions': [{'tool': 'read_file'}]})
                self.assertEqual((1, None), counts())
                event('tool.result', tool_call_id='a', status='ok',
                      structured={'kind': 'workspace_script_result', 'actions': []})
                self.assertEqual((1, 0), counts())
                event('tool.call', tool='run_js', tool_call_id='b')
                event('tool.result', tool_call_id='b', status='error',
                      structured={'kind': 'workspace_script_result',
                                  'actions': [{'tool': 'run_command'}, {'tool': 'read_output'}]})
                self.assertEqual((2, 2), counts())

    def test_audit_scoring_cost_and_time(self):
        report = official_report(official_feed())
        task = report['tasks'][CASES[0]]
        self.assertEqual(report['pi_version'], '1.4.0')
        self.assertEqual(task['pi']['passes'], 1)
        self.assertEqual(task['pi']['attempts'], 3)
        self.assertEqual(task['pi']['median_agent_verifier_seconds'], 20)
        self.assertEqual(task['pi']['mean_inference_cost_usd'], 0.1)
        self.assertEqual(task['all_22_including_pi'], {'passes': 84, 'attempts': 87, 'infra_excluded': 1})

    def test_incomplete_or_duplicate_official_data_is_rejected(self):
        for alteration in ('incomplete', 'duplicate'):
            with self.subTest(alteration=alteration):
                feed = official_feed()
                if alteration == 'incomplete':
                    feed['complete'] = False
                else:
                    feed['finished'].append(feed['finished'][0])
                with self.assertRaisesRegex(EvidenceError, 'expected a complete'):
                    official_report(feed)

    def test_local_attempt_sequence_accounting_and_allowlist(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first = root / SECRET / CASES[0]
            another = root / 'first' / CASES[1]
            rerun = root / 'output32k' / CASES[0]
            local_evidence(first, False)
            local_evidence(another, False)
            local_evidence(rerun, True, ledger=True)
            historic_result = rerun / 'live' / 'result.json'
            historic = json.loads(historic_result.read_text())
            historic['sdk_evidence']['limits'] = {'max_steps': 22}
            write_json(historic_result, historic)
            write_json(rerun / 'result.json', {'tree': {'commit': GIT_COMMIT, 'branch': SECRET}})
            report = build_report(official_feed(), [
                [CASES[0], first], [CASES[1], another], [CASES[0], rerun], [CASES[1], another, 'guarded'],
            ])
            one, _, two, four = report['local_attempts']
            self.assertEqual([r['phase'] for r in report['local_attempts']], ['first', 'first', 'rerun', 'rerun'])
            self.assertEqual([r['round'] for r in report['local_attempts']], ['first', 'first', 'output32k', 'guarded'])
            self.assertEqual((one['attempt'], two['attempt'], four['attempt']), (1, 2, 2))
            self.assertFalse(one['result']['passed'])
            self.assertTrue(two['result']['passed'])
            self.assertEqual(two['result']['sdk_execution_seconds'], 3.2)
            self.assertEqual(two['result']['verifier_seconds'], 1.3)
            self.assertEqual(one['result']['terminal_event'], 'turn.interrupted')
            self.assertEqual(one['result']['interrupt_reason'], 'length')
            self.assertEqual(two['result']['terminal_event'], 'turn.completed')
            self.assertEqual(two['provenance'], {'sdk_binary_sha256': SDK_SHA, 'checkout_commit': GIT_COMMIT})
            self.assertEqual(two['configuration']['max_requests'], 2)
            self.assertEqual(two['configuration']['max_output_tokens'], 8192)
            self.assertEqual(two['configuration']['max_steps'], 22)
            self.assertNotIn('max_steps', one['configuration'])
            self.assertEqual(two['tools']['tool_errors'], 1)
            self.assertEqual(two['tools']['non_ok_results'], 2)
            self.assertEqual(two['usage']['finish_reasons'], {'other': 1, 'stop': 1})
            self.assertEqual(two['usage']['reasoning_tokens'], 4)
            self.assertEqual(one['usage']['cache']['first_request']['cache_hit_rate'], 10 / 15)
            self.assertIsNone(one['usage']['cache']['subsequent_requests']['cache_hit_rate'])
            self.assertIsNone(one['cost']['token_estimate_usd'])
            self.assertEqual(two['cost']['token_estimate_usd'], 0.000001)
            self.assertEqual(two['cost']['unknown_usage_upper_bound_usd'], 0.12)
            self.assertIsNone(two['cost']['provider_billed_usd'])
            self.assertNotIn(SECRET, json.dumps(report))
            self.assertNotIn(str(root), json.dumps(report))
            with self.assertRaisesRegex(EvidenceError, 'invalid round label'):
                build_report(official_feed(), [[CASES[0], first, SECRET]])
            with self.assertRaisesRegex(EvidenceError, 'supported case'):
                build_report(official_feed(), [['openssl-selfsigned-cert', first]])
            with self.assertRaisesRegex(EvidenceError, 'local meter request does not match'):
                build_report(official_feed(), [[CASES[2], first]])

    def test_final_provider_failure_uses_same_actor_and_turn(self):
        for error_kind, reason, error_turn, sdk_error_kind, interrupt_reason in (
            ('rate_limited', 'rate_limited', 5, 'rate_limited', 'rate_limited'),
            ('transport', 'transport', 5, 'transport', 'transport'),
            ('transport', SECRET, 4, 'other', 'other'),
            (SECRET, SECRET, 5, 'other', 'other'),
        ):
            with self.subTest(error_kind=error_kind, reason=reason, error_turn=error_turn):
                with tempfile.TemporaryDirectory() as temporary:
                    case = CASES[0]
                    directory = Path(temporary) / case
                    local_evidence(directory, True)
                    live = directory / 'live'
                    events = next((live / 'session-data').glob('p/*/sessions/*/events.jsonl'))
                    records = [
                        {'kind': 'model.error', 'conversation': 'main', 'turn': error_turn,
                         'error_kind': error_kind, 'message': SECRET},
                        {'kind': 'turn.interrupted', 'conversation': 'main', 'turn': 5, 'reason': reason},
                        {'kind': 'model.error', 'conversation': SECRET, 'turn': 5, 'error_kind': 'authentication'},
                        {'kind': 'turn.completed', 'conversation': SECRET, 'turn': 5},
                        {'kind': 'runtime.operation', 'record': {
                            'operation': 'turn_settled', 'address': 'main', 'turn': 5,
                            'status': 'interrupted', 'error': 'provider error: ' + SECRET,
                        }},
                    ]
                    events.write_text(''.join(json.dumps(event) + '\n' for event in records), encoding='utf-8')
                    attempt = build_report(official_feed(), [[case, directory]])['local_attempts'][0]
                    result = attempt['result']
                    self.assertTrue(result['task_passed'])
                    self.assertEqual(result['terminal_event'], 'turn.interrupted')
                    self.assertEqual(result['interrupt_reason'], interrupt_reason)
                    self.assertTrue(result['durable_turn_settled'])
                    self.assertEqual(result['sdk_error_kind'], sdk_error_kind)
                    self.assertNotIn(SECRET, json.dumps(attempt))

    def test_openrouter_reports_partial_receipts_and_paired_cache_without_assuming_fallback(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            case = CASES[0]
            directory = root / 'first' / case
            local_evidence(directory, False)
            model = 'deepseek/deepseek-v4.1-flash-1017'
            canonical = 'deepseek/deepseek-v4.1-flash-20261017'
            meter = {
                'case': case, 'service': 'openrouter', 'wire': 'openai-responses',
                'model_requested': model, 'catalog_upstream_model': canonical,
                'upstream': 'deepseek', 'upstream_provider': 'DeepSeek', 'allow_fallbacks': False,
                'limits': {'max_output_tokens': 131072, 'max_request_bytes': 262144},
                'price_usd_per_million_tokens': {'prompt': '0.14', 'cache_read': '0.01', 'cache_write': '0E-8',
                                                'completion': '0.28'},
                'accounting': {'upstream_requests': 3, 'provider_reported_requests': 1,
                               'provider_billed_usd': None, 'provider_reported_usd': '0.023',
                               'unknown_requests': 2, 'unknown_usage_upper_bound_usd': '0.40',
                               'accounted_usd': '0.423'},
                'attempts': [
                    {'basis': 'no_upstream_charge', 'status': 'connection_failed'},
                    {'upstream_requested': 'deepseek', 'reported_upstream': 'DeepSeek', 'model_received': canonical,
                     'receipt_matches_route': True, 'usage': {'input_tokens': 20, 'output_tokens': 10},
                     'timing': {'request_to_first_output_delta_seconds': 1.5, 'request_to_first_reasoning_delta_seconds': 1.5,
                                'openrouter_receipt_latency_ms': 3500, 'openrouter_receipt_generation_time_ms': 2000,
                                'openrouter_receipt_completion_tokens': 10, 'openrouter_receipt_completion_tokens_per_second': 5,
                                'stream_completion_tokens': 10, 'request_to_stream_end_seconds': 3.5}},
                    {'upstream_requested': 'deepseek',
                     'usage': {'input_tokens': 30, 'output_tokens': 15, 'cache_read_input_tokens': 0,
                               'output_tokens_details': {'reasoning_tokens': 2}},
                     'timing': {'request_to_first_output_delta_seconds': 2.5, 'request_to_first_tool_argument_delta_seconds': 2.5,
                                'openrouter_receipt_latency_ms': 123, 'unknown_metric': SECRET}},
                    {'upstream_requested': 'deepseek',
                     'usage': {'input_tokens': 50, 'output_tokens': 6, 'cache_read_input_tokens': 30}},
                ],
            }
            write_json(directory / 'meter.json', meter)
            report = build_report(official_feed(), [[case, directory]])['local_attempts'][0]
            self.assertEqual(report['configuration']['model_requested'], model)
            self.assertEqual(report['configuration']['wire'], 'openai-responses')
            self.assertEqual(report['configuration']['max_output_tokens'], 131072)
            self.assertEqual(report['configuration']['price_usd_per_million_tokens'],
                             {'input': 0.14, 'cached_input': 0.01, 'output': 0.28, 'cache_write': 0})
            self.assertFalse(report['routing']['allow_fallbacks'])
            self.assertEqual(report['routing']['confirmed_upstream'], 'DeepSeek')
            self.assertIsNone(report['routing']['fallback_observed'])
            self.assertEqual(report['routing']['provider_receipt_coverage'], 'partial')
            self.assertEqual(report['routing']['requests_with_observed_pin'], 3)
            self.assertEqual(report['routing']['reported_provider_request_counts'], {'DeepSeek': 1})
            self.assertIsNone(report['cost']['provider_billed_usd'])
            self.assertEqual(report['cost']['provider_reported_usd'], 0.023)
            self.assertEqual(report['cost']['provider_reported_coverage'], 'partial')
            self.assertEqual(report['usage']['connection_failures_before_dispatch'], 1)
            cache = report['usage']['cache']
            self.assertIsNone(cache['first_request']['cache_read_tokens'])
            self.assertIsNone(cache['first_request']['cache_hit_rate'])
            self.assertEqual(cache['all']['observed_input_tokens'], 100)
            self.assertEqual(cache['all']['input_tokens_with_cache_observation'], 80)
            self.assertEqual(cache['all']['cache_read_tokens'], 30)
            self.assertEqual(cache['subsequent_requests']['cache_hit_rate'], 30 / 80)
            self.assertEqual(report['usage']['reasoning_tokens'], 2)
            performance = report['performance']
            first_output = performance['proxy']['request_to_first_output_delta_seconds']
            self.assertEqual((first_output['requests'], first_output['measured_requests'], first_output['p50']), (3, 2, 2.0))
            self.assertEqual(performance['proxy']['request_to_first_tool_argument_delta_seconds']['p50'], 2.5)
            self.assertEqual(performance['openrouter_receipt']['openrouter_receipt_latency_ms']['p50'], 3500)
            self.assertEqual(performance['openrouter_receipt']['openrouter_receipt_completion_tokens_per_second']['p50'], 5)
            self.assertEqual(performance['pooled_throughput']['openrouter_generation']['completion_tokens'], 10)
            self.assertEqual(performance['pooled_throughput']['openrouter_generation']['elapsed_seconds'], 2)
            self.assertEqual(performance['pooled_throughput']['proxy_stream']['tokens_per_second'], 5)
            self.assertIsNone(performance['provider_native_ttft']['p50'])
            self.assertIsNone(report['resources']['harness']['memory'])
            meter['attempts'][2].update(provider_matches_pin=False, receipt_matches_route=False, route_mismatch=True)
            meter['attempts'][3].update(reported_upstream=SECRET, receipt_matches_route=False)
            meter['accounting']['reconciled'] = False
            write_json(directory / 'meter.json', meter)
            report = build_report(official_feed(), [[case, directory]])['local_attempts'][0]
            self.assertTrue(report['routing']['fallback_observed'])
            self.assertEqual(report['routing']['requests_with_route_mismatch'], 1)
            self.assertEqual(report['routing']['provider_identity_coverage'], 'partial')
            self.assertEqual(report['usage']['requests_without_usage_due_to_route_mismatch'], 1)
            self.assertEqual(report['result']['failure_kind'], 'provider_boundary')
            self.assertNotIn(SECRET, json.dumps(report))

    def test_pre_dispatch_byte_limit_and_raw_reward_are_separate(self):
        with tempfile.TemporaryDirectory() as temporary:
            case = CASES[0]
            directory = Path(temporary) / 'guarded' / case
            local_evidence(directory, False)
            live = directory / 'live'
            result = json.loads((live / 'result.json').read_text())
            write_json(live / 'result.json', {**result, 'reward': 1.0, 'failure_kind': 'runtime', 'resources': {
                'harness_memory': {'peak_rss_kib': 31415, 'mean_rss_kib': 30000.4, 'samples': 11,
                                   'observed_seconds': 10, 'window_seconds': 12.5, 'coverage_ratio': 0.8},
                'harness_cpu_seconds': 2.3,
                'harness_self_usage': {'peak_rss_kib': 32000, 'user_cpu_seconds': 2, 'system_cpu_seconds': 0.3,
                                       'input_blocks': 6, 'output_blocks': 2},
                'process_io': {'rchar': 20, 'syscr': 2, 'unknown_field': SECRET},
                'process_sampling_errors': 3, 'container_sampling_errors': 1,
                'container_samples': 9, 'container_snapshots': [
                    {'BlockIO': '135kB / 1.39GB'}, {'BlockIO': '0B / 32.60MB', 'MemUsage': SECRET},
                    {'BlockIO': f'0B / {SECRET}'},
                ],
            }})
            events = next((live / 'session-data').glob('p/*/sessions/*/events.jsonl'))
            events.write_text(json.dumps({'kind': 'runtime.operation', 'record': {
                'operation': 'turn_settled', 'address': 'main', 'status': 'interrupted',
                'error': SDK_REQUEST_BYTES_ERROR, 'output': SECRET,
            }}) + '\n', encoding='utf-8')
            attempt = build_report(official_feed(), [[case, directory]])['local_attempts'][0]
            self.assertFalse(attempt['result']['task_passed'])
            self.assertEqual(attempt['result']['reward'], 1.0)
            self.assertIsNone(attempt['result']['terminal_event'])
            self.assertTrue(attempt['result']['durable_turn_settled'])
            self.assertEqual(attempt['result']['settled_status'], 'interrupted')
            self.assertEqual(attempt['result']['sdk_error_kind'], 'invalid_argument')
            self.assertEqual(attempt['result']['sdk_error_detail'], 'request_bytes_limit')
            self.assertEqual(attempt['resources']['harness']['memory']['mean_rss_kib'], 30000.4)
            self.assertEqual(attempt['resources']['harness']['memory']['coverage_ratio'], 0.8)
            self.assertEqual(attempt['resources']['harness']['cpu_seconds'], 2.3)
            self.assertEqual(attempt['resources']['harness']['self_usage']['input_blocks'], 6)
            self.assertEqual(attempt['resources']['process_including_waited_children']['io']['syscr'], 2)
            self.assertIsNone(attempt['resources']['process_including_waited_children']['io']['read_bytes'])
            self.assertEqual(attempt['resources']['process_including_waited_children']['sampling_errors'], 3)
            self.assertEqual(attempt['resources']['container']['samples'], 9)
            self.assertEqual(attempt['resources']['container']['retained_snapshot_count'], 3)
            self.assertEqual(attempt['resources']['container']['sampling_errors'], 1)
            self.assertEqual(attempt['resources']['container']['block_io'], {
                'read_bytes': 0, 'write_bytes': 32_600_000, 'display': '0B / 32.60MB',
            })
            self.assertNotIn(SDK_REQUEST_BYTES_ERROR, json.dumps(attempt))
            self.assertNotIn(SECRET, json.dumps(attempt))

    def test_container_io_supports_decimal_exponents_and_binary_units(self):
        summary = resource_summary({'container_snapshots': [{'BlockIO': '1e+03kB / 1.2 KiB'}, {'BlockIO': '-- / --'}]})
        self.assertEqual(summary['container']['block_io'], {
            'read_bytes': 1_000_000, 'write_bytes': 1229, 'display': '1e+03kB / 1.2KiB',
        })
        for sample in ('1QB / 2GB', '0B / --', '-1MB / 2MB', '0B / 1MB / 2MB', None):
            with self.subTest(sample=sample):
                summary = resource_summary({'container_snapshots': [{'BlockIO': sample}]})
                self.assertIsNone(summary['container']['block_io'])

    def test_resource_sampler_zero_errors_and_unknown_io_remain_distinct(self):
        historic = resource_summary({'harness_memory': {'peak_rss_kib': 35504, 'mean_rss_kib': 35866.97},
                                     'harness_self_usage': {'peak_rss_kib': 35504},
                                     'sdk_peak_rss_kib': 36436, 'sdk_peak_rss_high_water_kib': 36436})
        self.assertEqual(historic['harness']['memory']['peak_rss_kib'], 36436)
        self.assertEqual(historic['harness']['self_usage']['peak_rss_kib'], 35504)
        split = {'container_scope': 'sdk_and_tools', 'process_io_scope': SECRET,
                 'harness_self_usage': {'input_blocks': 0, 'output_blocks': 0}}
        complete = resource_summary({**split, 'capture_incomplete': False})
        self.assertEqual(complete['process_including_waited_children'], {'io': None, 'sampling_errors': 0})
        self.assertEqual(complete['container']['sampling_errors'], 0)
        self.assertIsNone(complete['container']['block_io'])
        self.assertEqual(complete['harness']['self_usage']['input_blocks'], 0)
        incomplete = resource_summary({**split, 'capture_incomplete': True})
        self.assertIsNone(incomplete['container']['sampling_errors'])
        self.assertIsNone(incomplete['process_including_waited_children']['sampling_errors'])
        legacy = resource_summary({'sampling_errors': 6})
        self.assertIsNone(legacy['container']['sampling_errors'])
        self.assertIsNone(legacy['process_including_waited_children']['sampling_errors'])
        self.assertEqual(legacy['legacy_combined_sampling_errors'], 6)

    def test_delayed_receipt_is_bound_to_meter_and_reused_without_recounting(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            case = CASES[0]
            directory = root / case
            delayed_id, _ = router_evidence(directory, {}, {
                'receipt_lookup': 200, 'receipt_matches_route': True, 'reported_upstream': 'BaseTen',
                'timing': {'openrouter_receipt_generation_time_ms': 3_000, 'openrouter_receipt_completion_tokens': 30},
            })
            attempt = [[case, directory]]
            meter_path = directory / 'meter.json'
            meter_raw = meter_path.read_bytes()
            with patch('frontier_report.upstream_json', return_value=(200, generation_receipt(delayed_id))) as get:
                report = build_report(official_feed(), attempt, receipt_key=SECRET)
                get.assert_called_once_with('openrouter.ai', '/api/v1/generation?id=' + delayed_id, SECRET)
            local = report['local_attempts'][0]
            routing = local['routing']
            self.assertEqual(routing['confirmed_upstream'], 'Baseten')
            self.assertEqual(routing['requests_with_matching_receipt'], 2)
            self.assertEqual(routing['requests_with_live_matching_receipt'], 1)
            self.assertEqual(routing['requests_with_delayed_matching_receipt'], 1)
            self.assertEqual(routing['live_receipt_lookup_counts'], {'200': 1, '404': 1})
            self.assertEqual(local['cost']['provider_cost_sources'], {'stream_usage': {'requests': 2, 'usd': 0.01}})
            perf = local['performance']
            self.assertEqual(perf['openrouter_receipt']['openrouter_receipt_latency_ms']['p50'], 3_500)
            self.assertEqual(perf['pooled_throughput']['openrouter_generation']['tokens_per_second'], 8)
            self.assertEqual(perf['provider_native_ttft']['measured_requests'], 0)
            self.assertEqual(meter_path.read_bytes(), meter_raw)
            sidecar = directory / DELAYED_RECEIPTS
            sidecar_raw = sidecar.read_text()
            self.assertEqual(json.loads(sidecar_raw)['meter_sha256'], sha256(meter_raw).hexdigest())
            self.assertNotIn(SECRET, sidecar_raw)
            self.assertNotIn(str(root), json.dumps(report))
            self.assertNotIn(SECRET, json.dumps(report))
            with patch('frontier_report.upstream_json') as get:
                self.assertEqual(build_report(official_feed(), attempt, receipt_key=SECRET), report)
                self.assertEqual(build_report(official_feed(), attempt), report)
                get.assert_not_called()
            self.assertEqual(sidecar.read_text(), sidecar_raw)
            meter_path.write_bytes(meter_raw + b'\n')
            routing = build_report(official_feed(), attempt)['local_attempts'][0]['routing']
            self.assertEqual(routing['requests_with_live_matching_receipt'], 1)
            self.assertEqual(routing['requests_with_delayed_matching_receipt'], 0)

    def test_delayed_receipt_lookup_failures_and_mismatches_leave_verification_unchanged(self):
        with tempfile.TemporaryDirectory() as temporary:
            case = CASES[0]
            directory = Path(temporary) / case
            ids = router_evidence(directory, {}, {}, {}, {}, {})
            replies = dict(zip(ids, [(404, None), (200, generation_receipt(ids[1], total_cost=0.02)),
                                     (200, generation_receipt(ids[2], provider_name='DeepSeek')),
                                     (200, generation_receipt(ids[3], model='unexpected')),
                                     (200, generation_receipt(ids[0]))], strict=True))
            with patch('frontier_report.upstream_json', side_effect=lambda host, path, key: replies[path.split('=', 1)[1]]) as get:
                local = build_report(official_feed(), [[case, directory]], receipt_key=SECRET)['local_attempts'][0]
            self.assertEqual(get.call_count, 5)
            self.assertTrue(local['result']['passed'])
            self.assertIsNone(local['routing']['confirmed_upstream'])
            self.assertEqual(local['routing']['live_receipt_lookup_counts'], {'404': 5})
            self.assertEqual(local['routing']['requests_with_delayed_matching_receipt'], 0)
            self.assertEqual(local['performance']['pooled_throughput']['openrouter_generation']['measured_requests'], 0)
            self.assertFalse((directory / DELAYED_RECEIPTS).exists())

    def test_refresh_receipts_cli_limits_gets_to_selected_attempts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            directory = root / CASES[0]
            selected_id, = router_evidence(directory, {})
            router_evidence(root / CASES[1], {})
            keyfile = root / 'credential.env'
            keyfile.write_text(f'KUKU_TEST_OPENROUTER_API_KEY={SECRET}\n')
            source = root / 'feed.json'
            output = root / 'public.json'
            write_json(source, official_feed())
            argv = ['frontier_report', '--official-live', str(source), '--refresh-receipts', str(keyfile),
                    '--attempt', CASES[0], str(directory), '--output', str(output)]
            with patch('sys.argv', argv), patch('frontier_report.upstream_json', return_value=(404, None)) as get:
                main()
            get.assert_called_once_with('openrouter.ai', '/api/v1/generation?id=' + selected_id, SECRET)
            self.assertNotIn(SECRET, output.read_text())
            self.assertNotIn(str(root), output.read_text())

    def test_cli_writes_report_to_requested_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / 'feed.json'
            output = root / 'public.json'
            write_json(source, official_feed())
            process = subprocess.run([
                sys.executable, str(Path(__file__).with_name('frontier_report.py')),
                '--official-live', str(source), '--output', str(output),
            ], check=True, capture_output=True, text=True)
            self.assertEqual(process.stdout, '')
            report = json.loads(output.read_text(encoding='utf-8'))
            self.assertEqual(report['schema_version'], 1)
            self.assertEqual(report['local_attempts'], [])
            self.assertEqual(report['official']['snapshot_sha256'], sha256(source.read_bytes()).hexdigest())


if __name__ == '__main__':
    unittest.main()
