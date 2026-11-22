"""Observe the benchmark SDK process separately from tools and the task container."""

import json
import math
import os
import subprocess
import threading
import time


IO_FIELDS = ('rchar', 'wchar', 'syscr', 'syscw', 'read_bytes', 'write_bytes', 'cancelled_write_bytes')
PROCESS_IO_SCOPE = 'SDK process and waited-for children. rchar and wchar count logical bytes.'
EXTERNAL_PROCESS_IO = 'proc_pid_io'
FINAL_PROCESS_IO = 'proc_self_io'
PROC_TICKS_PER_SECOND_FALLBACK = 100
CASE_MEMORY_BYTES = 2 * 1024**3
CASE_RESOURCE_LIMITS = {'cpus': 1, 'memory_bytes': CASE_MEMORY_BYTES, 'swap_bytes': 0}
CGROUP_FILES = ('cpu.max', 'cpu.stat', 'memory.max', 'memory.swap.max', 'memory.peak', 'memory.events')
CPU_THROTTLING_FIELDS = ('nr_periods', 'nr_throttled', 'throttled_usec')
PROC = '''p=$(cat /logs/agent/sdk.pid 2>/dev/null) || exit 1
d=/proc/$p
cat "$d/stat" "$d/status" 2>/dev/null || exit 1
cat "$d/io" 2>/dev/null || :
'''


def read_case_cgroup(container):
    command = 'for field do printf "[%s]\\n" "$field"; cat "/sys/fs/cgroup/$field" || exit 1; done'
    try:
        result = subprocess.run(['docker', 'exec', container, 'sh', '-c', command, 'sh', *CGROUP_FILES],
                                capture_output=True, text=True, check=True, timeout=5)
        return parse_case_cgroup(result.stdout)
    except (OSError, subprocess.SubprocessError, ValueError, KeyError, IndexError) as error:
        raise ValueError('benchmark resource evidence requires container cgroup v2 CPU and memory counters') from error


def parse_case_cgroup(output):
    blocks = {}
    name = None
    for line in output.splitlines():
        if line.startswith('[') and line.endswith(']'):
            name = line[1:-1]
            if name not in CGROUP_FILES or name in blocks:
                raise ValueError('invalid cgroup counter header')
            blocks[name] = []
        elif name is not None:
            blocks[name].append(line)
        else:
            raise ValueError('missing cgroup counter header')
    quota, period = map(int, blocks['cpu.max'][0].split())
    limit = int(blocks['memory.max'][0])
    swap = int(blocks['memory.swap.max'][0])
    peak = int(blocks['memory.peak'][0])
    cpu = dict(line.split() for line in blocks['cpu.stat'])
    events = dict(line.split() for line in blocks['memory.events'])
    oom = {name: int(events[name]) for name in ('oom', 'oom_kill')}
    cpu_usec = int(cpu['usage_usec'])
    if min(quota, period, limit, peak) <= 0 or min(cpu_usec, swap, *oom.values()) < 0:
        raise ValueError('invalid cgroup counter value')
    try:
        throttling = {field: int(cpu[field]) for field in CPU_THROTTLING_FIELDS}
    except (KeyError, ValueError):
        throttling = None
    if throttling is not None and min(throttling.values()) < 0:
        throttling = None
    return {'cpu_quota_usec': quota, 'cpu_period_usec': period, 'cpu_usage_usec': cpu_usec,
            'cpu_throttling': throttling,
            'memory_limit_bytes': limit, 'swap_limit_bytes': swap, 'memory_peak_bytes': peak,
            'memory_events': oom}


def validate_case_limits(counters, cpus=CASE_RESOURCE_LIMITS['cpus']):
    if (counters['cpu_quota_usec'] != counters['cpu_period_usec'] * cpus
            or counters['memory_limit_bytes'] != CASE_MEMORY_BYTES or counters['swap_limit_bytes'] != 0):
        raise ValueError(f'benchmark resource limits must be {cpus} CPU(s) and 2 GiB memory with swap disabled')


