"""Native process samples for the public SDK resource caller."""

from collections import defaultdict
import ctypes
import json
import os
import platform
from pathlib import Path
import re
from statistics import median
import sys
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


def smaps_rollup(pid):
    try:
        fields = (Path("/proc") / str(pid) / "smaps_rollup").read_text()
    except OSError:
        return None
    selected = ("Rss", "Pss", "Pss_Anon", "Pss_File", "Pss_Shmem", "Anonymous", "Private_Dirty")
    return {f"{name.lower()}_kib": int(value) for name, value in
            re.findall(r"^(\w+):\s+(\d+) kB$", fields, re.MULTILINE) if name in selected} or None


class WindowsProcesses:
    def __init__(self):
        from ctypes import wintypes

        class ProcessEntry(ctypes.Structure):
            _fields_ = [("size", wintypes.DWORD), ("usage", wintypes.DWORD),
                        ("pid", wintypes.DWORD), ("heap", ctypes.c_size_t),
                        ("module", wintypes.DWORD), ("threads", wintypes.DWORD),
                        ("parent", wintypes.DWORD), ("priority", wintypes.LONG),
                        ("flags", wintypes.DWORD), ("name", wintypes.WCHAR * 260)]

        class MemoryCounters(ctypes.Structure):
            _fields_ = [("size", wintypes.DWORD), ("faults", wintypes.DWORD)] + [
                (name, ctypes.c_size_t) for name in ("peak", "working_set", "peak_paged_pool", "paged_pool",
                                                   "peak_nonpaged_pool", "nonpaged_pool", "pagefile", "peak_pagefile")]

        self.process_entry = ProcessEntry
        self.memory_counters = MemoryCounters
        self.filetime = wintypes.FILETIME
        self.invalid_handle = ctypes.c_void_p(-1).value
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)

        def bind(name, argtypes, restype):
            function = getattr(kernel, name)
            function.argtypes = argtypes
            function.restype = restype
            return function

        self.create_snapshot = bind("CreateToolhelp32Snapshot", [wintypes.DWORD, wintypes.DWORD], wintypes.HANDLE)
        snapshot_args = [wintypes.HANDLE, ctypes.POINTER(ProcessEntry)]
        self.first_process = bind("Process32FirstW", snapshot_args, wintypes.BOOL)
        self.next_process = bind("Process32NextW", snapshot_args, wintypes.BOOL)
        self.open_process = bind("OpenProcess", [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD], wintypes.HANDLE)
        self.close_handle = bind("CloseHandle", [wintypes.HANDLE], wintypes.BOOL)
        self.process_times = bind("GetProcessTimes", [wintypes.HANDLE] + [ctypes.POINTER(self.filetime)] * 4, wintypes.BOOL)
        self.process_memory = bind("K32GetProcessMemoryInfo",
                                   [wintypes.HANDLE, ctypes.POINTER(MemoryCounters), wintypes.DWORD], wintypes.BOOL)

    def snapshot(self):
        th32cs_snapprocess = 0x00000002
        snapshot = self.create_snapshot(th32cs_snapprocess, 0)
        if snapshot == self.invalid_handle:
            raise ctypes.WinError(ctypes.get_last_error())
        entry = self.process_entry()
        entry.size = ctypes.sizeof(entry)
        children, threads = defaultdict(set), {}
        try:
            found = self.first_process(snapshot, ctypes.byref(entry))
            while found:
                threads[entry.pid] = entry.threads
                children[entry.parent].add(entry.pid)
                found = self.next_process(snapshot, ctypes.byref(entry))
        finally:
            self.close_handle(snapshot)

        def read(pid):
            return self.sample(pid, threads.get(pid), children.get(pid, set()))

        return read

    def sample(self, pid, threads, children):
        if threads is None:
            return None
        process_query_limited_information = 0x1000
        handle = self.open_process(process_query_limited_information, False, pid)
        if not handle:
            return None
        try:
            creation, exit_time, system, user = [self.filetime() for _ in range(4)]
            if not self.process_times(handle, ctypes.byref(creation), ctypes.byref(exit_time),
                                      ctypes.byref(system), ctypes.byref(user)):
                return None
            cpu = sum((counter.dwHighDateTime << 32) | counter.dwLowDateTime for counter in (system, user)) / 10_000_000
            memory = self.memory_counters()
            memory.size = ctypes.sizeof(memory)
            have_memory = self.process_memory(handle, ctypes.byref(memory), memory.size)
            return {"cpu_s": cpu, "threads": threads, "children": children,
                    "rss_kib": memory.working_set // 1024 if have_memory else None,
                    "rss_high_water_kib": memory.peak // 1024 if have_memory else None,
                    "read_bytes": None, "write_bytes": None, "reaped_children_cpu_s": None}
        finally:
            self.close_handle(handle)


