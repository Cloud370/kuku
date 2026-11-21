#!/usr/bin/env python3
"""Build a public FrontierHarness comparison from local evidence files."""

import argparse
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor
from decimal import Decimal, ROUND_HALF_UP
from hashlib import sha256
from http.client import HTTPException
import json
import math
from pathlib import Path
import re
from statistics import median
import sys
from urllib.parse import urlencode

from benchmark_resources import EXTERNAL_PROCESS_IO, FINAL_PROCESS_IO, complete_process_io, peak_rss_kib
from evidence import EVENT_LINE_BYTES, EVENT_RECORD_LIMIT
from paid_model import upstream_json, usd
from run import load_secrets, locked, write_json


CASES = ('log-summary-date-ranges', 'cancel-async-tasks', 'regex-log', 'large-scale-text-editing')
DATASET = 'frontier-harness-eval@v1.1.0'
TASK_COMMIT = '7131e4375048a0e408a8fb404b5f499d726b695b'
MODEL_ID = 'deepseek-v4.1-flash-1017'
MODEL_VERSION = 'DeepSeek-V4.1-Flash-1017'
OPENROUTER_MODELS = {'deepseek/deepseek-v4.1-flash-1017', 'deepseek/deepseek-v4.1-flash-20261017'}
LOCAL_MODELS = OPENROUTER_MODELS | {'deepseek-flash', MODEL_VERSION, f'deepseek-ai/{MODEL_VERSION}'}
PROVIDERS = {'deepseek': 'DeepSeek', 'baseten': 'Baseten', 'api.deepseek.com': 'api.deepseek.com'}
EFFORTS = {'none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'}
FINISH_REASONS = {'stop', 'length', 'tool_calls', 'function_call', 'content_filter', 'error',
                  'response.completed', 'response.incomplete', 'response.failed', 'insufficient_system_resource'}
FAILURE_KINDS = {'runtime', 'infrastructure', 'sdk_evidence', 'artifact_retention', 'agent_task', 'cleanup',
                 'provider_boundary', 'resource_evidence', 'agent_evidence'}
TOOL_STATUSES = {'ok', 'error', 'blocked', 'cancelled'}
TURN_EVENTS = {'turn.completed', 'turn.interrupted', 'turn.cancelled'}
PROVIDER_ERROR_KINDS = {'authentication', 'rate_limited', 'context_too_large', 'invalid_request',
                        'provider_unavailable', 'transport', 'internal', 'unknown'}
INTERRUPT_REASONS = FINISH_REASONS | PROVIDER_ERROR_KINDS | {'cancelled', 'canceled', 'timeout', 'step_limit', 'request_limit'}
HARNESSES = 22
ATTEMPTS_PER_TASK = 4
ROUND_LABEL = re.compile(r'(?:first|rerun|round|guidance|guarded)(?:-[1-9][0-9]{0,2})?|(?:output|context)[1-9][0-9]{0,3}k')
VERSION = re.compile(r'[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}')
DECIMAL = re.compile(r'[0-9]{1,10}(?:\.[0-9]{1,12})?(?:[Ee][+-]?[0-9]{1,3})?')
USAGE_FIELDS = {
    'input_tokens': ('input_tokens', 'prompt_tokens'),
    'output_tokens': ('output_tokens', 'completion_tokens'),
    'cached_input_tokens': ('cache_read_input_tokens', 'prompt_cache_hit_tokens'),
    'uncached_input_tokens': ('uncached_input_tokens', 'prompt_cache_miss_tokens'),
    'cache_write_input_tokens': ('cache_write_input_tokens', 'cache_creation_input_tokens'),
}
SETTLED_STATUSES = {'idle', 'running', 'waiting', 'stopped', 'interrupted'}
SDK_REQUEST_BYTES_ERROR = 'invalid argument: prepared model request exceeds the configured byte limit'
TEST_FIELDS = ('tests', 'passed', 'failed', 'skipped', 'pending', 'other')
MEMORY_FIELDS = ('peak_rss_kib', 'mean_rss_kib', 'samples', 'observed_seconds', 'window_seconds', 'coverage_ratio')
SELF_USAGE_FIELDS = ('user_cpu_seconds', 'system_cpu_seconds', 'peak_rss_kib', 'input_blocks', 'output_blocks')
IO_FIELDS = ('rchar', 'wchar', 'read_bytes', 'write_bytes', 'syscr', 'syscw', 'cancelled_write_bytes')
BLOCK_IO_SIZE = re.compile(r'([0-9]{1,10}(?:\.[0-9]{1,12})?(?:[eE][+-]?[0-9]{1,2})?)[ \t]*([A-Za-z]{1,3})')
BYTE_UNIT_SCALES = {
    'B': 1, 'kB': 1000,
    **{f'{prefix}B': 1000**power for power, prefix in enumerate('KMGTPEZY', 1)},
    **{f'{prefix}iB': 1024**power for power, prefix in enumerate('KMGTPEZY', 1)},
}
PROXY_TIMINGS = {
    'request_to_response_headers_seconds': 'seconds',
    'request_to_first_body_byte_seconds': 'seconds',
    'request_to_first_output_delta_seconds': 'seconds',
    'request_to_first_text_delta_seconds': 'seconds',
    'request_to_first_reasoning_delta_seconds': 'seconds',
    'request_to_first_tool_argument_delta_seconds': 'seconds',
    'request_to_last_output_delta_seconds': 'seconds',
    'request_to_stream_end_seconds': 'seconds',
    'completion_tokens_per_stream_second': 'tokens_per_second',
    'completion_tokens_per_end_to_end_second': 'tokens_per_second',
}
ROUTER_TIMINGS = {
    'openrouter_receipt_latency_ms': 'milliseconds',
    'openrouter_receipt_generation_time_ms': 'milliseconds',
    'openrouter_receipt_completion_tokens_per_second': 'tokens_per_second',
}
PRIVATE_RECEIPTS = '.frontier-receipts.jsonl'
DELAYED_RECEIPTS = 'delayed-receipts.json'
GENERATION_ID = re.compile(r'(?:gen-|resp[_-])[A-Za-z0-9_-]{1,152}')
RECEIPT_LOOKUP_RESULTS = {'missing_generation_id', 'response_id_only', 'invalid_response'}