def case_resource_evidence(before, after, resources, cpus=CASE_RESOURCE_LIMITS['cpus']):
    validate_case_limits(before, cpus)
    validate_case_limits(after, cpus)
    cpu = after['cpu_usage_usec'] - before['cpu_usage_usec']
    throttling = None
    start, end = before.get('cpu_throttling'), after.get('cpu_throttling')
    if start is not None and end is not None:
        deltas = {field: end[field] - start[field] for field in CPU_THROTTLING_FIELDS}
        if min(deltas.values()) >= 0:
            throttling = deltas
    oom = {key: after['memory_events'][key] - before['memory_events'][key] for key in before['memory_events']}
    peak = after['memory_peak_bytes']
    rss = resources['harness_memory']['peak_rss_kib']
    reasons = []
    if cpu < 0 or peak < before['memory_peak_bytes'] or min(oom.values()) < 0:
        reasons.append('cgroup counters went backwards')
    if peak > after['memory_limit_bytes'] or any(oom.values()):
        reasons.append('container exceeded its memory budget or encountered OOM')
    if resources['capture_incomplete'] or not rss:
        reasons.append('harness process memory evidence is incomplete')
    return {'passed': not reasons, 'reasons': reasons, 'scope': 'container_start_to_before_verifier',
            'cpu_seconds': cpu / 1_000_000 if cpu >= 0 else None, 'cpu_throttling': throttling,
            'memory_peak_bytes': peak,
            'memory_limit_bytes': after['memory_limit_bytes'], 'swap_limit_bytes': after['swap_limit_bytes'],
            'memory_event_deltas': oom, 'harness_peak_rss_kib': rss, 'before': before, 'after': after}


def proc_ticks_per_second():
    sysconf = getattr(os, 'sysconf', None)
    return sysconf('SC_CLK_TCK') if sysconf is not None else PROC_TICKS_PER_SECOND_FALLBACK


def peak_rss_kib(*observations):
    return max((value for value in observations
                if type(value) in (int, float) and value >= 0 and math.isfinite(value)), default=None)


def complete_process_io(fields):
    if isinstance(fields, dict) and all(type(fields.get(key)) is int and fields[key] >= 0 for key in IO_FIELDS):
        return {key: fields[key] for key in IO_FIELDS}
    return None


