#!/usr/bin/env python3
"""Small selectors for repository checks and unchanged Terminal-Bench tasks."""

import argparse
import contextlib
import csv
import datetime
import errno
import hashlib
import io
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import uuid
import zipfile

from benchmark_resources import (CASE_RESOURCE_LIMITS, ResourceSampler, case_resource_evidence, include_final_process_usage,
                                 read_case_cgroup, validate_case_limits)
from benchmark_runtime import (attempt_passed, bundle_ca, file_hashes, fresh_verifier_directory,
                               inspect_reference_agent, prepare_benchmark_agent, reward, verifier_passed, verifier_summary)
from evidence import FailureSummary, inspect_sdk


ROOT = Path(__file__).resolve().parents[2]
MANIFEST = Path(__file__).with_name("cases.json")
OUTPUT = ROOT / "benchmarks" / "acceptance"
LOG_BYTES = 65536
PROTOCOL_TESTS = (
    "provider_adapters", "provider_config", "provider_error", "provider_integration",
    "provider_trace_integration", "cache_runtime_notices",
)


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    with temporary.open("w", encoding="utf-8") as handle:
        json.dump(value, handle, indent=2, ensure_ascii=False, allow_nan=False)
        handle.write("\n")
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(temporary, path)


@contextlib.contextmanager
def locked(path, *, wait_seconds=0):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a+b") as handle:
        if os.name == "nt":
            import msvcrt
            handle.seek(0)
            handle.write(b"0")
            handle.flush()
            handle.seek(0)
        else:
            import fcntl
        deadline = time.monotonic() + wait_seconds
        while True:
            try:
                if os.name == "nt":
                    msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
                else:
                    fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except OSError as error:
                contended = isinstance(error, BlockingIOError) or (
                    os.name == "nt" and error.errno in (errno.EACCES, errno.EDEADLK))
                if not contended or time.monotonic() >= deadline:
                    raise
                time.sleep(0.05)
        try:
            yield
        finally:
            if os.name == "nt":
                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(handle, fcntl.LOCK_UN)


def money(value):
    number = float(value)
    if not math.isfinite(number) or number < 0:
        raise ValueError("cost must be a finite nonnegative USD amount")
    return round(number, 9)


class Budget:
    def __init__(self, path):
        self.path = path

    @contextlib.contextmanager
    def edit(self):
        with locked(self.path.with_suffix(".lock")):
            if not self.path.exists():
                raise ValueError("initialize the task's single budget ledger before paid runs")
            data = json.loads(self.path.read_text(encoding="utf-8"))
            yield data
            write_json(self.path, data)

    def initialize(self, authorization, limit, opening):
        with locked(self.path.with_suffix(".lock")):
            if self.path.exists():
                raise ValueError("budget ledger already exists; authorization is not renewed")
            limit, opening = money(limit), money(opening)
            if opening > limit:
                raise ValueError("opening spend exceeds authorization")
            write_json(self.path, {
                "schema_version": 1, "authorization": authorization,
                "limit_usd": limit, "opening_spent_usd": opening,
                "created_at": now(), "entries": [],
            })

    @staticmethod
    def committed(data):
        return money(data["opening_spent_usd"] + sum(
            entry.get("charged_usd", entry["reserved_usd"]) for entry in data["entries"]
        ))

    def reserve(self, run_id, upper_bound, reason):
        amount = money(upper_bound)
        if amount <= 0:
            raise ValueError("paid runs require a positive conservative request upper bound")
        with self.edit() as data:
            if any(entry["run_id"] == run_id for entry in data["entries"]):
                raise ValueError("run already reserved; do not repeat paid execution")
            if self.committed(data) + amount > data["limit_usd"] + 1e-9:
                raise ValueError("insufficient authorized budget; no paid process was started")
            data["entries"].append({
                "run_id": run_id, "reserved_usd": amount, "status": "reserved",
                "basis": "conservative_upper_bound", "reason": reason, "created_at": now(),
            })

    def settle(self, run_id, amount=None, basis="conservative_upper_bound", evidence=None):
        with self.edit() as data:
            entry = next((item for item in data["entries"] if item["run_id"] == run_id), None)
            if entry is None:
                raise ValueError("unknown budget reservation")
            charge = entry["reserved_usd"] if amount is None else money(amount)
            if basis == "conservative_upper_bound" and charge != entry["reserved_usd"]:
                raise ValueError("missing usage keeps the complete reservation charged")
            if charge > entry["reserved_usd"] + 1e-9:
                raise ValueError("reported cost exceeds reservation; stop paid work and reconcile")
            if basis != "conservative_upper_bound" and not evidence:
                raise ValueError("reported or estimated cost requires an evidence reference")
            entry.update(charged_usd=charge, basis=basis, status="settled", settled_at=now())
            if evidence:
                entry["evidence"] = evidence


def load_secrets(path):
    values = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        match = re.fullmatch(r"(?:export\s+)?(KUKU_TEST_[A-Z0-9_]+)=(.*)", line)
        if not match:
            raise ValueError("credential file accepts only KUKU_TEST_ assignments, without shell evaluation")
        value = match[2].strip()
        if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
            value = value[1:-1]
        if value:
            values[match[1]] = value
    return values


def redact(text, secrets=()):
    for secret in sorted(set(secrets), key=len, reverse=True):
        if secret:
            text = text.replace(secret, "[REDACTED]")
    text = re.sub(r"(?i)(bearer\s+)[^\s\"']+", r"\1[REDACTED]", text)
    text = re.sub(r'(?i)([\"\']?(?:api[_-]?key|authorization)[\"\']?\s*[:=]\s*)[^\s,}]+', r'\1[REDACTED]', text)
    return text