class EvidenceError(ValueError):
    pass


def object_value(value):
    return value if isinstance(value, dict) else {}


def allowed(value, choices):
    return value if isinstance(value, str) and value in choices else None


def count(value):
    return value if type(value) is int and value >= 0 else None


def number(value):
    return value if type(value) in (int, float) and 0 <= value < math.inf else None


def boolean(value):
    return value if type(value) is bool else None


def money(value):
    if isinstance(value, str) and DECIMAL.fullmatch(value):
        return number(float(Decimal(value)))
    return number(value)


def hex_digest(value, length):
    return value if isinstance(value, str) and re.fullmatch(rf'[0-9a-f]{{{length}}}', value) else None


def load_object(path, description):
    try:
        raw = path.read_bytes()
        result = json.loads(raw)
    except (OSError, UnicodeError, ValueError) as error:
        raise EvidenceError(f'cannot read {description}') from error
    if not isinstance(result, dict):
        raise EvidenceError(f'{description} must contain a JSON object')
    return result, raw


def read_object(path, description):
    return load_object(path, description)[0]


def average(values):
    present = [Decimal(str(value)) for value in values if value is not None]
    return float(sum(present) / len(present)) if present else None


def middle(values):
    present = [value for value in values if value is not None]
    return median(present) if present else None


def scored_rows(rows):
    return [row for row in rows if row['outcome'] != 'infra']


def official_counts(rows):
    scored = scored_rows(rows)
    return {
        'passes': sum(row['outcome'] == 'pass' and not row['lookup'] for row in scored),
        'attempts': len(scored),
        'infra_excluded': len(rows) - len(scored),
    }


def cohort_efficiency(rows):
    counts = official_counts(rows)
    scored = scored_rows(rows)
    costs = [number(row['cost_usd']) for row in scored]
    seconds = [number(row['run_seconds']) for row in scored]
    complete_cost = bool(scored) and all(value is not None for value in costs)
    complete_time = bool(scored) and all(value is not None for value in seconds)
    return {
        **counts,
        'mean_inference_cost_usd': average(costs),
        'inference_usd_per_pass': round(sum(costs) / counts['passes'], 8) if complete_cost and counts['passes'] else None,
        'priced_attempts': sum(value is not None for value in costs),
        'median_agent_verifier_seconds': middle(seconds),
        'mean_agent_verifier_seconds': average(seconds),
        'agent_verifier_seconds_per_pass': round(sum(seconds) / counts['passes'], 8) if complete_time and counts['passes'] else None,
        'timed_attempts': sum(value is not None for value in seconds),
    }


def official_report(feed):
    try:
        return parse_official(feed)
    except (KeyError, TypeError, ValueError, IndexError, StopIteration, OverflowError) as error:
        raise EvidenceError('expected a complete FrontierHarness v1.1.0 /api/live snapshot') from error


def parse_official(feed):
    if (feed['settings']['dataset'] != DATASET or feed['complete'] is not True
            or feed['attempts'] != ATTEMPTS_PER_TASK or feed['running']):
        raise ValueError('incomplete or unexpected feed')
    harnesses = feed['harnesses']
    if len(harnesses) != HARNESSES or len({harness['id'] for harness in harnesses}) != HARNESSES:
        raise ValueError('unexpected harness coverage')
    pi_index = next(index for index, harness in enumerate(harnesses) if harness['id'] == 'pi')
    version = harnesses[pi_index]['version']
    if not VERSION.fullmatch(version):
        raise ValueError('unexpected Pi version')
    model_index = next(index for index, model in enumerate(feed['models']) if model['id'] == MODEL_ID)
    model = feed['models'][model_index]
    if model['provider'] != 'baseten' or model['effort'] != 'max':
        raise ValueError('unexpected official model configuration')
    task_indices = {index: task['name'] for index, task in enumerate(feed['tasks']) if task['name'] in CASES}
    fields = {key: feed['finished_fields'].index(key) for key in
              ('task', 'model', 'harness', 'attempt', 'outcome', 'lookup', 'cost_usd', 'run_seconds')}
    selected = defaultdict(dict)
    for row in feed['finished']:
        if row[fields['model']] != model_index or row[fields['task']] not in task_indices:
            continue
        record = {key: row[index] for key, index in fields.items()}
        key = (task_indices[record['task']], record['harness'])
        attempt = record['attempt']
        if attempt in selected[key]:
            raise ValueError('duplicate attempt')
        outcome = feed['outcomes'][record['outcome']]
        if outcome not in {'pass', 'fail', 'timeout', 'infra'} or record['lookup'] not in {0, 1, 2}:
            raise ValueError('unrecognized trial outcome')
        record['outcome'] = outcome
        selected[key][attempt] = record
    task_results = {}
    for case in CASES:
        groups = [selected[case, index] for index in range(HARNESSES)]
        if any(set(group) != set(range(1, ATTEMPTS_PER_TASK + 1)) for group in groups):
            raise ValueError('missing selected attempts')
        pi_rows = list(groups[pi_index].values())
        task_results[case] = {
            'pi': cohort_efficiency(pi_rows),
            'all_22_including_pi': official_counts([row for group in groups for row in group.values()]),
        }
    peers = []
    for index, harness in enumerate(harnesses):
        by_case = {case: list(selected[case, index].values()) for case in CASES}
        peers.append({
            'id': harness['id'], 'label': harness.get('label', harness['id']), 'version': harness.get('version'),
            'sample': cohort_efficiency([row for rows in by_case.values() for row in rows]),
            'by_case': {case: cohort_efficiency(rows) for case, rows in by_case.items()},
        })
    return {
        'dataset': DATASET,
        'source': 'https://frontierharness.org/api/live',
        'task_commit': TASK_COMMIT,
        'pi_version': version,
        'model': MODEL_VERSION,
        'provider': 'baseten',
        'effort': 'max',
        'attempts_per_task_and_harness': ATTEMPTS_PER_TASK,
        'inference_usd_per_million_tokens': {key: number(model['pricing'][key]) for key in ('input', 'cached_input', 'output')},
        'tasks': task_results,
        'harness_comparison': peers,
    }


