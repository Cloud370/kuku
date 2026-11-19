import json
from pathlib import Path
import tempfile
import unittest

from benchmark_resources import include_final_process_usage, ResourceSampler


def proc(rss, hwm, start_time=222):
    stat = ['S'] + ['0'] * 21
    stat[19] = str(start_time)
    return (f"7 (name with spaces) {' '.join(stat)}\nVmRSS: {rss} kB\nVmHWM: {hwm} kB\n"
            "rchar: 12\nwchar: 13\nsyscr: 1\nsyscw: 2\nread_bytes: 0\nwrite_bytes: 4096\ncancelled_write_bytes: 0\n")


class ResourceEvidenceTest(unittest.TestCase):
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