def capture(command, directory, label, timeout=900, env=None, secrets=(), cwd=ROOT, on_start=None,
            retain_failures=False):
    directory.mkdir(parents=True, exist_ok=True)
    start = time.monotonic()
    flags = {"start_new_session": True} if os.name != "nt" else {
        "creationflags": subprocess.CREATE_NEW_PROCESS_GROUP}
    process = subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, bufsize=0, **flags)
    if on_start:
        on_start(process.pid)
    tail = bytearray()
    total = 0
    digest = hashlib.sha256()
    failures = FailureSummary() if retain_failures else None

    def drain():
        nonlocal total
        while True:
            try:
                chunk = os.read(process.stdout.fileno(), 8192)
            except (OSError, ValueError):
                break
            if not chunk:
                break
            total += len(chunk)
            digest.update(chunk)
            if failures is not None:
                failures.feed(chunk)
            tail.extend(chunk)
            del tail[:-LOG_BYTES]

    def terminate_tree():
        if os.name == "nt":
            subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)
        else:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(process.pid, signal.SIGKILL)

    reader = threading.Thread(target=drain, daemon=True)
    reader.start()
    timed_out = False
    try:
        process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        timed_out = True
        terminate_tree()
        process.wait(timeout=10)
    finally:
        reader.join(timeout=1)
        if reader.is_alive():
            terminate_tree()
            reader.join(timeout=5)
        process.stdout.close()
    raw_tail = tail.decode("utf-8", errors="replace")
    if total > LOG_BYTES:
        for secret in secrets:
            for length in range(min(len(secret) - 1, len(raw_tail)), 0, -1):
                if raw_tail.startswith(secret[-length:]):
                    raw_tail = "[REDACTED]" + raw_tail[length:]
                    break
    rendered = redact(raw_tail, secrets).encode("utf-8")
    output = rendered[-LOG_BYTES:].decode("utf-8", errors="ignore").encode("utf-8")
    path = directory / f"{label}.log"
    path.write_bytes(output)
    result = {
        "command": [redact(str(arg), secrets) for arg in command],
        "exit_code": process.returncode, "timed_out": timed_out,
        "seconds": round(time.monotonic() - start, 4),
        "log": str(path), "output_bytes": total,
        "retained_bytes": path.stat().st_size,
        "truncated": total > LOG_BYTES or len(rendered) > LOG_BYTES,
        "output_sha256": digest.hexdigest(),
    }
    if reader.is_alive():
        result["capture_incomplete"] = True
    if failures is not None:
        result.update(failures.write(directory, label, lambda text: redact(text, secrets)))
    return result


def require_success(result):
    if result["exit_code"] != 0 or result["timed_out"] or result.get("capture_incomplete"):
        raise RuntimeError(f"command failed; bounded log: {result['log']}")
    return result


def tree_state():
    def git(*args):
        return subprocess.check_output(["git", *args], cwd=ROOT, stderr=subprocess.DEVNULL)
    return {
        "root": str(ROOT), "commit": git("rev-parse", "HEAD").decode().strip(),
        "branch": git("branch", "--show-current").decode().strip(),
        "dirty": bool(git("status", "--porcelain")),
        "tracked_diff_sha256": hashlib.sha256(git("diff", "HEAD", "--binary")).hexdigest(),
        "system": platform.platform(), "python": platform.python_version(),
    }


