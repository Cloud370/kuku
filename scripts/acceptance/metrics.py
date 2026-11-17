"""Linux process samples for the small public-SDK resource caller."""

import json
import os
from pathlib import Path
import re
import threading
import time


def process_sample(pid):
    path = Path("/proc") / str(pid)
    try:
        fields = (path / "stat").read_text().rsplit(")", 1)[1].split()
        status = (path / "status").read_text()
        values = {}
        for field, key in (("VmRSS", "rss_kib"), ("VmHWM", "rss_high_water_kib"), ("Threads", "threads")):
            match = re.search(rf"^{field}:\s+(\d+)", status, re.MULTILINE)
            values[key] = int(match[1]) if match else None
        ticks = os.sysconf("SC_CLK_TCK")
        values["cpu_s"] = (int(fields[11]) + int(fields[12])) / ticks
        values["reaped_children_cpu_s"] = (int(fields[13]) + int(fields[14])) / ticks
        values["read_bytes"], values["write_bytes"] = None, None
        try:
            for name, value in re.findall(r"^(read_bytes|write_bytes):\s+(\d+)", (path / "io").read_text(), re.MULTILINE):
                values[name] = int(value)
        except OSError:
            pass
        children = set()
        for task in (path / "task").iterdir():
            try:
                children.update(int(value) for value in (task / "children").read_text().split())
            except OSError:
                continue
        values["children"] = children
        return values
    except (OSError, ValueError, IndexError):
        return None


class ProcessSampler:
    def __init__(self, phase_file, interval=0.02):
        self.phase_file, self.interval = phase_file, interval
        self.stopped = threading.Event()
        self.thread = None
        self.phases = {}
        self.last = None
        self.children_seen = set()

    def start(self, pid):
        self.pid = pid
        self.thread = threading.Thread(target=self.sample, daemon=True)
        self.thread.start()

    def phase(self):
        try:
            with self.phase_file.open() as handle:
                phase = json.loads(handle.read(512)).get("phase")
            return phase if isinstance(phase, str) and re.fullmatch(r"[A-Za-z][A-Za-z0-9_-]{0,32}", phase) else "unknown"
        except (OSError, ValueError, AttributeError):
            return "startup"

    def sample(self):
        while not self.stopped.is_set():
            sdk = process_sample(self.pid)
            if sdk is None:
                self.stopped.wait(self.interval)
                continue
            phase = self.phases.setdefault(self.phase(), {"samples": 0, "sdk_peak_rss_kib": None,
                "sdk_threads_max": 0, "tool_peak_rss_kib": 0, "sdk_first_cpu_s": sdk["cpu_s"],
                "sdk_last_cpu_s": sdk["cpu_s"], "sdk_first_read_bytes": sdk.get("read_bytes"),
                "sdk_first_write_bytes": sdk.get("write_bytes"), "tool_samples": 0,
                "first_sample_s": time.monotonic()})
            phase["last_sample_s"] = time.monotonic()
            phase["samples"] += 1
            if sdk["rss_kib"] is not None:
                phase["sdk_peak_rss_kib"] = max(phase["sdk_peak_rss_kib"] or 0, sdk["rss_kib"])
            phase["sdk_threads_max"] = max(phase["sdk_threads_max"], sdk["threads"] or 0)
            phase["sdk_last_cpu_s"] = sdk["cpu_s"]
            for field in ("read_bytes", "write_bytes"):
                phase["sdk_last_" + field] = sdk.get(field)
            pending, seen, rss = list(sdk["children"]), set(), 0
            while pending:
                pid = pending.pop()
                if pid in seen:
                    continue
                seen.add(pid)
                child = process_sample(pid)
                if child:
                    self.children_seen.add(pid)
                    rss += child["rss_kib"] or 0
                    phase["tool_samples"] += 1
                    pending.extend(child["children"])
            phase["tool_peak_rss_kib"] = max(phase["tool_peak_rss_kib"], rss)
            self.last = {key: value for key, value in sdk.items() if key != "children"}
            self.stopped.wait(self.interval)

    def finish(self):
        self.stopped.set()
        if self.thread:
            self.thread.join(timeout=5)
        for phase in self.phases.values():
            phase["sdk_sampled_cpu_delta_s"] = round(phase["sdk_last_cpu_s"] - phase["sdk_first_cpu_s"], 6)
            phase["sampled_span_s"] = round(phase["last_sample_s"] - phase["first_sample_s"], 6)
        return {"scope": "SDK caller including fixed in-process HTTP fixture; tool descendants are separate",
                "interval_s": self.interval, "cpu_clock_ticks_per_s": os.sysconf("SC_CLK_TCK"),
                "sampled_phases": self.phases, "last_sdk_sample": self.last,
                "observed_tool_processes": len(self.children_seen),
                "capture_incomplete": bool(self.thread and self.thread.is_alive()),
                "limitations": ["short-lived child processes can fall between samples",
                                "phase CPU and RSS are sampled; operation latency comes from the SDK caller",
                                "unavailable /proc I/O counters remain null; logical file sizes are recorded separately",
                                "cold initial open does not flush the operating-system filesystem cache"]}
