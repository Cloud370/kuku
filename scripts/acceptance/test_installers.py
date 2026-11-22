import gzip
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import io
import json
import os
from pathlib import Path
import platform
import subprocess
import tarfile
import tempfile
import threading
import unittest
import zipfile


SCRIPTS = Path(__file__).resolve().parents[1]
PAYLOAD = b"kuku installer fixture\n"
TARGETS = {
    ("Linux", "x86_64"): "linux-x86_64",
    ("Darwin", "arm64"): "darwin-aarch64",
    ("Windows", "AMD64"): "windows-x86_64",
}


def package(binary):
    output = io.BytesIO()
    if binary == "kuku.exe":
        with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            archive.writestr(binary, PAYLOAD)
    else:
        with tarfile.open(fileobj=output, mode="w:") as archive:
            entry = tarfile.TarInfo(binary)
            entry.size = len(PAYLOAD)
            archive.addfile(entry, io.BytesIO(PAYLOAD))
        return gzip.compress(output.getvalue(), mtime=0)
    return output.getvalue()


class InstallerTests(unittest.TestCase):
    target = TARGETS.get((platform.system(), platform.machine()))

    def setUp(self):
        if not self.target:
            self.skipTest("platform has no kuku release")
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.home = Path(temporary.name)
        self.binary = "kuku.exe" if self.target.startswith("windows") else "kuku"
        self.installed = self.home / "bin" / self.binary
        extension = "zip" if self.binary.endswith(".exe") else "tar.gz"
        self.archive = self.home / "cache" / f"kuku-{self.target}.{extension}"
        self.data = package(self.binary)
        self.entry = {"sha256": hashlib.sha256(self.data).hexdigest(), "size": len(self.data)}
        self.requests = 0
        self.next_status = 200
        fixture = self

        class Release(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                if self.path == "/latest.json":
                    address = f"http://127.0.0.1:{self.server.server_port}"
                    entry = {"url": f"{address}/package", **fixture.entry}
                    body = json.dumps({"version": "fixture", "platforms": {fixture.target: entry}},
                                      indent=2).encode()
                    self.respond(200, body, "application/json")
                elif self.path == "/package":
                    fixture.requests += 1
                    status, fixture.next_status = fixture.next_status, 200
                    self.respond(status, fixture.data if status == 200 else b"")
                else:
                    self.respond(404, b"")

            def respond(self, status, body, media_type="application/octet-stream"):
                self.send_response(status)
                self.send_header("Content-Type", media_type)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        server = ThreadingHTTPServer(("127.0.0.1", 0), Release)
        self.addCleanup(server.server_close)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(thread.join)
        self.addCleanup(server.shutdown)
        self.source = f"http://127.0.0.1:{server.server_port}/latest.json"

    def run_installer(self):
        env = dict(os.environ, KUKU_HOME=str(self.home))
        if self.binary.endswith(".exe"):
            command = ["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy",
                       "Bypass", "-File", str(SCRIPTS / "install.ps1"), "-Force", "-NoModifyPath",
                       "-Source", self.source]
        else:
            env["PATH"] = str(self.home / "bin") + os.pathsep + env["PATH"]
            command = ["sh", str(SCRIPTS / "install.sh"), "--force", "--source", self.source]
        return subprocess.run(command, env=env, capture_output=True, text=True, timeout=24)

    def installed_successfully(self, result):
        self.assertEqual(0, result.returncode, result.stdout + result.stderr)
        self.assertEqual(PAYLOAD, self.installed.read_bytes())
        self.assertEqual(self.data, self.archive.read_bytes())
        self.assertEqual([], list(self.installed.parent.glob(".install*")))

    def test_retries_the_download_and_reuses_only_verified_cache(self):
        self.next_status = 503
        first = self.run_installer()
        self.installed_successfully(first)
        self.assertIn(f"({len(self.data)} bytes)", first.stdout)
        self.assertEqual(2, self.requests)
        second = self.run_installer()
        self.installed_successfully(second)
        self.assertIn("Using verified cached", second.stdout)
        self.assertEqual(2, self.requests)
        self.archive.write_bytes(b"bad cache")
        self.installed_successfully(self.run_installer())
        self.assertEqual(3, self.requests)

    def test_bad_checksum_keeps_existing_executable_and_no_cache(self):
        self.installed.parent.mkdir()
        self.installed.write_bytes(b"old executable")
        self.entry["sha256"] = "0" * 64
        self.entry.pop("size")
        failed = self.run_installer()
        self.assertNotEqual(0, failed.returncode)
        self.assertIn("Checksum mismatch", failed.stdout + failed.stderr)
        self.assertEqual(b"old executable", self.installed.read_bytes())
        self.assertFalse(self.archive.exists())
        self.assertEqual([], list(self.installed.parent.glob(".install*")))

    @unittest.skipIf(os.name == "nt", "Unix installer")
    def test_path_hint_activates_kuku_in_the_calling_bash(self):
        shell_home = self.home / "home with ' and spaces"
        shell_home.mkdir()
        profile = shell_home / ".bashrc"
        profile.touch()
        env = dict(os.environ, HOME=str(shell_home), KUKU_HOME="", SHELL="/bin/bash")
        result = subprocess.run(
            ["bash", "--noprofile", "--norc", "-ec", """
                log=$(sh "$1" --source "$2")
                printf '%s\\n' "$log"
                installed="$HOME/.kuku/bin/kuku"
                test "$(command -v kuku || true)" != "$installed"
                activation=$(printf '%s\\n' "$log" | sed -n '/^To use kuku in this terminal, run:$/ {n; s/^  //; p;}')
                test -n "$activation"
                eval "$activation"
                test "$(command -v kuku)" = "$installed"
            """, "installer-test", str(SCRIPTS / "install.sh"), self.source],
            env=env, capture_output=True, text=True, timeout=24,
        )
        self.assertEqual(0, result.returncode, result.stdout + result.stderr)
        self.assertEqual(PAYLOAD, (shell_home / ".kuku" / "bin" / "kuku").read_bytes())
        command = result.stdout.split("To use kuku in this terminal, run:\n  ", 1)[1].splitlines()[0]
        self.assertIn(command + "\n", profile.read_text())