class ProcessSampler:
    def __init__(self, phase_file, interval=0.02):
        self.phase_file, self.interval = phase_file, interval
        self.stopped = threading.Event()
        self.thread = None
        self.phases = {}
        self.last = None
        self.children_seen = set()
        self.error = None
        self.windows = WindowsProcesses() if sys.platform == "win32" else None

    def start(self, pid):
        self.pid = pid
        self.thread = threading.Thread(target=self.sample, daemon=True)
        self.thread.start()

    def phase(self):
        try:
            with self.phase_file.open() as handle:
                phase = json.loads(handle.read(512)).get("phase")
            return phase if isinstance(phase, str) and re.fullmatch(r"[A-Za-z][A-Za-z0-9_-]{0,32}", phase) else None
        except FileNotFoundError:
            return "startup"
        except (OSError, ValueError, AttributeError):
            return None

    def sample(self):
        try:
            while not self.stopped.is_set():
                self.sample_once()
                self.stopped.wait(self.interval)
        except Exception as error:
            self.error = repr(error)

    def sample_once(self):
        name = self.phase()
        reader = self.windows.snapshot() if self.windows else process_sample
        sdk = reader(self.pid)
        if name is None or sdk is None or name != self.phase():
            return
        stamp = time.monotonic()
        phase = self.phases.setdefault(name, {"samples": 0, "sdk_rss_samples": [],
            "sdk_threads_max": 0, "tool_peak_rss_kib": 0, "sdk_first_cpu_s": sdk["cpu_s"],
            "sdk_first_read_bytes": sdk["read_bytes"], "sdk_first_write_bytes": sdk["write_bytes"],
            "tool_samples": 0, "tool_processes_max": 0, "first_sample_s": stamp})
        phase["last_sample_s"] = stamp
        phase["samples"] += 1
        if sdk["rss_kib"] is not None:
            phase["sdk_rss_samples"].append(sdk["rss_kib"])
        if not self.windows and phase["samples"] in (2, 32):
            phase["sdk_smaps_rollup"] = smaps_rollup(self.pid)
        phase["sdk_threads_max"] = max(phase["sdk_threads_max"], sdk["threads"] or 0)
        phase["sdk_last_cpu_s"] = sdk["cpu_s"]
        for field in ("read_bytes", "write_bytes"):
            phase["sdk_last_" + field] = sdk[field]
        pending, seen, rss, processes = list(sdk["children"]), {self.pid}, 0, 0
        while pending:
            pid = pending.pop()
            if pid in seen:
                continue
            seen.add(pid)
            child = reader(pid)
            if child:
                self.children_seen.add(pid)
                rss += child["rss_kib"] or 0
                processes += 1
                pending.extend(child["children"])
        phase["tool_peak_rss_kib"] = max(phase["tool_peak_rss_kib"], rss)
        phase["tool_processes_max"] = max(phase["tool_processes_max"], processes)
        phase["tool_samples"] += processes
        self.last = {key: value for key, value in sdk.items() if key != "children"}

    def finish(self):
        self.stopped.set()
        if self.thread:
            self.thread.join(timeout=5)
        for phase in self.phases.values():
            cpu = phase["sdk_last_cpu_s"] - phase["sdk_first_cpu_s"]
            span = phase["last_sample_s"] - phase["first_sample_s"]
            rss = phase.pop("sdk_rss_samples")
            phase["sdk_sampled_cpu_delta_s"] = round(cpu, 6)
            phase["sampled_span_s"] = round(span, 6)
            phase["sdk_percent_of_one_cpu"] = round(100 * cpu / span, 3) if span else None
            phase["sdk_peak_rss_kib"] = max(rss, default=None)
            phase["sdk_median_rss_kib"] = median(rss) if rss else None
        limitations = ["short-lived child processes can fall between samples",
                       "phase CPU and resident memory are sampled; operation latency comes from the SDK caller",
                       "logical file sizes are recorded separately",
                       "cold initial open does not flush the operating-system filesystem cache"]
        limitations.append("Windows I/O counters and reaped child CPU are uncollected" if self.windows else
                           "unavailable /proc I/O counters remain null")
        return {"scope": "SDK caller including fixed in-process HTTP fixture; tool descendants are separate",
                "native_backend": "win32" if self.windows else "procfs",
                "interval_s": self.interval, "cpu_clock_ticks_per_s": None if self.windows else os.sysconf("SC_CLK_TCK"),
                "sampled_phases": self.phases, "last_sdk_sample": self.last,
                "observed_tool_processes": len(self.children_seen),
                "capture_incomplete": self.error is not None or bool(self.thread and self.thread.is_alive()),
                "error": self.error, "limitations": limitations}