def token_field(usage, names):
    for name in names:
        if (value := count(usage.get(name))) is not None:
            return value
    return None


def request_usage(row):
    usage = object_value(row.get('usage'))
    values = {name: token_field(usage, keys) for name, keys in USAGE_FIELDS.items()}
    reasoning_fields = object_value(usage.get('output_tokens_details') or usage.get('completion_tokens_details'))
    cache_fields = object_value(usage.get('input_tokens_details'))
    if values['cached_input_tokens'] is None:
        values['cached_input_tokens'] = token_field(cache_fields, ('cached_tokens',))
    values['reasoning_tokens'] = token_field(usage, ('reasoning_tokens',))
    if values['reasoning_tokens'] is None:
        values['reasoning_tokens'] = token_field(reasoning_fields, ('reasoning_tokens',))
    return values


def sum_observed(values):
    known = [value for value in values if value is not None]
    return sum(known) if known else None


def coverage(observed, total):
    if total == 0:
        return 'not_applicable'
    if observed == 0 or observed is None:
        return 'unknown'
    return 'complete' if observed == total else 'partial'


def cache_observation(usages):
    pairs = [(tokens, cached) for usage in usages
             if (tokens := usage['input_tokens']) is not None
             and (cached := usage['cached_input_tokens']) is not None and cached <= tokens]
    paired_input = sum_observed(tokens for tokens, _ in pairs)
    paired_cached = sum_observed(cached for _, cached in pairs)
    return {
        'requests': len(usages),
        'requests_with_input': sum(usage['input_tokens'] is not None for usage in usages),
        'requests_with_cache_and_input': len(pairs),
        'coverage': coverage(len(pairs), len(usages)),
        'observed_input_tokens': sum_observed(usage['input_tokens'] for usage in usages),
        'input_tokens_with_cache_observation': paired_input,
        'cache_read_tokens': paired_cached,
        'cache_hit_rate': paired_cached / paired_input if paired_input else None,
    }


def provider_name(value):
    return PROVIDERS.get(value.lower()) if isinstance(value, str) else None


def exact_cost(value):
    if type(value) not in (int, float, str):
        return None
    try:
        return usd(value)
    except (ArithmeticError, ValueError):
        return None


def matched_receipt(meter, row, receipt):
    pin = provider_name(meter.get('upstream'))
    provider = provider_name(receipt.get('provider'))
    models = {meter.get(field) for field in ('model_requested', 'catalog_upstream_model')
              if isinstance(meter.get(field), str)} & OPENROUTER_MODELS
    model = allowed(receipt.get('model'), models)
    cost = exact_cost(receipt.get('cost_usd'))
    if not model or not pin or provider != pin or cost is None or cost != exact_cost(row.get('provider_reported_usd')):
        return None
    latency = number(receipt.get('latency_ms'))
    generation = number(receipt.get('generation_time_ms'))
    tokens = count(receipt.get('completion_tokens'))
    return {'request': row['request'], 'provider': provider, 'model': model, 'cost_usd': str(cost),
            'latency_ms': latency if latency is not None and latency <= 10**9 else None,
            'generation_time_ms': generation if generation is not None and 1 <= generation <= 10**9 else None,
            'completion_tokens': tokens if tokens is not None and tokens <= 10**9 else None}


def cached_receipts(path, meter, digest, rows):
    try:
        cache = read_object(path, 'delayed receipt evidence')
    except EvidenceError:
        return {}
    entries = cache.get('receipts')
    if cache.get('schema_version') != 1 or cache.get('meter_sha256') != digest or not isinstance(entries, list):
        return {}
    resolved = {}
    for entry in entries:
        receipt = object_value(entry)
        request = count(receipt.get('request'))
        if request in rows and (matched := matched_receipt(meter, rows[request], receipt)) is not None:
            resolved[request] = matched
    return resolved


def private_generation_ids(path):
    candidates = defaultdict(set)
    try:
        with path.open(encoding='utf-8') as handle:
            for line in handle:
                try:
                    row = object_value(json.loads(line))
                except ValueError:
                    continue
                request = count(row.get('request'))
                identifier = row.get('id')
                if request and isinstance(identifier, str) and GENERATION_ID.fullmatch(identifier):
                    candidates[request].add(identifier)
    except (OSError, UnicodeError):
        return {}
    occurrences = Counter(identifier for values in candidates.values() for identifier in values)
    return {request: identifier for request, values in candidates.items() if len(values) == 1
            for identifier in values if occurrences[identifier] == 1}


def fetch_receipt(meter, row, identifier, key):
    try:
        status, data = upstream_json('openrouter.ai', '/api/v1/generation?' + urlencode({'id': identifier}), key)
    except (OSError, HTTPException, KeyError, TypeError, ValueError):
        return None
    if status != 200 or not isinstance(data, dict) or data.get('id') != identifier:
        return None
    return matched_receipt(meter, row, {
        'provider': data.get('provider_name'), 'model': data.get('model'), 'cost_usd': data.get('total_cost'),
        'latency_ms': data.get('latency'), 'generation_time_ms': data.get('generation_time'),
        'completion_tokens': data.get('native_tokens_completion'),
    })


def delayed_receipts(directory, meter, raw, rows, key):
    if meter.get('service') != 'openrouter' or meter.get('allow_fallbacks') is not False:
        return {}
    pin = allowed(meter.get('upstream'), PROVIDERS)
    if not pin or provider_name(meter.get('upstream_provider')) not in (None, provider_name(pin)):
        return {}
    requests = Counter(count(row.get('request')) for row in rows)
    eligible = {}
    for row in rows:
        request = count(row.get('request'))
        reported = row.get('reported_upstream')
        if (request and requests[request] == 1 and row.get('upstream_requested') == pin
                and row.get('basis') == 'provider_reported' and row.get('route_mismatch') is not True
                and row.get('provider_matches_pin') is not False and row.get('receipt_matches_route') is None
                and (not reported or provider_name(reported) == provider_name(pin))
                and exact_cost(row.get('provider_reported_usd')) is not None):
            eligible[request] = row
    if not eligible:
        return {}
    digest = sha256(raw).hexdigest()
    cache_path = directory / DELAYED_RECEIPTS
    cache = cached_receipts(cache_path, meter, digest, eligible)
    if not key or len(cache) == len(eligible):
        return cache
    identifiers = private_generation_ids(directory / PRIVATE_RECEIPTS)
    jobs = [(eligible[request], identifier) for request, identifier in identifiers.items()
            if request in eligible and request not in cache]
    if not jobs:
        return cache
    with ThreadPoolExecutor(max_workers=4) as pool:
        received = [sample for sample in pool.map(lambda job: fetch_receipt(meter, *job, key), jobs)
                    if sample is not None]
    if not received:
        return cache
    try:
        with locked(cache_path.with_suffix('.lock')):
            if sha256((directory / 'meter.json').read_bytes()).hexdigest() != digest:
                return cache
            merged = cached_receipts(cache_path, meter, digest, eligible)
            merged.update({item['request']: item for item in received})
            write_json(cache_path, {'schema_version': 1, 'meter_sha256': digest,
                                    'receipts': [item for _, item in sorted(merged.items())]})
            return merged
    except OSError:
        return cache


