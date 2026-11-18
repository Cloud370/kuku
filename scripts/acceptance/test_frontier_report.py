import json
from hashlib import sha256
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from frontier_report import (CASES, DATASET, MODEL_ID, SDK_REQUEST_BYTES_ERROR, EvidenceError,
                             build_report, official_report)


SECRET = 'do-not-publish-local-identity'
SDK_SHA = '1' * 64
GIT_COMMIT = '3' * 40


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


class FrontierReportTests(unittest.TestCase):
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
                'container_samples': 9, 'container_snapshots': [SECRET],
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
            self.assertEqual(attempt['resources']['container']['samples'], 9)
            self.assertNotIn(SDK_REQUEST_BYTES_ERROR, json.dumps(attempt))
            self.assertNotIn(SECRET, json.dumps(attempt))

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