def metrics_host(args):
    host = {"platform": platform.platform(), "logical_cpus": os.cpu_count()}
    prefix = []
    if not sys.platform.startswith("linux"):
        if args.cpu_count is not None or args.address_space_mib is not None:
            raise ValueError("CPU affinity and address-space limits for metrics require Linux")
        return host, prefix

    import resource

    allowed = sorted(os.sched_getaffinity(0))
    host["available_cpu_ids"] = allowed
    host["physical_memory_bytes"] = os.sysconf("SC_PHYS_PAGES") * os.sysconf("SC_PAGE_SIZE")
    for variable in ("MALLOC_ARENA_MAX", "MALLOC_MMAP_THRESHOLD_"):
        value = os.environ.get(variable, "")
        if value.isascii() and value.isdecimal():
            host[variable] = int(value)
    cgroup_root = Path("/sys/fs/cgroup")
    membership = Path("/proc/self/cgroup").read_text(encoding="ascii").splitlines()
    relative = next((line[3:].lstrip("/") for line in membership if line.startswith("0::")), "")
    for field in ("cpu.max", "memory.max"):
        for path in (cgroup_root / relative / field, cgroup_root / field):
            if path.is_file():
                host[field] = path.read_text(encoding="ascii").strip()
                break
    selected = allowed
    if args.cpu_count is not None:
        if not 1 <= args.cpu_count <= len(allowed):
            raise ValueError("--cpu-count must fit the available CPU affinity")
        selected = allowed[:args.cpu_count]
        prefix += ["taskset", "-c", ",".join(map(str, selected))]
    host["caller_cpu_ids"] = selected
    current_limit = resource.getrlimit(resource.RLIMIT_AS)[0]
    host["caller_address_space_limit_bytes"] = current_limit if current_limit >= 0 else None
    if args.address_space_mib is not None:
        if args.address_space_mib <= 0:
            raise ValueError("--address-space-mib must be positive")
        limit = args.address_space_mib * 1024 * 1024
        prefix += ["prlimit", f"--as={limit}:{limit}", "--"]
        host["caller_address_space_limit_bytes"] = limit
    return host, prefix


