#!/usr/bin/env python3
"""Export a matched local Frontier Kuku/Pi comparison as shareable JSON."""

import argparse
from hashlib import sha256
import json
from pathlib import Path
import re

from benchmark_resources import CASE_RESOURCE_LIMITS
from frontier_report import (CASES, FAILURE_KINDS, TASK_COMMIT, TEST_FIELDS, EvidenceError, allowed, boolean,
                             count, hex_digest, money, number, object_value, read_object)


REQUIRED_LIMITS = ('max_request_bytes', 'max_output_tokens', 'agent_timeout_sec', 'provider_context_tokens')
TOKEN_FIELDS = ('prompt_tokens', 'completion_tokens', 'prompt_cache_hit_tokens', 'prompt_cache_miss_tokens')


def attempt(directory, harness):
    record = read_object(directory / 'result.json', 'acceptance result')
    meter = read_object(directory / 'meter.json', 'provider meter')
    benchmark = object_value(record.get('benchmark'))
    attempts = record.get('attempts')
    case = allowed(object_value(benchmark.get('case')).get('id'), CASES)
    if (record.get('group') != 'benchmark' or benchmark.get('suite') != 'frontier' or case is None
            or meter.get('case') != case or object_value(benchmark.get('source')).get('revision') != TASK_COMMIT
            or not isinstance(attempts, list) or len(attempts) != 1):
        raise EvidenceError('expected one pinned Frontier benchmark attempt per directory')
    result = object_value(attempts[0])
    if (result.get('harness') != harness or result.get('mode') != 'live'
            or result.get('resource_requirements') != CASE_RESOURCE_LIMITS):
        raise EvidenceError(f'expected strict live {harness} evidence')
    route = {'service': 'deepseek', 'wire': 'openai-chat', 'provider': 'api.deepseek.com',
             'model_requested': 'deepseek-flash', 'effort': 'max'}
    if any(meter.get(key) != value for key, value in route.items()):
        raise EvidenceError('paired runs require direct DeepSeek Chat with max reasoning')
    files = object_value(benchmark.get('task_files_sha256'))
    image_id = benchmark.get('image_id')
    limits = object_value(meter.get('limits'))
    cap = money(meter.get('case_cap_usd'))
    if (not files or not all(hex_digest(value, 64) for value in files.values())
            or not isinstance(image_id, str) or not re.fullmatch(r'sha256:[0-9a-f]{64}', image_id)
            or not cap or any(not count(limits.get(key)) for key in REQUIRED_LIMITS)
            or ('max_requests' not in limits or (limits['max_requests'] is not None and not count(limits['max_requests'])))):
        raise EvidenceError('missing task, image, budget or provider limit evidence')
    shared = {'case': case, 'source_revision': TASK_COMMIT, 'image_id': image_id,
              'task_files_sha256': sha256(json.dumps(files, sort_keys=True).encode()).hexdigest(),
              'provider': route, 'case_cap_usd': cap, 'container_limits': CASE_RESOURCE_LIMITS,
              'model_limits': {key: limits[key] for key in (*REQUIRED_LIMITS, 'max_requests')}}
    resources = object_value(result.get('resource_evidence'))
    accounting = object_value(meter.get('accounting'))
    package = object_value(result.get('pi_package'))
    runtime_files = object_value(result.get('runtime_files'))
    agent = object_value(result.get('agent_evidence' if harness == 'pi' else 'sdk_evidence'))
    tool_facts = agent if harness == 'pi' else object_value(agent.get('facts'))
    memory_events = object_value(resources.get('memory_event_deltas'))
    version = package.get('version')
    return shared, {
        'harness': harness, 'passed': boolean(record.get('passed')), 'task_passed': boolean(result.get('task_passed')),
        'failure_kind': allowed(result.get('failure_kind'), FAILURE_KINDS), 'reward': number(result.get('reward')),
        'agent_evidence_passed': boolean(agent.get('passed')),
        'provenance': {
            'checkout_commit': hex_digest(object_value(record.get('tree')).get('commit'), 40),
            'kuku_binary_sha256': hex_digest(runtime_files.get('session_task'), 64),
            'pi_version': version if isinstance(version, str) and re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+', version) else None,
            'pi_lock_sha256': hex_digest(package.get('lock_sha256'), 64),
            'pi_cli_sha256': hex_digest(package.get('cli_sha256'), 64),
            'node_binary_sha256': hex_digest(runtime_files.get('node'), 64),
        },
        'agent_seconds': number(object_value(result.get('execution')).get('seconds')),
        'verifier_seconds': number(object_value(result.get('verification')).get('seconds')),
        'tests': {key: count(object_value(result.get('verifier_summary')).get(key)) for key in TEST_FIELDS},
        'upstream_requests': count(accounting.get('upstream_requests')),
        'tool_calls': count(tool_facts.get('tool_calls')),
        'usage': {key: count(object_value(accounting.get('usage')).get(key)) for key in TOKEN_FIELDS},
        'token_estimate_usd': money(accounting.get('token_estimate_usd')),
        'accounted_usd': money(accounting.get('accounted_usd')),
        'accounting_complete': boolean(accounting.get('complete')),
        'resources_passed': boolean(resources.get('passed')),
        'container_cpu_seconds': number(resources.get('cpu_seconds')),
        'container_memory_peak_bytes': count(resources.get('memory_peak_bytes')),
        'harness_peak_rss_kib': count(resources.get('harness_peak_rss_kib')),
        'oom_events': count(memory_events.get('oom')),
        'oom_kills': count(memory_events.get('oom_kill')),
    }


def compare(kuku, pi):
    shared, left = attempt(kuku, 'kuku')
    pi_shared, right = attempt(pi, 'pi')
    mismatches = [field for field in shared if shared[field] != pi_shared[field]]
    if mismatches:
        raise EvidenceError('paired configuration differs in ' + ', '.join(mismatches))
    return {'schema_version': 1, 'configuration': shared, 'attempts': [left, right]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kuku', required=True, type=Path, metavar='RUN_DIRECTORY')
    parser.add_argument('--pi', required=True, type=Path, metavar='RUN_DIRECTORY')
    args = parser.parse_args()
    try:
        print(json.dumps(compare(args.kuku, args.pi), indent=2, allow_nan=False))
    except EvidenceError as error:
        parser.exit(2, f'error: {error}\n')


if __name__ == '__main__':
    main()