def config_record(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(65536):
            digest.update(chunk)
    return {"path": str(path.resolve()), "sha256": digest.hexdigest()}


def command_environment(secrets=None):
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(ROOT / "target")
    env.update(secrets or {})
    return env


def manifest_case(case_id, suite="legacy"):
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    if suite != "legacy":
        selection = manifest["suites"][suite]
        available = {case["id"]: case for case in manifest["cases"] + selection["additional_cases"]}
        manifest = {"source": selection["source"], "cases": [available[name] for name in selection["case_ids"]]}
    case = next((case for case in manifest["cases"] if case["id"] == case_id), None)
    if case is None:
        raise ValueError(f"unknown benchmark case for {suite}: {case_id}")
    return manifest, case


def prepare_source(manifest, directory, case=None):
    from benchmark_source import prepare_source as checkout
    return checkout(ROOT, manifest, directory, case)


def retain_task_artifacts(case, container, attempt):
    destination = attempt / "artifacts"
    destination.mkdir()
    records = []
    for index, path in enumerate(case["artifacts"]):
        info = subprocess.run(["docker", "exec", container, "du", "-sb", "--", path],
                              capture_output=True, text=True, timeout=10)
        item = {"task_path": path}
        if info.returncode != 0:
            item["status"] = "missing"
        else:
            item["bytes"] = int(info.stdout.split()[0])
            if item["bytes"] > 32 * 1024 * 1024:
                item["status"] = "over_32_mib_retention_limit"
            else:
                target = destination / str(index)
                result = capture(["docker", "cp", f"{container}:{path}", str(target)],
                                 attempt, f"artifact-{index}", timeout=30)
                item.update(status="retained" if result["exit_code"] == 0 else "copy_failed",
                            path=str(target), copy=result)
                if target.is_file():
                    item["sha256"] = hashlib.sha256(target.read_bytes()).hexdigest()
                elif target.is_dir():
                    item["files_sha256"] = file_hashes(target)
        records.append(item)
    return records


def benchmark_attempt(args, case, source, image, directory, mode, secrets, *, sdk_timeout_sec=None, on_sdk_finished=None):
    attempt = directory / mode
    logs = attempt / "logs"
    (logs / "verifier").mkdir(parents=True)
    (logs / "agent").mkdir()
    container = "kuku-acceptance-" + uuid.uuid4().hex[:12]
    cpus = getattr(args, "cpus", CASE_RESOURCE_LIMITS["cpus"])
    memory = f"{CASE_RESOURCE_LIMITS['memory_bytes'] // 1024**2}m"
    command = ["docker", "run", "-d", "--name", container,
               "--cpus", str(cpus), "--memory", memory,
               "--mount", f"type=bind,source={logs},target=/logs"]
    runner_command = None
    reference = mode == "live" and any(getattr(args, name, None) for name in ("pi_prefix", "codex_prefix"))
    strict = mode == "live" and (reference or getattr(args, "require_resource_evidence", False))
    if strict:
        command += ["--memory-swap", memory]
    record = {"mode": mode, "resources": {"scope": "benchmark_container_including_sdk_and_tools"}}
    if strict:
        record["resource_requirements"] = {**CASE_RESOURCE_LIMITS, "cpus": cpus}
    env = os.environ.copy()
    env.update(secrets)
    if mode == "live":
        data_dir = attempt / "session-data"
        data_dir.mkdir()
        runner_command, runner_args, config_path = prepare_benchmark_agent(args, case, source, attempt, record)
        if reference:
            command += ["--workdir", case["workspace"]]
        command += runner_args
        record["runtime_ca"] = bundle_ca(attempt / "runtime")
        record["runtime_files"]["ca-certificates.crt"] = record["runtime_ca"]["sha256"]
        command += ["--mount", f"type=bind,source={attempt / 'runtime'},target=/kuku/runtime,readonly",
                    "--mount", f"type=bind,source={data_dir},target=/kuku/data",
                    "--mount", f"type=bind,source={args.config.resolve()},target={config_path},readonly"]
        for name in secrets:
            command += ["--env", name]
        command += ["--env", "SSL_CERT_FILE=" + record["runtime_ca"]["runtime_path"]]
        record["session_data"] = str(data_dir)
    command += [image, "sleep", "infinity"]
    created = False
    try:
        require_success(capture(command, attempt, "container-start", env=env, secrets=secrets.values()))
        created = True
        if mode == "oracle":
            require_success(capture(["docker", "cp", str(source / "solution"), f"{container}:/solution"],
                                    attempt, "oracle-copy"))
            record["execution"] = capture(["docker", "exec", container, "bash", "/solution/solve.sh"],
                                           attempt, "oracle", timeout=case["timeout_sec"])
        elif mode == "live":
            if strict:
                try:
                    before = read_case_cgroup(container)
                    validate_case_limits(before, cpus)
                except ValueError as error:
                    record.update(passed=False, failure_kind="resource_evidence",
                                  resource_evidence={"passed": False, "reasons": [str(error)]})
                    return record
            sampler = ResourceSampler(container)
            sampler.start()
            try:
                record["execution"] = capture([
                    "docker", "exec", container, "sh", "-c",
                    'printf "%s\\n" "$$" > /logs/agent/sdk.pid; exec "$@"', "sh", *runner_command,
                ], attempt, record["harness"] if reference else "sdk", timeout=sdk_timeout_sec or case["timeout_sec"],
                                               secrets=secrets.values())
            finally:
                if on_sdk_finished is not None:
                    on_sdk_finished()
                record["resources"] = sampler.finish()
            process_result = logs / "agent" / "result.json"
            include_final_process_usage(record["resources"], process_result)
            if strict:
                try:
                    record["resource_evidence"] = case_resource_evidence(before, read_case_cgroup(container),
                                                                         record["resources"], cpus)
                except ValueError as error:
                    record["resource_evidence"] = {"passed": False, "reasons": [str(error)], "before": before}
            if reference:
                evidence_key = "agent_evidence"
                record[evidence_key] = inspect_reference_agent(record["harness"], data_dir, source / "instruction.md")
            else:
                evidence_key = "sdk_evidence"
                record["sdk_result"] = str(process_result)
                record[evidence_key] = inspect_sdk(process_result, data_dir, args.connection,
                                                    "/kuku/data", record["session_id"], args.require_streaming,
                                                    require_fixed_limits=args.suite == "legacy", require_output_limit=True,
                                                    expected_max_command_seconds=record["max_command_seconds"],
                                                    expected_max_request_bytes=getattr(args, "max_request_bytes", None))
        else:
            record["execution"] = {"exit_code": 0, "scope": "untouched environment; no Agent or oracle"}
        sdk = record.get("sdk_evidence", {})
        timed_out = record["execution"].get("timed_out") or sdk.get("task_deadline_elapsed")
        if timed_out and not sdk.get("durable_cleanup_closed"):
            record.update(passed=False, failure_kind="runtime",
                          verification={"skipped": "runner timed out without a verified Session close"})
            return record
        if mode == "live":
            record["task_artifacts"] = retain_task_artifacts(case, container, attempt)
        fresh_verifier_directory(logs)
        record["verifier_directory_reset_at"] = now()
        require_success(capture(["docker", "cp", str(source / "tests"), f"{container}:/tests"],
                                attempt, "verifier-copy"))
        record["verification"] = capture(["docker", "exec", container, "bash", "/tests/test.sh"],
                                          attempt, "verifier", timeout=case["timeout_sec"])
        record["reward"] = reward(logs)
        record["verifier_summary"] = verifier_summary(logs)
        record["original_verifier_passed"] = verifier_passed("live", record["verification"],
                                                            record["reward"], record["verifier_summary"])
        record["task_passed"] = not timed_out and attempt_passed(
            mode, record["execution"], record["verification"], record["reward"], record["verifier_summary"])
        agent_passed = mode != "live" or record[evidence_key]["passed"]
        resources_passed = not strict or record["resource_evidence"]["passed"]
        record["passed"] = record["task_passed"] and agent_passed and resources_passed
        if timed_out:
            record["failure_kind"] = "runtime"
        elif record["reward"] is None or not record["verifier_summary"] or record["verification"]["timed_out"]:
            record["failure_kind"] = "infrastructure"
        elif record["execution"]["exit_code"] != 0:
            record["failure_kind"] = "runtime"
        elif not agent_passed:
            record["failure_kind"] = evidence_key
        elif not resources_passed:
            record["failure_kind"] = "resource_evidence"
        elif mode == "live" and any(item["status"] != "retained" for item in record["task_artifacts"]):
            record.update(passed=False, failure_kind="artifact_retention")
        elif not record["passed"]:
            record["failure_kind"] = "agent_task" if mode == "live" else "verifier_self_check"
        return record
    finally:
        if created:
            record["cleanup"] = capture(["docker", "rm", "-f", container], attempt, "cleanup", timeout=30)
            record["cleanup_passed"] = record["cleanup"]["exit_code"] == 0 and not record["cleanup"]["timed_out"]
            if not record["cleanup_passed"]:
                record.update(passed=False, failure_kind="cleanup")
        write_json(attempt / "result.json", record)


def run_benchmark(args, directory, record, secrets):
    manifest, case = manifest_case(args.case, args.suite)
    spec = manifest["source"]
    source = prepare_source(manifest, directory, case) / spec.get("task_root", "") / case["id"]
    frontier = args.suite == "frontier"
    published_image = case.get("published_image")
    image = published_image or f"kuku-acceptance/{case['id']}:{spec['revision'][:12]}"
    record["benchmark"] = {"suite": args.suite, "source": spec, "case": case,
                           "task_files_sha256": file_hashes(source), "image_tag": image}
    image_ref = image
    if published_image:
        require_success(capture(["docker", "pull", published_image], directory, "image-pull", timeout=600))
    else:
        image_id_file = directory / "image-id.txt"
        build = ["docker", "build", "--force-rm", "--tag", image]
        if frontier:
            build += ["--iidfile", str(image_id_file)]
        require_success(capture([*build, str(source / "environment")],
                                directory, "image-build", timeout=600))
        if frontier:
            image_ref = image_id_file.read_text().strip()
    inspected = require_success(capture(["docker", "image", "inspect", "--format", "{{.Id}}", image_ref],
                                        directory, "image-inspect"))
    image_id = record["benchmark"]["image_id"] = Path(inspected["log"]).read_text().strip()
    if frontier and args.mode == "live":
        from frontier import run_live
        args.agent_timeout_sec = args.agent_timeout_sec or case["timeout_sec"]
        record["attempts"] = [run_live(args, case, source, image_id, directory, secrets)]
    else:
        modes = ("live",) if args.mode == "live" else ("negative",)
        if not frontier and args.mode == "preflight":
            modes += ("oracle",)
        record["attempts"] = [benchmark_attempt(args, case, source, image_id if frontier else image,
                                                 directory, mode, secrets) for mode in modes]
    record["passed"] = all(attempt["passed"] for attempt in record["attempts"])


def selected_commands(group, embedded_web_assets=False):
    features = "kuku/test_support,kuku-app/embedded-web-assets" if embedded_web_assets else "kuku/test_support"
    contract = ["cargo", "test", "--workspace", "--no-fail-fast", "--timings", "--features", features]
    if os.name == "nt":
        contract += ["--", "--test-threads=1"]
    if group == "contract":
        return [contract]
    if group == "protocol":
        command = ["cargo", "test", "-p", "kuku", "--features", "test_support"]
        for suite in PROTOCOL_TESTS:
            command += ["--test", suite]
        return [command]
    return [["cargo", "fmt", "--all", "--", "--check"],
            ["cargo", "clippy", "--workspace", "--all-targets", "--features", "kuku/test_support", "--", "-D", "warnings"],
            contract]


def extract_release_binary(archive, binary):
    with contextlib.ExitStack() as stack:
        if archive.name.endswith(".tar.gz"):
            package = stack.enter_context(tarfile.open(archive, "r:gz"))
            members = [member for member in package.getmembers() if member.name == binary.name and member.isfile()]
            if len(members) != 1:
                raise ValueError(f"Expected one regular {binary.name} at the release archive root")
            source = stack.enter_context(package.extractfile(members[0]))
        elif archive.suffix == ".zip":
            package = stack.enter_context(zipfile.ZipFile(archive))
            members = [entry for entry in package.infolist() if entry.filename == binary.name]
            if len(members) != 1:
                raise ValueError(f"Expected one {binary.name} at the release archive root")
            source = stack.enter_context(package.open(members[0]))
        else:
            raise ValueError("Release archive must be a .tar.gz or .zip")
        with binary.open("xb") as handle:
            shutil.copyfileobj(source, handle)
    binary.chmod(0o755)


def pty_build(log):
    for line in reversed(Path(log).read_text(encoding="utf-8").splitlines()):
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if message.get("reason") == "compiler-artifact" and message.get("executable"):
            target = message["target"]
            if target["name"] == "interactive_pty" and "test" in target["kind"]:
                return message
    raise ValueError("Cargo did not report the compiled native PTY test helper")


def pty_artifact(log):
    return Path(pty_build(log)["executable"])


def verify_desktop_report(report, record):
    if report.stat().st_size > LOG_BYTES:
        raise ValueError("The desktop result exceeds the evidence limit")
    result = json.loads(report.read_text(encoding="utf-8"))
    if result.get("passed") is not True or result.get("binary_sha256") != record["binary"]["sha256"]:
        raise RuntimeError("The desktop evidence did not verify the installed binary")


def run_linux_desktop(binary, bridge, scratch, evidence, record, checked):
    from interactive_fixture import CONFIG

    image_file = scratch / "desktop-image"
    desktop = Path(__file__).with_name("desktop")
    checked(["docker", "build", "--iidfile", str(image_file), str(desktop)], "desktop-image")
    image = image_file.read_text(encoding="utf-8").strip()
    if not re.fullmatch(r"sha256:[a-f0-9]{64}", image):
        raise ValueError("Docker did not report the desktop image ID")
    record["desktop_image"] = image

    cidfile = scratch / "desktop-container"
    command = ["docker", "create", "--cidfile", str(cidfile), "--init", "--network", "none", "--read-only",
               "--user", f"{os.getuid()}:{os.getgid()}", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
               "--pids-limit", "256", "--memory", "1g", "--cpus", "2", "--log-driver", "none",
               "--tmpfs", "/tmp:rw,nosuid,nodev,size=256m,mode=1777"]
    for source, target in ((binary, "/test/kuku"), (bridge, "/test/bridge"),
                           (Path(__file__).with_name("interactive_fixture.py"), "/test/interactive_fixture.py"),
                           (CONFIG, "/test/fixtures/config.toml"), (evidence, "/evidence")):
        options = ["type=bind", f"source={source.resolve()}", f"target={target}"]
        if source != evidence:
            options.append("readonly")
        mount = io.StringIO()
        csv.writer(mount, lineterminator="").writerow(options)
        command.extend(["--mount", mount.getvalue()])
    try:
        checked([*command, image], "desktop-container", timeout=30)
        container = cidfile.read_text(encoding="utf-8").strip()
        checked(["docker", "start", "--attach", container], "installed-desktop", timeout=180)
    finally:
        if cidfile.is_file():
            container = cidfile.read_text(encoding="utf-8").strip()
            if container:
                checked(["docker", "rm", "--force", container], "desktop-cleanup", timeout=30)


def run_desktop(args, binary, bridge, scratch, directory, record, checked):
    evidence = directory / "desktop"
    evidence.mkdir()
    report = evidence / "result.json"
    try:
        if os.name == "nt":
            script = Path(__file__).with_name("desktop") / "windows.py"
            checked([sys.executable, str(script), "--binary", str(binary), "--bridge", str(bridge),
                     "--directory", str(scratch / "desktop"), "--evidence", str(evidence),
                     "--windows-terminal", str(args.windows_terminal.resolve())],
                    "installed-desktop", timeout=180, cwd=scratch)
        else:
            run_linux_desktop(binary, bridge, scratch, evidence, record, checked)
        verify_desktop_report(report, record)
    finally:
        if report.is_file():
            record["desktop_result"] = str(report)


def run_interactive(args, directory, record):
    from interactive_fixture import EXE, scratch_directory

    record["desktop"] = args.desktop
    if args.native_smoke and any((args.desktop, args.browser, args.live, args.intent, args.upgrade_from)):
        raise ValueError("--native-smoke selects only controlled installed terminal cases")
    if args.cases is not None and any((args.native_smoke, args.desktop, args.browser,
                                       args.live, args.intent, args.upgrade_from)):
        raise ValueError("--cases selects its own controlled installed scenarios")
    cases = args.cases.split(",") if args.cases is not None else []
    if cases:
        record["cases"] = cases
    record["scenario"] = "selected" if cases else "native-smoke" if args.native_smoke else "full"
    if args.windows_terminal and (not args.desktop or os.name != "nt"):
        raise ValueError("--windows-terminal requires --desktop on a dedicated Windows test desktop")
    if args.desktop and os.name == "nt":
        from desktop.windows_setup import require_active_desktop

        if args.windows_terminal is None:
            raise ValueError("Select the WindowsTerminal.exe of the dedicated test desktop with --windows-terminal")
        require_active_desktop()
    elif args.desktop and (sys.platform != "linux" or not shutil.which("docker")):
        raise ValueError("Desktop interactive acceptance requires Linux and Docker")
    if args.live and (not args.archive or not args.secrets or not args.ledger):
        raise ValueError("Live interactive acceptance requires --archive, --secrets and --ledger")
    if args.intent and (not args.secrets or not args.ledger or not args.revision or not (args.archive or args.binary) or args.browser):
        raise ValueError("--intent requires --archive or --binary, --secrets, --ledger and --revision, without --browser")
    if not args.intent and any((args.intent_model, args.intent_effort, args.intent_upstream,
                                args.intent_max_requests != 36, args.intent_max_output_tokens != 2048)):
        raise ValueError("Select --intent to set its model, reasoning effort, or upstream endpoint")
    if args.upgrade_from and (not args.archive or args.live or args.intent):
        raise ValueError("--upgrade-from requires a controlled run with --archive")
    scratch = (args.scratch_base or Path(tempfile.gettempdir())).resolve()
    if scratch.is_relative_to(ROOT):
        raise ValueError("Interactive acceptance needs a scratch directory outside the source tree")
    scratch.mkdir(parents=True, exist_ok=True)
    record["browser"] = args.browser or args.live or "browser" in cases
    record["native_pty"] = "conpty" if os.name == "nt" else "unix-pty"
    record["checks"] = []

    def checked(command, label, *, timeout=1800, cwd=ROOT):
        result = capture(command, directory, label, timeout=timeout, env=command_environment(), cwd=cwd)
        record["checks"].append(result)
        return require_success(result)

    def build_bridge():
        compiled = checked(["cargo", "test", "--locked", "--timings", "--workspace", "--features", "kuku/test_support",
                            "--test", "interactive_pty",
                            "--no-run", "--message-format=json"], "native-pty-build")
        artifact = pty_build(compiled["log"])
        record["bridge_build"] = {key: artifact[key] for key in ("features", "profile", "fresh")}
        return Path(artifact["executable"])

    with scratch_directory(scratch, record, prefix="kuku-interactive-") as scratch_dir:
        if args.live or args.intent:
            from interactive_intent import run_selected_live

            bridge = args.intent_bridge.resolve() if args.intent and args.intent_bridge else build_bridge()
            run_selected_live(args, directory, record, scratch_dir, bridge, checked)
            return
        prefix = scratch_dir / "install"
        binary = prefix / "bin" / f"kuku{EXE}"
        if args.archive or args.binary:
            binary.parent.mkdir(parents=True)
            if args.archive:
                record["input"] = {"type": "release_archive", **config_record(args.archive)}
                extract_release_binary(args.archive, binary)
            else:
                record["input"] = {"type": "installed_binary", **config_record(args.binary)}
                shutil.copy2(args.binary, binary)
        else:
            record["input"] = {"type": "source_install", "features": ["embedded-web-assets"],
                               "profile": "release", "target_dir": str(ROOT / "target")}
            web = ROOT / "apps" / "web"
            npm = "npm.cmd" if os.name == "nt" else "npm"
            checked([npm, "ci"], "web-dependencies", cwd=web)
            checked([npm, "run", "build"], "web-assets", cwd=web)
            checked(["cargo", "install", "--locked", "--timings", "--path", "apps/kuku", "--features",
                     "embedded-web-assets", "--root", str(prefix)], "source-install")
        record["binary"] = config_record(binary)

        version = checked([str(binary), "--version"], "installed-version", timeout=30, cwd=scratch_dir)
        record["binary"]["version"] = Path(version["log"]).read_text(encoding="utf-8").strip()
        bridge_cache = args.bridge_cache.resolve() if args.bridge_cache else None
        if bridge_cache and bridge_cache.is_file():
            bridge_source = bridge_cache
            record["bridge_build"] = {"cached": True}
        else:
            bridge_source = build_bridge()
            if bridge_cache:
                bridge_cache.parent.mkdir(parents=True, exist_ok=True)
                with tempfile.NamedTemporaryFile(dir=bridge_cache.parent, delete=False) as staged:
                    staged_cache = Path(staged.name)
                try:
                    shutil.copy2(bridge_source, staged_cache)
                    staged_cache.replace(bridge_cache)
                finally:
                    staged_cache.unlink(missing_ok=True)
        bridge = scratch_dir / f"interactive_pty{EXE}"
        shutil.copy2(bridge_source, bridge)
        record["bridge"] = config_record(bridge)
        command = [sys.executable, str(Path(__file__).with_name("interactive.py")), "--binary", str(binary),
                   "--bridge", str(bridge), "--directory", str(scratch_dir / "test")]
        if args.upgrade_from:
            record["upgrade_from"] = config_record(args.upgrade_from)
            command.extend(["--upgrade", str(args.upgrade_from.resolve()), str(args.archive.resolve())])
        if args.browser:
            command.append("--browser")
        if args.native_smoke:
            command.append("--native-smoke")
        if args.cases is not None:
            command.extend(["--cases", args.cases])
        try:
            checked(command, "installed-interactive", timeout=420, cwd=scratch_dir)
        finally:
            lifecycle = {}
            for case, name in (("settings", "first-run"), ("defaults", "model-default"),
                               ("defaults", "model-default-failure"),
                               ("keyboard-queue", "keyboard-queue-withdrawal"),
                               ("rewind", "readonly-rewind"), ("rewind", "readonly-rewind-failure"),
                               ("rewind-history", "rewind-history"),
                               ("auth-repair", "auth-repair")):
                source = scratch_dir / "test" / case / f"{name}.json"
                if source.is_file():
                    destination = directory / f"{name}.json"
                    shutil.copy2(source, destination)
                    lifecycle[name] = str(destination)
            if lifecycle:
                record["lifecycle"] = lifecycle
            if args.desktop:
                run_desktop(args, binary, bridge, scratch_dir, directory, record, checked)
    record["passed"] = True


def parser():
    root = argparse.ArgumentParser(description=__doc__)
    root.add_argument("--output", type=Path, default=OUTPUT, help="ignored local evidence directory")
    root.add_argument("--run-id", default=None, help="unique evidence/reservation id; never reuse for a paid rerun")
    sub = root.add_subparsers(dest="group", required=True)
    sub.add_parser("contract").add_argument("--embedded-web-assets", action="store_true",
                                            help="compile the app with prepared Web assets for debug acceptance")
    for group in ("protocol", "integration", "list"):
        sub.add_parser(group)
    interactive = sub.add_parser("interactive", help="accept an installed kuku with a local provider and a native PTY")
    product = interactive.add_mutually_exclusive_group()
    product.add_argument("--binary", type=Path, help="copy a previously installed binary to the test prefix")
    product.add_argument("--archive", type=Path, help="extract the platform release .tar.gz or .zip")
    interactive.add_argument("--upgrade-from", type=Path, help="replace a previous package during an active tool")
    interactive.add_argument("--scratch-base", type=Path, help="temporary installation and workspaces outside the checkout")
    interactive.add_argument("--browser", action="store_true", help="also exercise the embedded WebUI with Playwright Chromium")
    interactive.add_argument("--cases", help="run only the comma-separated installed scenarios")
    interactive.add_argument("--native-smoke", action="store_true", help="exercise composer actions, first run, model editing and paste on the native terminal")
    interactive.add_argument("--bridge-cache", type=Path, help="reuse or populate the native PTY helper at this keyed cache path")
    scenario = interactive.add_mutually_exclusive_group()
    scenario.add_argument("--desktop", action="store_true", help="also exercise an isolated Linux or Windows graphical terminal")
    scenario.add_argument("--live", action="store_true", help="run the budgeted release-archive and real-model handoff")
    scenario.add_argument("--intent", choices=("deepseek", "openrouter"), help="run budgeted native reply, edit, and first-delegation checks")
    interactive.add_argument("--intent-case", choices=("all", "delivery", "delegation", "release-review"), default="all")
    interactive.add_argument("--intent-bridge", type=Path, help="reuse a built native PTY helper with --intent")
    interactive.add_argument("--intent-model", help="selected provider model for --intent")
    interactive.add_argument("--intent-effort", help="explicit native reasoning effort for --intent (default: low)")
    interactive.add_argument("--intent-upstream", help="exact OpenRouter endpoint tag for --intent")
    interactive.add_argument("--intent-max-requests", type=int, default=36)
    interactive.add_argument("--intent-max-output-tokens", type=int, default=2048)
    interactive.add_argument("--revision", help="expected installed source revision for --intent")
    interactive.add_argument("--windows-terminal", type=Path, help="existing WindowsTerminal.exe in a dedicated test desktop")
    interactive.add_argument("--secrets", type=Path, help="explicit credentials file for budgeted interactive checks")
    interactive.add_argument("--ledger", type=Path, help="existing budget ledger for the selected API service")
    workspace = sub.add_parser("workspace", help="verify native log analysis and named report recovery through the SDK or installed TUI")
    workspace_product = workspace.add_mutually_exclusive_group(required=True)
    workspace_product.add_argument("--sdk", type=Path, help="built kuku session_task example")
    workspace_product.add_argument("--binary", type=Path, help="install a copy of the selected kuku binary")
    workspace_product.add_argument("--archive", type=Path, help="install the selected platform release .tar.gz or .zip")
    workspace.add_argument("--bridge", type=Path, help="reuse a built native PTY and log fixture")
    workspace.add_argument("--scratch-base", type=Path, help="temporary workspaces outside the source tree")
    workspace.add_argument("--handoff-only", action="store_true", help="with --sdk, verify the named report handoff to a reviewer")
    workspace.add_argument("--provider", choices=("deepseek", "openrouter"), help="explicitly select a paid service; default is a fixed local provider")
    workspace.add_argument("--model", help="explicit provider model slug")
    workspace.add_argument("--effort", help="native reasoning effort for a paid provider (default: high)")
    workspace.add_argument("--upstream", help="exact upstream endpoint tag (OpenRouter default: openai)")
    workspace.add_argument("--max-requests", type=int, default=12, help="total bounded upstream requests including retries")
    workspace.add_argument("--max-output-tokens", type=int, default=8192)
    workspace.add_argument("--secrets", type=Path, help="explicit KUKU_TEST_ credentials file for paid runs")
    workspace.add_argument("--ledger", type=Path, help="existing budget ledger for the selected API service")
    metrics = sub.add_parser("metrics", help="build an isolated public SDK caller and measure local fixed-provider workloads")
    metrics.add_argument("--profile", choices=("debug", "release"), default="debug")
    metrics.add_argument("--idle-agents", type=int, default=32)
    metrics.add_argument("--appends", type=int, default=64)
    metrics.add_argument("--concurrent-agents", type=int, default=2)
    metrics.add_argument("--output-bytes", type=int, default=4 * 1024 * 1024)
    metrics.add_argument("--output-quota", type=int, default=1024 * 1024)
    metrics.add_argument("--repeated-tasks", type=int, default=6)
    metrics.add_argument("--script-workers", type=int, default=2)
    metrics.add_argument("--script-host-operations", type=int, default=4)
    metrics.add_argument("--pressure-rounds", type=int, default=1, help="repeat shared-host stress and idle recovery in the same caller")
    metrics.add_argument("--long-session-mib", type=int, help="select a historical Session journal of 4–512 MiB instead of the small workload")
    metrics.add_argument("--cpu-count", type=int, help="limit the caller and its descendants to this many available CPUs on Linux")
    metrics.add_argument("--address-space-mib", type=int, help="apply a per-process Linux address-space bound to the caller and descendants")
    inspect = sub.add_parser("inspect", help="check existing durable SDK evidence without model requests")
    inspect.add_argument("--sdk-result", type=Path, required=True)
    inspect.add_argument("--session-data", type=Path, required=True)
    inspect.add_argument("--container-data", help="data mount prefix in a container-produced SDK report")
    inspect.add_argument("--connection")
    inspect.add_argument("--require-streaming", action="store_true")
    budget = sub.add_parser("budget")
    budget.add_argument("action", choices=("init", "show", "settle"))
    budget.add_argument("--ledger", type=Path, default=OUTPUT / "budget.json")
    budget.add_argument("--authorization")
    budget.add_argument("--limit-usd", type=money)
    budget.add_argument("--opening-spent-usd", type=money, default=0)
    budget.add_argument("--entry")
    budget.add_argument("--cost-usd", type=money)
    budget.add_argument("--basis", choices=("provider_reported", "token_estimate", "conservative_upper_bound"),
                        default="conservative_upper_bound")
    budget.add_argument("--evidence")
    for group in ("live", "benchmark"):
        paid = sub.add_parser(group)
        paid.add_argument("--secrets", type=Path, default=Path("/code/.secrets/kuku-acceptance.env"))
        paid.add_argument("--ledger", type=Path, default=OUTPUT / "budget.json")
        paid.add_argument("--reserve-usd", type=money)
        paid.add_argument("--reservation-reason", help="request/token/retry bounds and conservative rates")
        paid.add_argument("--require-streaming", action="store_true", help="require nonempty unlagged SDK model streaming observations")
        if group == "live":
            paid.add_argument("--config", type=Path, required=True, help="hash the caller's config without copying possible inline credentials")
            paid.add_argument("--sdk-result", type=Path, required=True)
            paid.add_argument("--session-data", type=Path, required=True)
            paid.add_argument("--connection")
            paid.add_argument("command", nargs=argparse.REMAINDER, help="public SDK/CLI caller after --")
        else:
            from benchmark_source import add_benchmark_arguments
            add_benchmark_arguments(paid)
    return root


def main():
    args = parser().parse_args()
    if args.group == "list":
        print(MANIFEST.read_text(encoding="utf-8"))
        return 0
    if args.group == "budget":
        budget = Budget(args.ledger)
        if args.action == "init":
            if not args.authorization or args.limit_usd is None:
                raise ValueError("budget init requires an authorization reference and its total USD limit")
            budget.initialize(args.authorization, args.limit_usd, args.opening_spent_usd)
        elif args.action == "settle":
            budget.settle(args.entry, args.cost_usd, args.basis, args.evidence)
        data = json.loads(args.ledger.read_text(encoding="utf-8"))
        print(json.dumps({"ledger": str(args.ledger), "authorization": data["authorization"],
                          "limit_usd": data["limit_usd"], "committed_usd": budget.committed(data),
                          "remaining_usd": round(data["limit_usd"] - budget.committed(data), 9)}, indent=2))
        return 0
    paid = args.group == "live" or (args.group == "benchmark" and args.mode == "live")
    metered = paid and args.group == "benchmark" and args.suite == "frontier"
    if args.group == "benchmark":
        from benchmark_source import validate_benchmark_arguments
        validate_benchmark_arguments(args, metered, paid)
    secrets = {}
    if paid:
        if metered:
            if sum(bool(runner) for runner in (args.sdk_binary, args.pi_prefix, args.codex_prefix)) != 1 or args.config or args.connection or args.sdk_args:
                raise ValueError("Frontier live requires one runner (--sdk-binary, --pi-prefix or --codex-prefix) and generates its config")
            if args.reserve_usd is not None or args.reservation_reason:
                raise ValueError("Frontier live uses --case-cap-usd for its host-metered budget")
            limits = (args.case_cap_usd, args.max_output_tokens, args.max_requests, args.agent_timeout_sec)
            if any(limit is not None and limit <= 0 for limit in limits):
                raise ValueError("Frontier live limits must be positive")
        elif args.reserve_usd is None or not args.reservation_reason:
            raise ValueError("paid calls need --reserve-usd and --reservation-reason before execution")
        secrets = load_secrets(args.secrets)
        if not secrets:
            raise ValueError("credential file contains no nonempty KUKU_TEST_ assignments")
    run_id = args.run_id or datetime.datetime.now().strftime("%Y%m%d-%H%M%S-") + uuid.uuid4().hex[:8]
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,100}", run_id):
        raise ValueError("run id must be a short filename component")
    args.run_id = run_id
    directory = args.output.resolve() / run_id
    directory.mkdir(parents=True, exist_ok=False)
    record = {"schema_version": 1, "run_id": run_id, "group": args.group, "started_at": now(),
              "tree": tree_state(), "paid": paid or (args.group == "interactive" and bool(args.live or args.intent))
              or (args.group == "workspace" and bool(args.provider)), "passed": False,
              "selector_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "manifest_sha256": hashlib.sha256(MANIFEST.read_bytes()).hexdigest()}
    reserved = False
    try:
        if paid and not metered:
            if args.group == "benchmark":
                if not args.sdk_binary or not args.config or not args.connection:
                    raise ValueError("live benchmark requires --sdk-binary, --config and --connection")
                args.sdk_args = args.sdk_args[1:] if args.sdk_args[:1] == ["--"] else args.sdk_args
                if "--max-requests" not in args.sdk_args:
                    args.sdk_args.extend(["--max-requests", "12"])
                if "--max-output-tokens" not in args.sdk_args:
                    args.sdk_args.extend(["--max-output-tokens", "32768"])
                if "--no-provider-retry" not in args.sdk_args:
                    args.sdk_args.append("--no-provider-retry")
            record["config"] = config_record(args.config)
            Budget(args.ledger).reserve(run_id, args.reserve_usd, args.reservation_reason)
            reserved = True
            record["budget"] = {"ledger": str(args.ledger), "reserved_usd": args.reserve_usd,
                                "basis": "conservative_upper_bound"}
        if args.group == "benchmark":
            run_benchmark(args, directory, record, secrets)
        elif args.group == "metrics":
            from metrics import run_metrics

            run_metrics(args, directory, record)
        elif args.group == "interactive":
            run_interactive(args, directory, record)
        elif args.group == "workspace":
            from workspace import run_workspace

            run_workspace(args, directory, record)
        elif args.group == "inspect":
            record["sdk_evidence"] = inspect_sdk(args.sdk_result.resolve(), args.session_data.resolve(),
                                                  args.connection, args.container_data, require_streaming=args.require_streaming)
            record["passed"] = record["sdk_evidence"]["passed"]
        else:
            commands = (selected_commands(args.group, getattr(args, "embedded_web_assets", False))
                        if not paid else [args.command[1:] if args.command[:1] == ["--"] else args.command])
            if not all(commands):
                raise ValueError("live requires a public SDK/CLI command after --")
            env = command_environment(secrets)
            record["checks"] = []
            for index, command in enumerate(commands):
                check = capture(command, directory, f"check-{index + 1}", timeout=1800,
                                env=env, secrets=secrets.values(),
                                retain_failures=not paid and "--no-fail-fast" in command)
                record["checks"].append(check)
                if check["exit_code"] != 0 or check["timed_out"] or check.get("capture_incomplete"):
                    break
            record["passed"] = len(record["checks"]) == len(commands) and all(
                check["exit_code"] == 0 and not check["timed_out"] and not check.get("capture_incomplete")
                for check in record["checks"])
            if paid:
                record["sdk_evidence"] = inspect_sdk(args.sdk_result.resolve(), args.session_data.resolve(), args.connection,
                                                      require_streaming=args.require_streaming, require_fixed_limits=True)
                record["passed"] = record["passed"] and record["sdk_evidence"]["passed"]
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError, tarfile.TarError, zipfile.BadZipFile) as error:
        record.update(passed=False, error=redact(str(error), secrets.values()))
        record.setdefault("failure_kind", "infrastructure")
    finally:
        if reserved:
            try:
                Budget(args.ledger).settle(run_id)
            except (OSError, ValueError) as error:
                record["budget"]["settlement_error"] = redact(str(error), secrets.values())
                record["passed"] = False
        record["finished_at"] = now()
        write_json(directory / "result.json", record)
    print(json.dumps({"run_id": run_id, "passed": record["passed"],
                      "evidence": str(directory / "result.json"), "error": record.get("error")}, indent=2))
    return 0 if record["passed"] else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError) as error:
        print(f"acceptance: {redact(str(error))}", file=sys.stderr)
        sys.exit(2)