def run_metrics(args, directory, record):
    from run import ROOT, capture, command_environment, config_record, require_success

    if not (sys.platform.startswith("linux") or sys.platform == "win32"):
        raise ValueError("process resource sampling requires Linux or Windows; the Rust caller is portable")
    record["host"], prefix = metrics_host(args)
    target_root = ROOT / "target"
    target = (ROOT / os.environ.get("CARGO_TARGET_DIR", target_root)).resolve()
    if not target.is_relative_to(target_root):
        raise ValueError("metrics CARGO_TARGET_DIR must be in this checkout's target directory")
    environment = command_environment()
    environment["CARGO_TARGET_DIR"] = str(target)
    command = ["cargo", "build", "-p", "kuku", "--example", "resource_probe", "--features", "test_support"]
    if args.profile == "release":
        command.append("--release")
    record["build"] = require_success(capture(command, directory, "build", timeout=1800, env=environment))
    record["build"]["target_dir"] = str(target)
    record["build"]["profile"] = args.profile
    record["sources"] = [config_record(path) for path in
                          (ROOT / "crates/kuku/examples/resource_probe.rs",
                           ROOT / "crates/kuku/examples/resource_probe/workspace.rs",
                           Path(__file__).with_name("run.py"), Path(__file__),
                           ROOT / "crates/kuku/tests/common/mod.rs")]
    binary_name = "resource_probe.exe" if sys.platform == "win32" else "resource_probe"
    binary = target / args.profile / "examples" / binary_name
    record["binary"] = config_record(binary)
    phase_file, result_file = directory / "phase.json", directory / "caller.json"
    command = prefix + [str(binary), "--workspace", str(directory / "workspace"), "--data-dir", str(directory / "session-data"),
               "--phase-file", str(phase_file), "--output", str(result_file),
               "--idle-agents", str(args.idle_agents), "--appends", str(args.appends),
               "--concurrent-agents", str(args.concurrent_agents), "--output-bytes", str(args.output_bytes),
               "--output-quota", str(args.output_quota), "--repeated-tasks", str(args.repeated_tasks),
               "--script-workers", str(args.script_workers), "--script-host-operations", str(args.script_host_operations),
               "--pressure-rounds", str(args.pressure_rounds)]
    sampler = ProcessSampler(phase_file)
    try:
        record["execution"] = capture(command, directory, "caller", timeout=180,
                                       env=environment, on_start=sampler.start)
    finally:
        record["resources"] = sampler.finish()
    caller = json.loads(result_file.read_text(encoding="utf-8"))
    record["caller_result"] = str(result_file)
    record["measurements"] = caller.get("measurements")
    record["passed"] = (caller.get("passed") is True and record["execution"]["exit_code"] == 0
                        and not record["execution"]["timed_out"] and not record["execution"].get("capture_incomplete")
                        and not record["resources"]["capture_incomplete"])
    record["missing_measurements"] = [key for key in ("context_prepare_us", "request_prepare_us")
                                      if caller.get("measurements", {}).get(key) is None]
    phases = record["resources"]["sampled_phases"]
    stress_peaks = ["script_peak"] + [f"script_peak_{index}" for index in range(2, args.pressure_rounds + 1)]
    stress_idle = [f"after_pressure_idle_{index}" for index in range(1, args.pressure_rounds + 1)]
    for phase in ("fixture_baseline", "single_idle_agent", "idle_agents", "large_output", "concurrent_agents",
                  "post_direct_idle", "post_analysis_idle", "before_pressure_idle", "post_task_idle", *stress_peaks, *stress_idle):
        if phases.get(phase, {}).get("sdk_median_rss_kib") is None or phases[phase]["samples"] < 2:
            record["missing_measurements"].append("process_samples_" + phase)
    workload = caller.get("workspace_workload", {})
    observed_peaks = [phases.get(phase, {}).get("tool_processes_max") for phase in stress_peaks]
    if workload and all(peak is not None for peak in observed_peaks):
        capacity = min(args.script_host_operations, args.script_workers * workload["stress_commands_per_worker"])
        record["host_operation_limit"] = {"configured": args.script_host_operations,
                                          "possible_peak_for_workload": capacity,
                                          "observed_tool_process_peak": max(observed_peaks),
                                          "per_round_tool_process_peaks": observed_peaks}
        if max(observed_peaks) < capacity:
            record["missing_measurements"].append("script_host_operation_saturation")
        if max(observed_peaks) > args.script_host_operations:
            record["passed"] = False
    else:
        record["missing_measurements"].append("script_host_operation_peak")
    record["measurements_complete"] = not record["missing_measurements"]