def with_delayed_receipt(row, receipt):
    if receipt is None:
        return row
    generation = receipt['generation_time_ms']
    tokens = receipt['completion_tokens']
    return {**row, 'reported_upstream': receipt['provider'], 'provider_matches_pin': True,
            'receipt_matches_route': True, 'timing': {
                **object_value(row.get('timing')),
                'openrouter_receipt_latency_ms': receipt['latency_ms'],
                'openrouter_receipt_generation_time_ms': generation,
                'openrouter_receipt_completion_tokens': tokens,
                'openrouter_receipt_completion_tokens_per_second': tokens * 1000 / generation
                if tokens is not None and generation else None,
            }}


def routing_summary(meter, rows, requests, delayed_receipt_count):
    pin_tag = allowed(meter.get('upstream'), PROVIDERS)
    pin_name = provider_name(meter.get('upstream_provider')) or provider_name(pin_tag)
    reported_names = [provider_name(row.get('reported_upstream')) for row in rows]
    observed = Counter(name for name in reported_names if name is not None)
    observed_requests = sum(observed.values())
    reported_requests = sum((isinstance(row.get('reported_upstream'), str) and bool(row['reported_upstream']))
                            or boolean(row.get('provider_matches_pin')) is not None for row in rows)
    confirmed = sorted({name for row in rows if row.get('receipt_matches_route') is True
                        and (name := provider_name(row.get('reported_upstream'))) is not None and name == pin_name})
    match = sum(boolean(row.get('receipt_matches_route')) is True for row in rows)
    mismatch = sum(boolean(row.get('receipt_matches_route')) is False for row in rows)
    live_lookups = Counter(str(value) if type(value) is int and 100 <= value <= 599
                           else allowed(value, RECEIPT_LOOKUP_RESULTS) or 'other'
                           for row in rows if (value := row.get('receipt_lookup')) is not None)
    provider_comparisons = []
    if pin_name:
        for row, name in zip(rows, reported_names, strict=True):
            comparison = boolean(row.get('provider_matches_pin'))
            if comparison is not None or name is not None:
                provider_comparisons.append(comparison is not False and name in (None, pin_name))
    provider_mismatch = provider_comparisons.count(False)
    fallback = None
    if provider_mismatch:
        fallback = True
    elif requests and len(provider_comparisons) == requests:
        fallback = False
    return {
        'provider_pin': pin_tag,
        'provider_pin_name': pin_name,
        'allow_fallbacks': boolean(meter.get('allow_fallbacks')) if meter.get('service') == 'openrouter' else None,
        'confirmed_upstream': confirmed[0] if confirmed else None,
        'requests': requests,
        'requests_with_reported_provider': reported_requests,
        'requests_with_identified_provider': observed_requests,
        'provider_receipt_coverage': coverage(reported_requests, requests),
        'provider_identity_coverage': coverage(observed_requests, requests),
        'requests_with_observed_pin': sum(pin_name is not None and provider_name(row.get('upstream_requested')) == pin_name
                                          for row in rows),
        'reported_provider_request_counts': dict(sorted(observed.items())),
        'reported_model_request_counts': dict(sorted(Counter(
            model for row in rows if (model := allowed(row.get('model_received'), LOCAL_MODELS)) is not None).items())),
        'requests_with_matching_receipt': match,
        'requests_with_live_matching_receipt': match - delayed_receipt_count,
        'requests_with_delayed_matching_receipt': delayed_receipt_count,
        'live_receipt_lookup_counts': dict(sorted(live_lookups.items())),
        'requests_with_nonmatching_receipt': mismatch,
        'requests_with_route_mismatch': sum(row.get('route_mismatch') is True for row in rows),
        'receipt_match_coverage': coverage(match + mismatch, requests),
        'requests_with_provider_comparison': len(provider_comparisons),
        'provider_mismatch_requests': provider_mismatch if provider_comparisons else None,
        'fallback_observed': fallback,
    }


def cost_summary(meter, ledger, requests, usages):
    legacy_complete = meter.get('amount_includes_unknown_upper_bounds') is False
    legacy_cost = money(meter.get('charged_peak_estimate_or_upper_bound_usd'))
    estimated = count(ledger.get('estimated_requests'))
    if not ledger and legacy_complete:
        estimated = sum(all(usage[field] is not None for field in
                            ('input_tokens', 'output_tokens', 'cached_input_tokens', 'uncached_input_tokens'))
                        for usage in usages)
    unknown = count(ledger.get('unknown_requests')) if ledger else (requests - estimated if estimated is not None else None)
    receipts = count(ledger.get('provider_reported_requests'))
    legacy_estimate = legacy_cost if legacy_complete else None
    token_estimate = money(ledger.get('token_estimate_usd')) if ledger else legacy_estimate
    return {
        'requests': requests,
        'provider_billed_usd': money(ledger.get('provider_billed_usd')) if receipts == requests and requests else None,
        'provider_reported_usd': money(ledger.get('provider_reported_usd')) if receipts else None,
        'provider_reported_requests': receipts,
        'provider_cost_sources': {
            source: {'requests': count(values.get('requests')), 'usd': money(values.get('usd'))}
            for source, values in object_value(ledger.get('provider_cost_sources')).items()
            if source in ('generation_receipt', 'stream_usage') and isinstance(values, dict)
        },
        'provider_reported_coverage': (coverage(receipts, requests) if meter.get('service') == 'openrouter' or receipts
                                       else 'not_applicable'),
        'token_estimate_usd': token_estimate if estimated else None,
        'unknown_usage_upper_bound_usd': money(ledger.get('unknown_usage_upper_bound_usd')),
        'accounted_estimate_or_upper_bound_usd': money(ledger.get('accounted_usd')) if ledger else legacy_cost,
        'estimated_requests': estimated,
        'unknown_requests': unknown,
    }


