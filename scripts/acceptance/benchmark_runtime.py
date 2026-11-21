"""Bundle isolated benchmark runtimes and interpret original verifier evidence."""

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import ssl
import subprocess


def file_hashes(directory):
    return {str(path.relative_to(directory)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(directory.rglob("*")) if path.is_file() and ".git" not in path.parts}


def bundle_binary(binary, directory, filename="session_task"):
    destination = directory / "runtime"
    destination.mkdir()
    shutil.copy2(binary, destination / filename)
    result = subprocess.run(["ldd", str(binary)], capture_output=True, text=True)
    libraries = re.findall(r"(?:=>\s+)?(/[^\s()]+)", result.stdout)
    for path in libraries:
        shutil.copy2(path, destination / Path(path).name)
    loader = next((Path(path).name for path in libraries if "ld-linux" in path), None)
    if not libraries and "statically linked" not in result.stdout + result.stderr:
        raise ValueError("cannot resolve runtime libraries; provide a Linux executable")
    command = [f"/kuku/runtime/{filename}"]
    if loader:
        command = [f"/kuku/runtime/{loader}", "--library-path", "/kuku/runtime", *command]
    return command, file_hashes(destination)


def bundle_prompts(source, destination):
    if not source.is_dir():
        raise ValueError("expected a Prompt override directory")
    assets = [source / "system.md"]
    for name in ("agents", "blocks", "memory", "runtime", "tools"):
        directory = source / name
        if directory.is_symlink():
            raise ValueError("Prompt override subdirectories must be local directories")
        assets.extend(directory.glob("*.md"))
    assets = [asset for asset in assets if asset.exists() or asset.is_symlink()]
    if not 0 < len(assets) <= 256:
        raise ValueError("expected between 1 and 256 Prompt override assets")
    for asset in assets:
        if asset.is_symlink() or not asset.is_file():
            raise ValueError("Prompt override assets must be regular files")
        with asset.open("rb") as handle:
            content = handle.read(128 * 1024 + 1)
        if len(content) > 128 * 1024:
            raise ValueError("Prompt override asset exceeds 128 KiB")
        content.decode("utf-8")
        target = destination / asset.relative_to(source)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content)
    return file_hashes(destination)


def bundle_ca(directory, source=None):
    if source is None:
        explicit = os.environ.get("SSL_CERT_FILE")
        candidates = [explicit] if explicit else ["/etc/ssl/certs/ca-certificates.crt",
                                                "/etc/pki/tls/certs/ca-bundle.crt",
                                                ssl.get_default_verify_paths().cafile]
        source = next((Path(name) for name in candidates if name and Path(name).is_file()), None)
    if source is None or not source.is_file() or source.stat().st_size == 0:
        raise ValueError("a nonempty host CA bundle is required for the Linux benchmark runtime")
    target = directory / "ca-certificates.crt"
    shutil.copyfile(source, target)
    target.chmod(0o444)
    return {"source": str(source.resolve()), "sha256": hashlib.sha256(target.read_bytes()).hexdigest(),
            "runtime_path": "/kuku/runtime/ca-certificates.crt",
            "environment": {"SSL_CERT_FILE": "/kuku/runtime/ca-certificates.crt"}}


def reward(logs):
    path = logs / "verifier" / "reward.txt"
    if not path.exists():
        return None
    try:
        return float(path.read_text(encoding="utf-8").strip())
    except ValueError:
        return None


def verifier_summary(logs):
    path = logs / "verifier" / "ctrf.json"
    if not path.exists() or path.stat().st_size > 1024 * 1024:
        return None
    try:
        report = json.loads(path.read_text(encoding="utf-8"))
        summary = report["results"]["summary"]
        return summary if isinstance(summary, dict) else None
    except (ValueError, KeyError, TypeError):
        return None


def fresh_verifier_directory(logs):
    path = logs / "verifier"
    if path.is_symlink() or (path.exists() and not path.is_dir()):
        path.unlink()
    elif path.exists():
        shutil.rmtree(path)
    path.mkdir()


def attempt_passed(mode, execution, verification, value, summary):
    if execution["exit_code"] != 0 or execution.get("timed_out") or execution.get("capture_incomplete"):
        return False
    return verifier_passed(mode, verification, value, summary)


def verifier_passed(mode, verification, value, summary):
    if verification["exit_code"] != 0 or verification.get("timed_out") or verification.get("capture_incomplete"):
        return False
    if not summary or summary.get("tests", 0) <= 0:
        return False
    if mode == "negative":
        return value == 0.0 and summary.get("failed", 0) > 0
    return value == 1.0 and summary.get("tests") == summary.get("passed") and all(
        summary.get(field) == 0 for field in ("failed", "skipped", "pending", "other"))
