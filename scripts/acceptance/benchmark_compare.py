#!/usr/bin/env python3
"""Export a matched Frontier harness, SDK, Prompt, or reasoning comparison as shareable JSON."""

import argparse
from collections import Counter
from datetime import datetime
from hashlib import sha256
import json
from pathlib import Path
import re

from benchmark_resources import CASE_RESOURCE_LIMITS
from benchmark_runtime import verifier_passed
from evidence import tool_batch_summary
from frontier_report import (FAILURE_KINDS, TASK_COMMIT, TEST_FIELDS, EvidenceError, allowed, boolean,
                             count, hex_digest, middle, money, number, object_value, read_object,
                             session_events, session_journal, session_summary)
from run import manifest_case


REQUIRED_LIMITS = ('max_request_bytes', 'max_output_tokens', 'agent_timeout_sec', 'provider_context_tokens')
TOKEN_FIELDS = ('prompt_tokens', 'completion_tokens', 'prompt_cache_hit_tokens', 'prompt_cache_miss_tokens')


def digest_map(value):
    files = object_value(value)
    if files and all(hex_digest(digest, 64) for digest in files.values()):
        return sha256(json.dumps(files, sort_keys=True).encode()).hexdigest()
    return None


def relay_summary(rows):
    if not isinstance(rows, list) or any(not isinstance(row, dict) for row in rows):
        raise EvidenceError('paired meter must contain request rows')
    elapsed = [value for row in rows if (value := number(row.get('seconds'))) is not None]
    request_bytes = [value for row in rows if (value := count(row.get('request_bytes'))) is not None]
    first_output = [value for row in rows if (value := number(object_value(row.get('timing')).get(
        'request_to_first_output_delta_seconds'))) is not None]
    emitted = [value for row in rows if (value := count(row.get('emitted_tool_calls'))) is not None]
    reasoning = [value for row in rows if (value := count(row.get('reasoning_output_tokens'))) is not None]
    return {
        'admitted_attempts': len(rows),
        'timed_attempts': len(elapsed),
        'elapsed_seconds': round(sum(elapsed), 6) if elapsed and len(elapsed) == len(rows) else None,
        'request_body_bytes': sum(request_bytes) if request_bytes and len(request_bytes) == len(rows) else None,
        'first_output_samples': len(first_output),
        'first_output_p50_seconds': middle(first_output),
        'emitted_tool_calls': sum(emitted) if emitted else None,
        'reasoning_output_tokens': sum(reasoning) if reasoning and len(reasoning) == len(rows) else None,
        'reasoning_usage_samples': len(reasoning),
        'emitted_tool_batches': {'observed_responses': len(emitted), **tool_batch_summary(Counter(emitted))}
        if emitted else None,
    }


def journal_time(event):
    value = re.sub(r'(\.\d{6})\d+', r'\1', event['ts']).replace('Z', '+00:00')
    time = datetime.fromisoformat(value)
    if time.tzinfo is None:
        raise ValueError('Session journal timestamp has no offset')
    return time


def artifact_mutations(event):
    receipt = object_value(event.get('structured'))
    if receipt.get('kind') != 'workspace_script_result':
        return (receipt,)
    return (object_value(action.get('result')) for action in receipt.get('actions', [])
            if isinstance(action, dict) and action.get('status') == 'ok')


def artifact_observation(live, attempt):
    artifacts = attempt.get('task_artifacts')
    if attempt.get('task_passed') is not True or not isinstance(artifacts, list) or not artifacts:
        return None
    expected = {}
    for artifact in map(object_value, artifacts):
        path = artifact.get('task_path')
        digest = hex_digest(artifact.get('sha256'), 64)
        if artifact.get('status') != 'retained' or not isinstance(path, str) or not digest or path in expected:
            return None
        expected[path] = 'sha256:' + digest
    found = {}
    first, last = None, None
    try:
        journal = session_journal(live)
        if journal is None:
            return None
        for event in session_events(journal):
            kind = event.get('kind')
            if kind == 'runtime.step_prepared' and first is None:
                first = journal_time(event)
            elif kind == 'turn.completed':
                last = journal_time(event)
            elif first is not None and kind == 'tool.result' and event.get('status') == 'ok':
                for facts in artifact_mutations(event):
                    path = facts.get('canonical_path')
                    if (path in expected and path not in found and facts.get('commit_state') == 'committed'
                            and facts.get('content_hash_after') == expected[path]):
                        found[path] = journal_time(event)
        complete = max(found.values()) if found.keys() == expected.keys() else None
        start_seconds = round((complete - first).total_seconds(), 6) if complete else None
        tail_seconds = round((last - complete).total_seconds(), 6) if last and complete else None
        if ((start_seconds is not None and start_seconds < 0) or (tail_seconds is not None and tail_seconds < 0)):
            return None
    except (OSError, ValueError, TypeError, KeyError, RuntimeError):
        return None
    return {'clock': 'session_journal', 'retained_artifacts': len(expected), 'matched_artifacts': len(found),
            'seconds_from_first_prepared_request': start_seconds, 'seconds_to_last_completed_turn': tail_seconds}