def timing_summaries(timings, definitions, requests):
    result = {}
    for name, unit in definitions.items():
        values = [value for timing in timings if (value := number(timing.get(name))) is not None]
        result[name] = {'unit': unit, 'requests': requests, 'measured_requests': len(values),
                        'coverage': coverage(len(values), requests), 'p50': middle(values), 'mean': average(values)}
    return result


def pooled_throughput(timings, requests, token_field, duration_field, *, start_field=None, scale=1):
    samples = []
    for timing in timings:
        tokens = count(timing.get(token_field))
        duration = number(timing.get(duration_field))
        start = number(timing.get(start_field)) if start_field else 0
        if tokens is not None and duration is not None and start is not None and duration > start:
            samples.append((tokens, (duration - start) / scale))
    tokens = sum_observed(tokens for tokens, _ in samples)
    seconds = sum_observed(seconds for _, seconds in samples)
    return {'requests': requests, 'measured_requests': len(samples), 'coverage': coverage(len(samples), requests),
            'completion_tokens': tokens, 'elapsed_seconds': seconds,
            'tokens_per_second': tokens / seconds if seconds else None}


def performance_summary(dispatched, requests):
    observed = [object_value(row.get('timing')) for row in dispatched if row.get('route_mismatch') is not True]
    matched = [object_value(row.get('timing')) for row in dispatched if row.get('receipt_matches_route') is True
               and row.get('route_mismatch') is not True]
    return {
        'proxy': timing_summaries(observed, PROXY_TIMINGS, requests),
        'openrouter_receipt': timing_summaries(matched, ROUTER_TIMINGS, requests),
        'pooled_throughput': {
            'proxy_stream': pooled_throughput(observed, requests, 'stream_completion_tokens',
                                              'request_to_stream_end_seconds', start_field='request_to_first_output_delta_seconds'),
            'proxy_end_to_end': pooled_throughput(observed, requests, 'stream_completion_tokens', 'request_to_stream_end_seconds'),
            'openrouter_generation': pooled_throughput(matched, requests, 'openrouter_receipt_completion_tokens',
                                                       'openrouter_receipt_generation_time_ms', scale=1000),
        },
        'provider_native_ttft': {'unit': 'seconds', 'requests': requests, 'measured_requests': 0,
                                 'coverage': coverage(0, requests), 'p50': None, 'mean': None},
    }


def meter_summary(meter, rows, delayed):
    ledger = object_value(meter.get('accounting'))
    dispatched = [with_delayed_receipt(row, delayed.get(count(row.get('request')))) for row in rows
                  if row.get('basis') != 'no_upstream_charge' or row.get('status') != 'connection_failed']
    usages = [request_usage(row if row.get('route_mismatch') is not True else {}) for row in dispatched]
    aggregate_usage = {key: sum_observed(usage[key] for usage in usages) for key in (*USAGE_FIELDS, 'reasoning_tokens')}
    finish = Counter((allowed(row.get('response_status'), FINISH_REASONS) or 'other')
                     for row in dispatched if row.get('response_status') is not None)
    rejections = meter.get('rejections')
    upstream_requests = count(ledger.get('upstream_requests'))
    if upstream_requests is None:
        upstream_requests = len(dispatched)
    return {
        'usage': {
            'requests': upstream_requests,
            'connection_failures_before_dispatch': len(rows) - len(dispatched),
            'requests_with_reported_input_tokens': sum(usage['input_tokens'] is not None for usage in usages),
            'requests_without_usage_due_to_route_mismatch': sum(row.get('route_mismatch') is True for row in dispatched),
            'meter_rejections': len(rejections) if isinstance(rejections, list) else None,
            'http_error_responses': sum(400 <= (count(row.get('http_status')) or 0) <= 599 for row in dispatched),
            'finish_reasons': dict(sorted(finish.items())),
            'last_request_finish_reason': ((allowed(dispatched[-1].get('response_status'), FINISH_REASONS) or 'other')
                                           if dispatched and dispatched[-1].get('response_status') is not None else None),
            **aggregate_usage,
            'requests_with_reasoning_tokens': sum(usage['reasoning_tokens'] is not None for usage in usages),
            'cache': {'all': cache_observation(usages), 'first_request': cache_observation(usages[:1]),
                      'subsequent_requests': cache_observation(usages[1:])},
        },
        'cost': cost_summary(meter, ledger, upstream_requests, usages),
        'routing': routing_summary(meter, dispatched, upstream_requests, len(delayed)),
        'performance': performance_summary(dispatched, upstream_requests),
    }


def session_journal(live):
    session_root = live / 'session-data'
    resolved_session_root = session_root.resolve()
    if resolved_session_root != live.resolve() / 'session-data':
        return None
    candidates = sorted(session_root.glob('p/*/sessions/*/events.jsonl'))
    if len(candidates) != 1:
        return None
    candidate = candidates[0]
    if candidate.resolve() != resolved_session_root / candidate.relative_to(session_root):
        return None
    return candidate


def session_events(journal):
    with journal.open('rb') as source:
        for index, line in enumerate(iter(lambda: source.readline(EVENT_LINE_BYTES + 1), b''), 1):
            if index > EVENT_RECORD_LIMIT or len(line) > EVENT_LINE_BYTES or not line.endswith(b'\n'):
                raise EvidenceError('saved Session journal exceeds its read bounds or has an incomplete tail')
            event = json.loads(line)
            if not isinstance(event, dict):
                raise EvidenceError('saved Session journal contains a non-object event')
            yield event


