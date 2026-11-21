"""Select and prepare isolated upstream Terminal-Bench tasks."""

import argparse
from pathlib import Path
import subprocess
import tempfile

from run import capture, locked, money, require_success


def validate_benchmark_arguments(args, metered, paid):
    if args.cpus <= 0:
        raise ValueError("--cpus must be positive")
    if args.model is not None and (not metered or args.provider != "deepseek"):
        raise ValueError("--model selects a direct DeepSeek model for Frontier live")
    if args.pi_prefix and (not metered or args.provider != "deepseek" or args.require_streaming):
        raise ValueError("--pi-prefix selects direct DeepSeek Frontier live with Pi Session evidence")
    if args.node_binary and not args.pi_prefix:
        raise ValueError("--node-binary requires --pi-prefix")
    if args.prompts_dir and (not metered or not args.sdk_binary or args.pi_prefix):
        raise ValueError("--prompts-dir requires a live Frontier SDK runner")
    if args.max_command_seconds is not None:
        if not metered or not args.sdk_binary or args.pi_prefix:
            raise ValueError("--max-command-seconds requires a live Frontier SDK runner")
        if not 0 < args.max_command_seconds <= 2**32 - 1:
            raise ValueError("--max-command-seconds must be a positive 32-bit integer")
    if args.require_resource_evidence and not paid:
        raise ValueError("--require-resource-evidence applies to live benchmark measurements")


def add_benchmark_arguments(parser):
    parser.add_argument("--suite", choices=("legacy", "frontier"), default="legacy")
    parser.add_argument("--case", required=True)
    parser.add_argument("--mode", choices=("preflight", "live"), default="preflight")
    parser.add_argument("--cpus", type=int, default=1, help="container CPU quota (default: 1)")
    parser.add_argument("--sdk-binary", type=Path)
    parser.add_argument("--prompts-dir", type=Path, help="snapshot overrides for a live Frontier SDK runner")
    parser.add_argument("--pi-prefix", type=Path, help="compare with an isolated Pi npm install on direct DeepSeek")
    parser.add_argument("--node-binary", type=Path, help="Linux Node.js executable for --pi-prefix")
    parser.add_argument("--require-resource-evidence", action="store_true",
                        help="require kernel cgroup CPU, memory peak, OOM and harness RSS evidence")
    parser.add_argument("--config", type=Path)
    parser.add_argument("--connection")
    parser.add_argument("--provider", choices=("openrouter", "deepseek"), default="openrouter",
                        help="Frontier live API service (default: openrouter)")
    parser.add_argument("--model", help="priced model for direct DeepSeek Frontier live (default: deepseek-flash)")
    parser.add_argument("--upstream", help="exact OpenRouter endpoint tag for Frontier live")
    parser.add_argument("--case-cap-usd", type=money, default=0.75)
    parser.add_argument("--max-output-tokens", type=int, help="Frontier live output ceiling (default: the selected route's limit)")
    parser.add_argument("--max-requests", type=int, help="optional total upstream attempt limit for Frontier live")
    parser.add_argument("--agent-timeout-sec", type=int)
    parser.add_argument("--max-command-seconds", type=int, help="optional per-command deadline for Frontier SDK live")
    parser.add_argument("sdk_args", nargs=argparse.REMAINDER, help="additional bounded session_task args after --")


def prepare_source(root, manifest, directory, case=None):
    sources = root / "benchmarks" / "sources"
    spec = manifest["source"]
    if "cache_key" in spec:
        return prepare_pinned(sources, spec, directory, case)

    source = sources / "terminal-bench-2"
    revision = spec["revision"]
    with locked(sources / "terminal-bench-2.lock"):
        existing = (source / ".git").exists()
        if not existing:
            require_success(capture(["git", "clone", "--filter=blob:none", "--no-checkout",
                                     spec["repository"], str(source)], directory, "source-clone", timeout=600))
        if existing:
            require_clean(source)
        present = subprocess.run(["git", "-C", str(source), "cat-file", "-e", revision],
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if present.returncode:
            require_success(capture(["git", "-C", str(source), "fetch", "--depth", "1", "origin", revision],
                                    directory, "source-fetch", timeout=600))
        require_success(capture(["git", "-C", str(source), "sparse-checkout", "init", "--cone"],
                                directory, "source-sparse-init"))
        require_success(capture(["git", "-C", str(source), "sparse-checkout", "set",
                                 *[case["id"] for case in manifest["cases"]]],
                                directory, "source-sparse-set", timeout=600))
        require_success(capture(["git", "-C", str(source), "checkout", "--detach", revision],
                                directory, "source-checkout", timeout=600))
        require_clean(source)
    return source


def require_clean(source):
    if subprocess.check_output(["git", "--no-optional-locks", "-C", str(source), "status", "--porcelain"]):
        raise ValueError("benchmark source is modified; preserve and inspect it before continuing")


def check_pinned(checkout, revision, task, patterns):
    def git(*args):
        return subprocess.check_output(["git", "--no-optional-locks", "-C", str(checkout), *args])

    head = git("rev-parse", "HEAD").decode().strip()
    required = ("instruction.md", "task.toml", "environment/Dockerfile", "tests/test.sh")
    if head != revision or any(not (checkout / task / name).is_file() for name in required):
        raise ValueError(f"benchmark checkout does not match the pinned task: {checkout}")
    if (git("sparse-checkout", "list").decode().splitlines() != list(patterns)
        or (checkout / task / "solution").exists()
        or git("status", "--porcelain", "--untracked-files=no")
        or git("ls-files", "--others")):
        raise ValueError(f"benchmark source is modified; preserve and inspect it before continuing: {checkout}")


def prepare_pinned(sources, spec, directory, case):
    revision = spec["revision"]
    cache = sources / spec["cache_key"] / revision
    checkout = cache / case["id"]
    task = Path(spec["task_root"]) / case["id"]
    patterns = tuple(f"/{task.as_posix()}/{name}" for name in (
        ".gitignore", "README.md", "instruction.md", "task.toml", "environment/", "tests/"))
    if checkout.exists():
        check_pinned(checkout, revision, task, patterns)
        return checkout
    with locked(checkout.with_suffix(".lock"), wait_seconds=3600):
        if not checkout.exists():
            with tempfile.TemporaryDirectory(prefix=f".{case['id']}-", dir=cache) as temporary:
                staging = Path(temporary) / "source"
                commands = (
                    ("init", "--quiet", str(staging)),
                    ("remote", "add", "origin", spec["repository"]),
                    ("config", "remote.origin.promisor", "true"),
                    ("fetch", "--no-tags", "--depth=1", "--filter=blob:none", "origin", revision),
                    ("sparse-checkout", "set", "--no-cone", *patterns),
                    ("checkout", "--detach", revision),
                )
                for command in commands:
                    path = cache if command[0] == "init" else staging
                    attempts = 3 if command[0] in {"fetch", "checkout"} else 1
                    for attempt in range(attempts):
                        label = f"source-{command[0]}" + (f"-retry-{attempt}" if attempt else "")
                        result = capture(["git", "-C", str(path), *command], directory, label, timeout=600)
                        if result["exit_code"] == 0 or result["timed_out"]:
                            break
                    require_success(result)
                check_pinned(staging, revision, task, patterns)
                staging.rename(checkout)
        else:
            check_pinned(checkout, revision, task, patterns)
    return checkout