def attempt(directory, harness):
    record = read_object(directory / 'result.json', 'acceptance result')
    meter = read_object(directory / 'meter.json', 'provider meter')
    benchmark = object_value(record.get('benchmark'))
    attempts = record.get('attempts')
    case = object_value(benchmark.get('case')).get('id')
    try:
        manifest_case(case, 'frontier')
    except ValueError as error:
        raise EvidenceError('expected a case from the Frontier manifest') from error
    if (record.get('group') != 'benchmark' or benchmark.get('suite') != 'frontier'
            or meter.get('case') != case or object_value(benchmark.get('source')).get('revision') != TASK_COMMIT
            or not isinstance(attempts, list) or len(attempts) != 1):
        raise EvidenceError('expected one pinned Frontier benchmark attempt per directory')
    result = object_value(attempts[0])
    requirements = object_value(result.get('resource_requirements'))
    cpus = count(requirements.get('cpus'))
    if (result.get('harness') != harness or result.get('mode') != 'live'
            or not cpus or requirements != {**CASE_RESOURCE_LIMITS, 'cpus': cpus}):
        raise EvidenceError(f'expected strict live {harness} evidence')
    route = {'service': 'deepseek', 'provider': 'api.deepseek.com',
             'model_requested': 'deepseek-flash'}
    wire = meter.get('wire')
    expected_wire = {'pi': ('openai-chat',), 'codex': ('openai-responses',),
                     'kuku': ('openai-chat', 'openai-responses')}
    effort = meter.get('effort')
    if (any(meter.get(key) != value for key, value in route.items())
            or wire not in expected_wire[harness] or effort not in ('low', 'high', 'max')):
        raise EvidenceError('paired runs require direct DeepSeek Flash with a recorded reasoning effort')
    model_version = meter.get('model_version')
    if not isinstance(model_version, str) or not re.fullmatch(r'[A-Za-z0-9._-]{1,120}', model_version):
        model_version = None
    route.update(wire=wire, effort=effort, model_version=model_version)
    task_digest = digest_map(benchmark.get('task_files_sha256'))
    image_id = benchmark.get('image_id')
    limits = object_value(meter.get('limits'))
    cap = money(meter.get('case_cap_usd'))
    max_command_seconds = result.get('max_command_seconds')
    if max_command_seconds is not None and (harness != 'kuku' or not count(max_command_seconds)):
        raise EvidenceError('invalid SDK command deadline evidence')
    if (not task_digest
            or not isinstance(image_id, str) or not re.fullmatch(r'sha256:[0-9a-f]{64}', image_id)
            or not cap or any(not count(limits.get(key)) for key in REQUIRED_LIMITS)
            or ('max_requests' not in limits or (limits['max_requests'] is not None and not count(limits['max_requests'])))):
        raise EvidenceError('missing task, image, budget or provider limit evidence')
    shared = {'case': case, 'source_revision': TASK_COMMIT, 'image_id': image_id,
              'task_files_sha256': task_digest,
              'provider': route, 'case_cap_usd': cap, 'container_limits': requirements,
              'max_command_seconds': max_command_seconds,
              'model_limits': {key: limits[key] for key in (*REQUIRED_LIMITS, 'max_requests')}}
    resources = object_value(result.get('resource_evidence'))
    accounting = object_value(meter.get('accounting'))
    package = object_value(result.get('pi_package'))
    codex_package = object_value(result.get('codex_package'))
    runtime_files = object_value(result.get('runtime_files'))
    prompt_digest = digest_map(result.get('prompt_overrides'))
    if 'prompt_overrides' in result and not prompt_digest:
        raise EvidenceError('invalid recorded Prompt override snapshot')
    agent = object_value(result.get('sdk_evidence' if harness == 'kuku' else 'agent_evidence'))
    tool_facts = object_value(agent.get('facts')) if harness == 'kuku' else agent
    tool_batches = object_value(agent.get('tool_batches'))
    tool_calls = count(tool_facts.get('tool_calls'))
    tool_call_source = 'runner_inspection' if tool_calls is not None else None
    if harness == 'kuku' and tool_calls is None:
        observed, _ = session_summary(directory / 'live', agent)
        tool_calls = count(observed.get('tool_calls'))
        if tool_calls is not None:
            tool_call_source = 'session_journal'
    rejections = meter.get('rejections')
    case_cap_rejections = (sum(object_value(entry).get('reason') == 'case USD cap' for entry in rejections)
                           if isinstance(rejections, list) else None)
    memory_events = object_value(resources.get('memory_event_deltas'))
    verification = object_value(result.get('verification'))
    verifier_summary = object_value(result.get('verifier_summary'))
    original_verifier_passed = (verifier_passed('live', verification, result.get('reward'), verifier_summary)
                                if type(verification.get('exit_code')) is int else None)
    version = package.get('version')
    codex_version = codex_package.get('version')
    runner_timeout = boolean(object_value(result.get('execution')).get('timed_out'))
    sdk_deadline = boolean(agent.get('task_deadline_elapsed')) if harness == 'kuku' else None
    return shared, {
        'harness': harness, 'passed': boolean(record.get('passed')), 'task_passed': boolean(result.get('task_passed')),
        'failure_kind': allowed(result.get('failure_kind'), FAILURE_KINDS), 'reward': number(result.get('reward')),
        'agent_evidence_passed': boolean(agent.get('passed')),
        'provenance': {
            'checkout_commit': hex_digest(object_value(record.get('tree')).get('commit'), 40),
            'kuku_binary_sha256': hex_digest(runtime_files.get('session_task'), 64),
            'kuku_runtime_sha256': digest_map(runtime_files) if harness == 'kuku' else None,
            'prompt_overrides_sha256': prompt_digest,
            'pi_version': version if isinstance(version, str) and re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+', version) else None,
            'pi_lock_sha256': hex_digest(package.get('lock_sha256'), 64),
            'pi_cli_sha256': hex_digest(package.get('cli_sha256'), 64),
            'node_binary_sha256': hex_digest(runtime_files.get('node'), 64),
            'codex_version': codex_version if isinstance(codex_version, str) and re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+', codex_version) else None,
            'codex_lock_sha256': hex_digest(codex_package.get('lock_sha256'), 64),
            'codex_catalog_sha256': hex_digest(codex_package.get('catalog_sha256'), 64),
            'codex_binary_sha256': hex_digest(runtime_files.get('codex'), 64),
        },
        'agent_seconds': number(object_value(result.get('execution')).get('seconds')),
        'agent_timed_out': True if sdk_deadline else runner_timeout,
        'sdk_task_deadline_elapsed': sdk_deadline,
        'supervisor_timed_out': runner_timeout,
        'verifier_seconds': number(verification.get('seconds')),
        'original_verifier_passed': original_verifier_passed,
        'tests': {key: count(verifier_summary.get(key)) for key in TEST_FIELDS},
        'final_artifact_observation': artifact_observation(directory / 'live', result) if harness == 'kuku' else None,
        'upstream_requests': count(accounting.get('upstream_requests')),
        'provider_relay': relay_summary(meter.get('attempts', [])),
        'case_cap_rejections': case_cap_rejections,
        'tool_calls': tool_calls,
        'tool_call_source': tool_call_source,
        'tool_batches': {key: count(tool_batches.get(key))
                         for key in ('without_tools', 'single_call', 'multiple_calls', 'max_calls')} if tool_batches else None,
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


def require_matching(left, right):
    mismatches = [field for field in left if left[field] != right[field]]
    if mismatches:
        raise EvidenceError('paired configuration differs in ' + ', '.join(mismatches))


def compare(kuku, reference, harness='pi'):
    shared, left = attempt(kuku, 'kuku')
    other, right = attempt(reference, harness)
    require_matching(shared, other)
    return {'schema_version': 1, 'configuration': shared, 'attempts': [left, right]}


def require_same_sdk(left, right):
    before, after = left['provenance'], right['provenance']
    for key in ('kuku_binary_sha256', 'kuku_runtime_sha256'):
        if not before[key] or before[key] != after[key]:
            raise EvidenceError('comparison requires the same SDK binary and packaged runtime')
    return before, after


def compare_prompts(baseline, candidate):
    shared, left = attempt(baseline, 'kuku')
    other, right = attempt(candidate, 'kuku')
    require_matching(shared, other)
    before, after = require_same_sdk(left, right)
    if (not before['prompt_overrides_sha256'] or not after['prompt_overrides_sha256']
            or before['prompt_overrides_sha256'] == after['prompt_overrides_sha256']):
        raise EvidenceError('Prompt comparison requires two distinct recorded Prompt override snapshots')
    return {'schema_version': 1, 'configuration': shared,
            'attempts': [{'arm': 'baseline', **left}, {'arm': 'candidate', **right}]}


def compare_sdk(baseline, candidate):
    shared, left = attempt(baseline, 'kuku')
    other, right = attempt(candidate, 'kuku')
    require_matching(shared, other)
    before, after = left['provenance'], right['provenance']
    if not shared['provider']['model_version']:
        raise EvidenceError('SDK comparison requires a recorded model version')
    if before['prompt_overrides_sha256'] != after['prompt_overrides_sha256']:
        raise EvidenceError('SDK comparison requires the same external Prompt overrides')
    for key in ('kuku_binary_sha256', 'kuku_runtime_sha256'):
        if not before[key] or not after[key] or before[key] == after[key]:
            raise EvidenceError('SDK comparison requires distinct SDK binaries and packaged runtimes')
    return {'schema_version': 1, 'configuration': shared,
            'attempts': [{'arm': 'baseline', **left}, {'arm': 'candidate', **right}]}


def compare_effort(baseline, candidate):
    shared, left = attempt(baseline, 'kuku')
    other, right = attempt(candidate, 'kuku')
    before_route, after_route = dict(shared['provider']), dict(other['provider'])
    before_effort, after_effort = before_route.pop('effort'), after_route.pop('effort')
    require_matching({**shared, 'provider': before_route}, {**other, 'provider': after_route})
    if not before_route['model_version'] or before_effort == after_effort:
        raise EvidenceError('reasoning comparison requires a recorded model version and distinct efforts')
    before, after = require_same_sdk(left, right)
    if before['prompt_overrides_sha256'] != after['prompt_overrides_sha256']:
        raise EvidenceError('reasoning comparison requires the same Prompt')
    return {'schema_version': 1, 'configuration': {**shared, 'provider': before_route},
            'attempts': [{'arm': 'baseline', 'reasoning_effort': before_effort, **left},
                         {'arm': 'candidate', 'reasoning_effort': after_effort, **right}]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('kuku', 'pi', 'codex', 'baseline', 'candidate'):
        parser.add_argument(f'--{name}', type=Path, metavar='RUN_DIRECTORY')
    comparison = parser.add_mutually_exclusive_group()
    comparison.add_argument('--compare-effort', action='store_true',
                            help='compare two SDK attempts with different reasoning efforts and the same Prompt')
    comparison.add_argument('--compare-sdk', action='store_true',
                            help='compare distinct SDK builds with matching run settings and external Prompt overrides')
    args = parser.parse_args()
    try:
        if (args.kuku and bool(args.pi) != bool(args.codex)
                and not (args.baseline or args.candidate or args.compare_effort or args.compare_sdk)):
            report = compare(args.kuku, args.pi or args.codex, 'pi' if args.pi else 'codex')
        elif args.baseline and args.candidate and not (args.kuku or args.pi or args.codex):
            if args.compare_sdk:
                comparator = compare_sdk
            elif args.compare_effort:
                comparator = compare_effort
            else:
                comparator = compare_prompts
            report = comparator(args.baseline, args.candidate)
        else:
            parser.error('provide --kuku and one reference (--pi or --codex), or --baseline and --candidate')
        print(json.dumps(report, indent=2, allow_nan=False))
    except EvidenceError as error:
        parser.exit(2, f'error: {error}\n')


if __name__ == '__main__':
    main()