def session_summary(live, sdk):
    facts = object_value(sdk.get('facts'))
    tools = {**{key: count(facts.get(key)) for key in ('tool_calls', 'tool_results', 'tool_errors')},
             'non_ok_results': count(facts.get('tool_non_ok_results')),
             'workspace_script_calls': None, 'workspace_script_host_actions': None}
    terminal = 'turn.completed' if (count(facts.get('turns_completed')) or 0) > 0 else None
    result = {'terminal_event': terminal, 'interrupt_reason': None, 'durable_turn_settled': None,
              'settled_status': None, 'sdk_error_kind': None, 'sdk_error_detail': None}
    journal = session_journal(live)
    if journal is None:
        return tools, result
    statuses = Counter()
    calls = 0
    script_calls = script_actions = 0
    pending_scripts = set()
    script_evidence_complete = True
    observed = {**result, 'durable_turn_settled': False}
    agent = sdk.get('agent') or 'main'
    provider_error_turn, provider_error_kind = None, None
    try:
        for event in session_events(journal):
            event_agent = event.get('conversation') or event.get('agent') or 'main'
            if event.get('kind') == 'tool.call':
                calls += 1
                if event.get('tool') in ('run_js', 'workspace_script'):
                    script_calls += 1
                    call_id = event.get('tool_call_id')
                    if isinstance(call_id, str) and call_id:
                        call = (event_agent, event.get('turn'), call_id)
                        if call in pending_scripts:
                            script_evidence_complete = False
                        pending_scripts.add(call)
                    else:
                        script_evidence_complete = False
            if event.get('kind') == 'tool.result':
                statuses[allowed(event.get('status'), TOOL_STATUSES) or 'unknown'] += 1
                call = (event_agent, event.get('turn'), event.get('tool_call_id'))
                if call in pending_scripts:
                    pending_scripts.remove(call)
                    receipt = object_value(event.get('structured'))
                    if receipt.get('kind') == 'workspace_script_result' and isinstance(receipt.get('actions'), list):
                        script_actions += len(receipt['actions'])
                    else:
                        script_evidence_complete = False
            if event_agent == agent:
                kind = allowed(event.get('kind'), TURN_EVENTS)
                if kind is not None:
                    observed.update(terminal_event=kind, interrupt_reason=None)
                    if kind == 'turn.interrupted':
                        observed['interrupt_reason'] = allowed(event.get('reason'), INTERRUPT_REASONS) or 'other'
                if event.get('kind') == 'model.error':
                    provider_error_turn = count(event.get('turn'))
                    provider_error_kind = allowed(event.get('error_kind'), PROVIDER_ERROR_KINDS)
                elif kind == 'turn.interrupted' and observed['interrupt_reason'] in PROVIDER_ERROR_KINDS:
                    provider_error_turn = count(event.get('turn'))
                    provider_error_kind = observed['interrupt_reason']
            operation = object_value(event.get('record')) if event.get('kind') == 'runtime.operation' else {}
            if operation.get('operation') == 'turn_settled' and operation.get('address') == agent:
                error = operation.get('error')
                error_kind = None
                if isinstance(error, str):
                    error_kind = 'invalid_argument' if error.startswith('invalid argument: ') else 'other'
                    if (error.startswith('provider error: ') and provider_error_turn is not None
                            and provider_error_turn == count(operation.get('turn'))):
                        error_kind = provider_error_kind or 'other'
                observed.update(durable_turn_settled=True,
                                settled_status=allowed(operation.get('status'), SETTLED_STATUSES),
                                sdk_error_kind=error_kind,
                                sdk_error_detail='request_bytes_limit' if error == SDK_REQUEST_BYTES_ERROR else None)
    except (OSError, UnicodeError, ValueError, TypeError):
        return tools, result
    results = sum(statuses.values())
    tools = {'tool_calls': calls, 'tool_results': results, 'tool_errors': statuses['error'],
             'non_ok_results': results - statuses['ok'] if not statuses['unknown'] else None,
             'workspace_script_calls': script_calls,
             'workspace_script_host_actions': script_actions if script_evidence_complete and not pending_scripts else None}
    return tools, observed


def selector_summary(directory, case):
    root_result = directory / 'result.json'
    if not root_result.is_file():
        return None, None
    selector = read_object(root_result, 'selector run result')
    benchmark = object_value(selector.get('benchmark'))
    selector_case = object_value(benchmark.get('case')).get('id')
    if selector_case is not None and selector_case != case:
        raise EvidenceError('local selector result does not match its declared case')
    tree = object_value(selector.get('tree'))
    git = object_value(selector.get('git'))
    commit = hex_digest(tree.get('commit'), 40) or hex_digest(git.get('commit'), 40) or hex_digest(git.get('revision'), 40)
    attempts = selector.get('attempts')
    record = object_value(attempts[0]) if selector_case == case and isinstance(attempts, list) and len(attempts) == 1 else {}
    return commit, record if record.get('mode') == 'live' else None


def measured_fields(source, fields):
    source = object_value(source)
    values = {field: number(source.get(field)) for field in fields}
    return values if any(value is not None for value in values.values()) else None


def latest_container_block_io(snapshots):
    for snapshot in reversed(snapshots):
        value = object_value(snapshot).get('BlockIO')
        if not isinstance(value, str) or len(value) > 96:
            continue
        read, _, write = value.partition('/')
        sizes = [BLOCK_IO_SIZE.fullmatch(part.strip()) for part in (read, write)]
        if any(size is None or size[2] not in BYTE_UNIT_SCALES for size in sizes):
            continue
        read_bytes, write_bytes = (
            int((Decimal(size[1]) * BYTE_UNIT_SCALES[size[2]]).to_integral_value(rounding=ROUND_HALF_UP))
            for size in sizes
        )
        return {'read_bytes': read_bytes, 'write_bytes': write_bytes,
                'display': ' / '.join(size[1] + size[2] for size in sizes)}
    return None