class ResourceSampler:
    def __init__(self, container):
        self.container = container
        self.stopped = threading.Event()
        self.threads = [threading.Thread(target=method, daemon=True) for method in (self.sample_process, self.sample_container)]
        self.data = {'container_scope': 'sdk_and_tools', 'container_samples': 0, 'container_snapshots': [],
                     'process_sampling_errors': 0, 'container_sampling_errors': 0,
                     'sdk_process_samples': 0, 'sdk_peak_rss_kib': None, 'sdk_peak_rss_high_water_kib': None,
                     'sdk_cpu_seconds': None, 'sdk_threads_max': None, 'process_io': None,
                     'process_io_source': None, 'process_io_scope': PROCESS_IO_SCOPE}
        self.previous = None
        self.observed = 0.0
        self.rss_seconds = 0.0

    def start(self):
        self.started = time.monotonic()
        for worker in self.threads:
            worker.start()

    def observe(self, output, when):
        line, _, status = output.partition('\n')
        if ')' not in line:
            return
        stat = line.rsplit(')', 1)[1].split()
        fields = {}
        for line in status.splitlines():
            key, sep, value = line.partition(':')
            parts = value.split()
            if sep and parts and parts[0].isdigit():
                fields[key] = int(parts[0])
        if len(stat) < 20 or 'VmRSS' not in fields:
            return
        identity, rss = stat[19], fields['VmRSS']
        if self.previous is not None:
            last_identity, last_rss, last_time = self.previous
            if identity != last_identity:
                return
            elapsed = when - last_time
            self.rss_seconds += elapsed * (last_rss + rss) / 2
            self.observed += elapsed
        self.previous = (identity, rss, when)
        self.data['sdk_process_samples'] += 1
        self.data['sdk_cpu_seconds'] = (int(stat[11]) + int(stat[12])) / proc_ticks_per_second()
        for source, target in [('VmRSS','sdk_peak_rss_kib'),('VmHWM','sdk_peak_rss_high_water_kib'),('Threads','sdk_threads_max')]:
            if source in fields:
                self.data[target] = max(self.data[target] or 0, fields[source])
        if (io := complete_process_io(fields)) is not None:
            self.data.update(process_io=io, process_io_source=EXTERNAL_PROCESS_IO)

    def sample_process(self):
        while not self.stopped.is_set():
            try:
                result = subprocess.run(['docker','exec',self.container,'sh','-c',PROC], capture_output=True,
                                        text=True, timeout=4)
                if result.returncode == 0 and not self.stopped.is_set():
                    self.observe(result.stdout, time.monotonic())
            except (OSError, ValueError, subprocess.TimeoutExpired):
                self.data['process_sampling_errors'] += 1
            self.stopped.wait(0.5)

    def sample_container(self):
        while not self.stopped.is_set():
            try:
                result = subprocess.run(['docker','stats','--no-stream','--format','{{json .}}',self.container],
                                        capture_output=True,text=True,timeout=5)
                if result.returncode == 0 and not self.stopped.is_set():
                    snapshot = json.loads(result.stdout)
                    self.data['container_samples'] += 1
                    self.data['container_snapshots'].append({key:snapshot.get(key) for key in ('CPUPerc','MemUsage','PIDs','BlockIO')})
                    self.data['container_snapshots'] = self.data['container_snapshots'][-8:]
            except (OSError, ValueError, subprocess.TimeoutExpired):
                self.data['container_sampling_errors'] += 1
            self.stopped.wait(1)

    def finish(self):
        self.stopped.set()
        window = time.monotonic() - self.started
        for worker in self.threads:
            worker.join(timeout=7)
        self.data['capture_incomplete'] = any(worker.is_alive() for worker in self.threads)
        self.data['harness_memory'] = {
            'peak_rss_kib': peak_rss_kib(self.data['sdk_peak_rss_high_water_kib'], self.data['sdk_peak_rss_kib']),
            'mean_rss_kib': round(self.rss_seconds / self.observed, 2) if self.observed else None,
            'observed_seconds': round(self.observed, 4), 'window_seconds':round(window,4),
            'coverage_ratio':round(self.observed/window,6) if window > 0 else None,
            'samples':self.data['sdk_process_samples']}
        return self.data


def include_final_process_usage(resources, result_path):
    try:
        if result_path.stat().st_size > 4 * 1024 * 1024:
            return
        result = json.loads(result_path.read_text())
        stats = result.get('process_resources') if isinstance(result, dict) else None
        if not isinstance(stats, dict):
            return
        if (io := complete_process_io(stats.get('process_io'))) is not None:
            resources.update(process_io=io, process_io_source=FINAL_PROCESS_IO, process_io_scope=PROCESS_IO_SCOPE)
        keys = ('user_cpu_seconds','system_cpu_seconds','peak_rss_kib','input_blocks','output_blocks')
        if not all(type(stats.get(k)) in (int,float) and stats[k]>=0 for k in keys):
            return
        memory = resources['harness_memory']
        memory['peak_rss_kib'] = peak_rss_kib(memory.get('peak_rss_kib'), stats['peak_rss_kib'])
        resources['harness_cpu_seconds'] = stats['user_cpu_seconds'] + stats['system_cpu_seconds']
        resources['harness_self_usage'] = {k:stats[k] for k in keys}
    except (OSError, ValueError, KeyError):
        return
