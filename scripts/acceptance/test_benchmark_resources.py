import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from benchmark_resources import (CASE_MEMORY_BYTES, IO_FIELDS, PROCESS_IO_SCOPE, ResourceSampler,
                                 case_resource_evidence, include_final_process_usage, parse_case_cgroup,
                                 validate_case_limits)
from frontier_report import resource_summary


def proc(rss, hwm, start_time=222, user_ticks=0, system_ticks=0):
    stat = ['S'] + ['0'] * 21
    stat[11] = str(user_ticks)
    stat[12] = str(system_ticks)
    stat[19] = str(start_time)
    return (f"7 (name with spaces) {' '.join(stat)}\nVmRSS: {rss} kB\nVmHWM: {hwm} kB\n"
            "rchar: 12\nwchar: 13\nsyscr: 1\nsyscw: 2\nread_bytes: 0\nwrite_bytes: 4096\ncancelled_write_bytes: 0\n")


class ResourceEvidenceTest(unittest.TestCase):
    def test_cgroup_peaks_and_cpu_are_independent_of_process_rss(self):
        before = parse_case_cgroup(
            f'[cpu.max]\n100000 100000\n[cpu.stat]\nusage_usec 200000\n'
            f'[memory.max]\n{CASE_MEMORY_BYTES}\n[memory.swap.max]\n0\n'
            '[memory.peak]\n14000000\n[memory.events]\noom 0\noom_kill 0\n')
        after = {**before, 'cpu_usage_usec': 2_700_000, 'memory_peak_bytes': 80_000_000}
        resources = {'capture_incomplete': False, 'harness_memory': {'peak_rss_kib': 30_000}}
        evidence = case_resource_evidence(before, after, resources)
        self.assertTrue(evidence['passed'])
        self.assertIsNone(evidence['cpu_throttling'])
        self.assertEqual(evidence['cpu_seconds'], 2.5)
        self.assertEqual(evidence['memory_peak_bytes'], 80_000_000)
        self.assertEqual(evidence['harness_peak_rss_kib'], 30_000)
        with self.assertRaisesRegex(ValueError, '2 CPU'):
            case_resource_evidence(before, after, resources, cpus=2)
        self.assertTrue(case_resource_evidence({**before, 'cpu_quota_usec': 200_000},
                                              {**after, 'cpu_quota_usec': 200_000}, resources, cpus=2)['passed'])
        for invalid in ({'memory_events': {'oom': 1, 'oom_kill': 0}}, {'cpu_usage_usec': 0}):
            self.assertFalse(case_resource_evidence(before, {**after, **invalid}, resources)['passed'])
        self.assertFalse(case_resource_evidence(before, after, {**resources, 'capture_incomplete': True})['passed'])
        with self.assertRaisesRegex(ValueError, 'swap disabled'):
            validate_case_limits({**before, 'swap_limit_bytes': 1024})

    def test_cpu_throttling_is_a_delta_and_distinguishes_zero_from_missing(self):
        def snapshot(stats):
            return parse_case_cgroup(
                '[cpu.max]\n100000 100000\n[cpu.stat]\nusage_usec 1000000\n'
                + stats
                + f'[memory.max]\n{CASE_MEMORY_BYTES}\n[memory.swap.max]\n0\n'
                  '[memory.peak]\n14000000\n[memory.events]\noom 0\noom_kill 0\n')

        before = snapshot('nr_periods 10\nnr_throttled 3\nthrottled_usec 300\n')
        after = snapshot('nr_periods 16\nnr_throttled 9\nthrottled_usec 537819\n')
        resources = {'capture_incomplete': False, 'harness_memory': {'peak_rss_kib': 30_000}}
        evidence = case_resource_evidence(before, after, resources)
        self.assertTrue(evidence['passed'])
        self.assertEqual(evidence['cpu_throttling'], {
            'nr_periods': 6, 'nr_throttled': 6, 'throttled_usec': 537519,
        })
        self.assertEqual(case_resource_evidence(before, before, resources)['cpu_throttling'], {
            'nr_periods': 0, 'nr_throttled': 0, 'throttled_usec': 0,
        })
        for stats in ('nr_periods 16\n',
                      'nr_periods 16\nnr_throttled -1\nthrottled_usec 537819\n',
                      'nr_periods 16\nnr_throttled invalid\nthrottled_usec 537819\n',
                      'nr_periods 9\nnr_throttled 3\nthrottled_usec 301\n'):
            with self.subTest(stats=stats):
                evidence = case_resource_evidence(before, snapshot(stats), resources)
                self.assertIsNone(evidence['cpu_throttling'])
                self.assertTrue(evidence['passed'])

    def test_proc_cpu_ticks_have_windows_fallback_without_sysconf(self):
        sampler = ResourceSampler('unused')
        with patch('benchmark_resources.os.sysconf', None, create=True):
            sampler.observe(proc(10, 30, user_ticks=30, system_ticks=20), 10.0)
        self.assertEqual(sampler.data['sdk_cpu_seconds'], 0.5)

    def test_unequal_intervals_and_missing_samples(self):
        sampler = ResourceSampler('unused')
        sampler.observe(proc(10, 30), 10.0)
        sampler.observe(proc(30, 30), 11.0)
        sampler.observe(proc(50, 50), 14.0)
        sampler.observe(proc(200, 200, start_time=333), 16.0)
        self.assertEqual(sampler.data['sdk_process_samples'], 3)
        self.assertEqual(sampler.rss_seconds / sampler.observed, 35)
        self.assertEqual(sampler.observed, 4)
        self.assertEqual(sampler.data['process_io']['write_bytes'], 4096)
        self.assertEqual(sampler.data['process_io_source'], 'proc_pid_io')
        self.assertEqual(resource_summary(sampler.data)['process_including_waited_children']['io_source'], 'proc_pid_io')
        self.assertEqual(sampler.data['sdk_peak_rss_high_water_kib'], 50)

    def test_end_of_sdk_high_water_and_own_block_counts(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'result.json'
            resources = {'harness_memory': {'peak_rss_kib': 123}}
            path.write_text('{}')
            include_final_process_usage(resources, path)
            self.assertNotIn('harness_self_usage', resources)
            stats = {'peak_rss_kib': 321, 'user_cpu_seconds': 0.25, 'system_cpu_seconds': 0.5,
                     'input_blocks': 0, 'output_blocks': 32}
            path.write_text(json.dumps({'process_resources': stats}))
            include_final_process_usage(resources, path)
            self.assertEqual(resources['harness_memory']['peak_rss_kib'], 321)
            self.assertEqual(resources['harness_cpu_seconds'], 0.75)
            self.assertEqual(resources['harness_self_usage'], stats)
            resources['harness_memory']['peak_rss_kib'] = 512
            include_final_process_usage(resources, path)
            self.assertEqual(resources['harness_memory']['peak_rss_kib'], 512)
            self.assertEqual(resources['harness_self_usage']['peak_rss_kib'], 321)

    def test_complete_final_io_replaces_external_sample_without_rusage(self):
        sampler = ResourceSampler('unused')
        sampler.observe(proc(10, 30), 10.0)
        final_io = dict(zip(IO_FIELDS, (90, 91, 4, 6, 0, 8192, 0)))
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'result.json'
            path.write_text(json.dumps({'process_resources': {'process_io': final_io, 'irrelevant': 1}}))
            include_final_process_usage(sampler.data, path)
        self.assertEqual(sampler.data['process_io'], final_io)
        self.assertEqual(sampler.data['process_io_source'], 'proc_self_io')
        self.assertEqual(sampler.data['process_io_scope'], PROCESS_IO_SCOPE)
        self.assertNotIn('harness_self_usage', sampler.data)
        public = resource_summary(sampler.data)['process_including_waited_children']
        self.assertEqual(public['io'], final_io)
        self.assertEqual(public['io_source'], 'proc_self_io')
        for old in ({'process_io': final_io}, {**sampler.data, 'process_io_source': 'unknown'}):
            with self.subTest(old=old):
                self.assertNotIn('io_source', resource_summary(old)['process_including_waited_children'])

    def test_unavailable_or_incomplete_final_io_preserves_external_sample_or_none(self):
        sampler = ResourceSampler('unused')
        sampler.observe(proc(10, 30), 10.0)
        sampled = sampler.data['process_io'].copy()
        empty = ResourceSampler('unused').data
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'result.json'
            for unavailable in (None, {'read_bytes': 0}, {**sampled, 'syscr': True}, {**sampled, 'syscr': -1}):
                with self.subTest(unavailable=unavailable):
                    path.write_text(json.dumps({'process_resources': {'process_io': unavailable}}))
                    include_final_process_usage(sampler.data, path)
                    include_final_process_usage(empty, path)
                    self.assertEqual(sampler.data['process_io'], sampled)
                    self.assertEqual(sampler.data['process_io_source'], 'proc_pid_io')
                    self.assertIsNone(empty['process_io'])
                    self.assertIsNone(empty['process_io_source'])