def resource_summary(source):
    resources = object_value(source)
    memory = object_value(resources.get('harness_memory')).copy()
    memory['peak_rss_kib'] = peak_rss_kib(memory.get('peak_rss_kib'), resources.get('sdk_peak_rss_kib'),
                                          resources.get('sdk_peak_rss_high_water_kib'),
                                          object_value(resources.get('harness_self_usage')).get('peak_rss_kib'))
    snapshots = resources.get('container_snapshots')
    capture_incomplete = boolean(resources.get('capture_incomplete'))
    completed_split_sampler = (resources.get('container_scope') == 'sdk_and_tools'
                               and 'process_io_scope' in resources and capture_incomplete is False)
    default_errors = 0 if completed_split_sampler else None
    process_io = resources.get('process_io')
    process = {'io': measured_fields(process_io, IO_FIELDS),
               'sampling_errors': count(resources.get('process_sampling_errors', default_errors))}
    io_source = allowed(resources.get('process_io_source'), (EXTERNAL_PROCESS_IO, FINAL_PROCESS_IO))
    if io_source is not None and complete_process_io(process_io) is not None:
        process['io_source'] = io_source
    return {
        'harness': {
            'memory': measured_fields(memory, MEMORY_FIELDS),
            'cpu_seconds': number(resources.get('harness_cpu_seconds')),
            'self_usage': measured_fields(resources.get('harness_self_usage'), SELF_USAGE_FIELDS),
        },
        'process_including_waited_children': process,
        'container': {
            'samples': count(resources.get('container_samples')),
            'retained_snapshot_count': len(snapshots) if isinstance(snapshots, list) else None,
            'capture_incomplete': capture_incomplete,
            'sampling_errors': count(resources.get('container_sampling_errors', default_errors)),
            'block_io': latest_container_block_io(snapshots) if isinstance(snapshots, list) else None,
        },
        'legacy_combined_sampling_errors': count(resources.get('sampling_errors')),
    }


def local_attempt(case, directory, ordinal, label, receipt_key=None):
    if label is not None and not ROUND_LABEL.fullmatch(label):
        raise EvidenceError('invalid round label; use first, rerun, round-N, guidance, guarded, outputNk or contextNk')
    live = directory.resolve() / 'live'
    result = read_object(live / 'result.json', 'local live result')
    if result.get('harness', 'kuku') != 'kuku':
        raise EvidenceError('historical report expects Kuku evidence; use benchmark_compare.py for the paired run')
    meter, raw_meter = load_object(directory / 'meter.json', 'local meter')
    if result.get('mode') != 'live' or meter.get('case', case) != case:
        raise EvidenceError('local attempt evidence does not match its declared case or mode')
    sdk = object_value(result.get('sdk_evidence'))
    execution = object_value(result.get('execution'))
    verification = object_value(result.get('verification'))
    verifier = object_value(result.get('verifier_summary'))
    limits = object_value(meter.get('limits'))
    sdk_limits = object_value(sdk.get('limits'))
    rows = meter.get('attempts', [])
    if not isinstance(rows, list) or any(not isinstance(row, dict) for row in rows):
        raise EvidenceError('local meter must contain request rows')
    if any('case' in row and row['case'] != case for row in rows):
        raise EvidenceError('local meter request does not match its declared case')
    receipts = delayed_receipts(directory, meter, raw_meter, rows, receipt_key)
    measure = meter_summary(meter, rows, receipts)
    outputs = {value for row in rows if (value := count(row.get('max_output_tokens'))) is not None}
    output_cap = next(iter(outputs)) if len(outputs) == 1 else None
    model_version = allowed(meter.get('model_version'), {MODEL_VERSION})
    fallback_label = 'first' if ordinal == 1 else f'rerun-{ordinal - 1}'
    inferred_label = directory.parent.name
    tools, terminal = session_summary(live, sdk)
    sdk_binary = hex_digest(object_value(result.get('runtime_files')).get('session_task'), 64)
    meter_binary = hex_digest(meter.get('sdk_binary_sha256'), 64)
    if sdk_binary and meter_binary and sdk_binary != meter_binary:
        raise EvidenceError('local SDK binary digests disagree')
    checkout, selector_attempt = selector_summary(directory, case)
    reconciled = boolean(object_value(meter.get('accounting')).get('reconciled'))
    final_result = selector_attempt or result
    failure = allowed(final_result.get('failure_kind'), FAILURE_KINDS)
    prices = object_value(meter.get('price_usd_per_million_tokens'))
    return {
        'case': case,
        'attempt': ordinal,
        'phase': 'first' if ordinal == 1 else 'rerun',
        'round': label or (inferred_label if ROUND_LABEL.fullmatch(inferred_label) else fallback_label),
        'provenance': {'sdk_binary_sha256': sdk_binary or meter_binary, 'checkout_commit': checkout},
        'configuration': {
            'model_version': model_version,
            'model_requested': allowed(meter.get('model_requested'), LOCAL_MODELS),
            'service': (allowed(meter.get('service'), {'deepseek', 'openrouter'})
                        or ('deepseek' if meter.get('upstream') == 'api.deepseek.com' else None)),
            'wire': allowed(meter.get('wire'), {'openai-chat', 'openai-responses'}),
            'upstream': allowed(meter.get('upstream'), PROVIDERS),
            'catalog_upstream_model': allowed(meter.get('catalog_upstream_model'), LOCAL_MODELS),
            'price_usd_per_million_tokens': {field: money(prices.get(field, prices.get(wire_field)))
                                             for field, wire_field in (('input', 'prompt'), ('cached_input', 'cache_read'),
                                                                       ('output', 'completion'), ('cache_write', 'cache_write'))},
            'reasoning_effort': allowed(meter.get('effort'), EFFORTS),
            'case_cap_usd': money(meter.get('case_cap_usd')),
            'max_requests': count(limits.get('max_requests', sdk_limits.get('max_requests'))),
            'max_request_bytes': count(limits.get('max_request_bytes', sdk_limits.get('max_request_bytes'))),
            'observed_max_request_bytes': max((value for row in rows if (value := count(row.get('request_bytes'))) is not None),
                                              default=None),
            'max_output_tokens': count(limits.get('max_output_tokens', sdk_limits.get('max_output_tokens', output_cap))),
            'observed_max_output_tokens': max(outputs, default=None),
            'provider_max_output_tokens': count(limits.get('provider_max_output_tokens')),
            'sdk_context_tokens': count(limits.get('sdk_context_tokens', sdk_limits.get('max_context_tokens'))),
            'provider_context_tokens': count(limits.get('provider_context_tokens')),
            'agent_timeout_sec': count(limits.get('agent_timeout_sec')),
            **({'max_steps': count(sdk_limits['max_steps'])} if 'max_steps' in sdk_limits else {}),
            'automatic_retries': count(sdk_limits.get('automatic_retries')),
            'max_agents': count(sdk_limits.get('max_agents')),
        },
        'result': {
            'passed': False if reconciled is False else boolean(final_result.get('passed')),
            'live_result_passed': boolean(result.get('passed')),
            'meter_reconciled': reconciled,
            'task_passed': boolean(result.get('task_passed')),
            'reward': number(result.get('reward')),
            'failure_kind': 'provider_boundary' if reconciled is False else failure,
            **terminal,
            'sdk_execution_seconds': number(execution.get('seconds')),
            'verifier_seconds': number(verification.get('seconds')),
            'sdk_timed_out': boolean(execution.get('timed_out')),
            'verifier_timed_out': boolean(verification.get('timed_out')),
            'tests': {key: count(verifier.get(key)) for key in TEST_FIELDS} if verifier else None,
        },
        'tools': tools,
        'resources': resource_summary(result.get('resources')),
        **measure,
    }


def build_report(feed, attempts, official_live_sha256=None, *, receipt_key=None):
    official = official_report(feed)
    official['snapshot_sha256'] = hex_digest(official_live_sha256, 64)
    counts = Counter()
    records = []
    for attempt in attempts:
        if len(attempt) not in {2, 3} or attempt[0] not in CASES:
            raise EvidenceError('expected --attempt CASE RESULT_DIR [ROUND_LABEL] for a supported case')
        case, directory, *label = attempt
        counts[case] += 1
        records.append(local_attempt(case, Path(directory), counts[case], label[0] if label else None, receipt_key))
    return {
        'schema_version': 1,
        'measurement': {
            'comparison': 'The historical official Runta/Baseten baseline and local kuku observations come from different environments.',
            'ordering': 'For each case, the first supplied attempt is first and each later attempt is a rerun.',
            'official_scoring': 'Flagged passes score as failures. Infrastructure outcomes are excluded.',
            'official_time': ('Agent plus verifier time, excluding queue wait. Time per pass divides total time across all '
                              'scored attempts by audited passes. It requires timing on every scored attempt.'),
            'official_cost': 'Mean inference USD from the official feed. Sandbox charges are excluded.',
            'local_time': 'SDK execution inside the container and verifier wall times are measured separately.',
            'local_performance': ('Proxy timing starts before the outbound connection. First output is the first nonempty text, '
                                  'reasoning or tool-argument delta. Stream TPS spans first output to stream end and uses reported '
                                  'completion tokens. OpenRouter receipt latency is the reported latency field. Provider-native TTFT is unavailable. '
                                  'Receipt TPS uses native completion tokens and generation duration. '
                                  'Matching delayed receipts can supplement provider timing and routing evidence.'),
            'local_cost': ('Completed OpenRouter stream usage or generation receipts provide known request charges. DeepSeek direct charges are estimated from token usage. '
                           'Missing charges may be accounted at an upper bound. Partial reported amounts cover requests with a provider-reported cost.'),
            'local_usage': ('Token sums use available usage and may be incomplete. Reasoning tokens are part of output when reported. '
                            'Cache hit rate divides cached input by input only on requests with both fields.'),
            'local_routing': ('The pin is request configuration. Reported providers and route matching come from response evidence. '
                              'Unknown observations do not confirm a route or rule out fallback.'),
            'local_result': 'Task pass is the local runner result. Reward is the raw verifier score independently observed by the runner.',
            'local_tools': ('Tool errors count tool.result status error. Non-ok results include blocked and canceled calls. '
                            'Workspace script calls count model invocations. Host actions count the dispatched actions in '
                            'script result receipts and are unknown if receipts are incomplete.'),
            'local_resources': ('Harness memory is in KiB. The mean is time-weighted over RSS samples. '
                                'The peak is the largest available process RSS or high-water observation. '
                                'End-of-run self usage is from getrusage(RUSAGE_SELF), '
                                'with I/O in blocks. Process I/O uses the final in-process /proc/self/io snapshot when complete '
                                '(io_source proc_self_io), or the last complete external /proc/PID/io sample (io_source proc_pid_io). '
                                'Both include children already waited on. Older observations may lack source metadata. '
                                'Its rchar and wchar count logical characters. The read_bytes and write_bytes fields count storage I/O bytes. '
                                'Block I/O covers the entire container, including tools. Its byte values come from the latest valid '
                                'retained Docker stats display and may be rounded or precede final writes. '
                                'Sampling error counters track caught sampling exceptions. Earlier records combine the two samplers.'),
        },
        'official': official,
        'local_attempts': records,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--official-live', required=True, type=Path, metavar='JSON')
    parser.add_argument('--attempt', action='append', nargs='+', default=[], metavar='VALUE',
                        help='CASE RESULT_DIR [ROUND_LABEL]; repeat in chronological order per case')
    parser.add_argument('--refresh-receipts', type=Path, metavar='KEYFILE',
                        help='read KUKU_TEST_OPENROUTER_API_KEY from a credential file for delayed generation lookups')
    parser.add_argument('--output', type=Path, metavar='JSON', help='default: standard output')
    args = parser.parse_args()
    try:
        feed, raw = load_object(args.official_live, 'official live snapshot')
        key = None
        if args.refresh_receipts is not None and args.attempt:
            try:
                key = load_secrets(args.refresh_receipts).get('KUKU_TEST_OPENROUTER_API_KEY')
            except (OSError, UnicodeError, ValueError):
                pass
            if not key:
                sys.stderr.write('OpenRouter receipt refresh skipped: credential unavailable\n')
        report = build_report(feed, args.attempt, sha256(raw).hexdigest(), receipt_key=key)
        encoded = json.dumps(report, indent=2, allow_nan=False) + '\n'
        if args.output is None:
            sys.stdout.write(encoded)
        else:
            args.output.write_text(encoded, encoding='utf-8')
    except EvidenceError as error:
        parser.exit(2, f'error: {error}\n')
    except OSError:
        parser.exit(2, 'error: cannot write report output\n')


if __name__ == '__main__':
    main()
