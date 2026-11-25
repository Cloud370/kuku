"""Inspect Windows desktop support and prepare a disposable hosted runner."""

import argparse
import ctypes
from ctypes import wintypes
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
from urllib.request import urlopen
from zipfile import ZipFile


TERMINAL_VERSION = "1.25.3131.0"
TERMINAL_SHA256 = "273d6f1502eca1b5bd95c43f5d4d7ec72cc503df340102f8659717e5bead3437"
TERMINAL_SIZE = 12126938
PINYIN_SERVICE = "{81D4E9C9-1D3B-41BC-9E6C-4B40BF79E35E}"
PINYIN_PROFILE = "{FA550B04-5AD7-411F-A5AC-CA038EC515D7}"
PINYIN_TIP = f"0804:{PINYIN_SERVICE}{PINYIN_PROFILE}"


def powershell(script, timeout=15):
    command = "$ErrorActionPreference = 'Stop'; [Console]::OutputEncoding = [Text.UTF8Encoding]::new(); "
    try:
        result = subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", command + script],
                                capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=timeout,
                                creationflags=subprocess.CREATE_NO_WINDOW)
    except subprocess.TimeoutExpired as error:
        raise RuntimeError(f"PowerShell did not complete within {timeout} seconds") from error
    if result.returncode:
        raise RuntimeError(f"PowerShell returned {result.returncode}\n{result.stderr.strip()[-2000:]}")
    return result.stdout.strip()


def desktop_state():
    user = ctypes.WinDLL("user32", use_last_error=True)
    user.GetProcessWindowStation.restype = wintypes.HWINSTA
    user.GetThreadDesktop.argtypes = (wintypes.DWORD,)
    user.GetThreadDesktop.restype = wintypes.HDESK
    user.GetUserObjectInformationW.argtypes = (wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p,
                                                wintypes.DWORD, ctypes.POINTER(wintypes.DWORD))
    user.OpenInputDesktop.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
    user.OpenInputDesktop.restype = wintypes.HDESK
    user.CloseDesktop.argtypes = (wintypes.HDESK,)
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)

    def name(handle):
        buffer = ctypes.create_unicode_buffer(256)
        if handle and user.GetUserObjectInformationW(handle, 2, buffer, ctypes.sizeof(buffer), None):
            return buffer.value

    current = name(user.GetThreadDesktop(kernel.GetCurrentThreadId()))
    station = name(user.GetProcessWindowStation())
    active = user.OpenInputDesktop(0, False, 0x0001)
    try:
        input_desktop = name(active)
    finally:
        if active:
            user.CloseDesktop(active)
    return {"station": station, "thread_desktop": current, "input_desktop": input_desktop,
            "interactive": station == "WinSta0" and current == input_desktop == "Default"}


def require_active_desktop():
    if os.name != "nt" or not desktop_state()["interactive"]:
        raise RuntimeError("The test requires an active Windows input desktop")


def require_hosted_desktop():
    if os.name != "nt" or os.environ.get("GITHUB_ACTIONS") != "true" or (
        os.environ.get("RUNNER_ENVIRONMENT"), os.environ.get("RUNNER_OS")
    ) != ("github-hosted", "Windows"):
        raise ValueError("Windows Terminal GUI automation requires a disposable GitHub-hosted Windows runner")
    require_active_desktop()


def pinyin_registered():
    import winreg

    registry_path = rf"SOFTWARE\Microsoft\CTF\TIP\{PINYIN_SERVICE}\LanguageProfile\0x00000804\{PINYIN_PROFILE}"
    try:
        with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, registry_path):
            return True
    except FileNotFoundError:
        return False


def pinyin_configured():
    result = powershell(
        f"[bool]((Get-WinUserLanguageList).InputMethodTips -contains '{PINYIN_TIP}')",
        timeout=45,
    )
    return result == "True"


def prepare_pinyin():
    existing = pinyin_registered()
    if not existing:
        powershell("""
$capability = 'Language.Basic~~~zh-CN~0.0.1.0'
if ((Get-WindowsCapability -Online -Name $capability).State -ne 'Installed') {
    $added = Add-WindowsCapability -Online -Name $capability
    if ($added.RestartNeeded) { throw 'Microsoft Pinyin Basic Typing requires a restart' }
}
""", timeout=180)
    powershell(f"""
$languages = Get-WinUserLanguageList
if ($languages.InputMethodTips -notcontains '{PINYIN_TIP}') {{
    $chinese = @($languages) | Where-Object LanguageTag -Match '^zh-(Hans-)?CN$' | Select-Object -First 1
    if (-not $chinese) {{
        $chinese = (New-WinUserLanguageList 'zh-CN')[0]
        $languages.Add($chinese)
    }}
    if ($chinese.InputMethodTips -notcontains '{PINYIN_TIP}') {{ $chinese.InputMethodTips.Add('{PINYIN_TIP}') }}
    Set-WinUserLanguageList $languages -Force -WarningAction SilentlyContinue
}}
""")
    if not pinyin_registered() or not pinyin_configured():
        raise RuntimeError("Microsoft Pinyin is unavailable after configuring the test account")
    return "existing-profile" if existing else "basic-typing-capability"


def prepare_terminal(directory):
    filename = f"Microsoft.WindowsTerminal_{TERMINAL_VERSION}_x64.zip"
    url = f"https://github.com/microsoft/terminal/releases/download/v{TERMINAL_VERSION}/{filename}"
    with urlopen(url, timeout=45) as response:
        data = response.read(TERMINAL_SIZE + 1)
    if len(data) != TERMINAL_SIZE or hashlib.sha256(data).hexdigest() != TERMINAL_SHA256:
        raise RuntimeError("The Windows Terminal archive does not match the pinned release")
    directory.mkdir(parents=True)
    with ZipFile(io.BytesIO(data)) as archive:
        archive.extractall(directory)
    executable, = directory.rglob("WindowsTerminal.exe")
    executable.with_name(".portable").touch(exist_ok=False)
    return executable


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--prepare", type=Path, metavar="TERMINAL_DIRECTORY")
    args = parser.parse_args()
    result = {"ready": False}
    stage = "desktop"
    try:
        if os.name != "nt":
            raise ValueError("Windows desktop inspection requires Windows Python")
        result.update(windows_build=sys.getwindowsversion().build, desktop=desktop_state())
        stage = "pinyin"
        result.update(pinyin_registered=pinyin_registered(), pinyin_configured=pinyin_configured())
        if args.prepare is not None:
            stage = "hosted-desktop"
            require_hosted_desktop()
            stage = "terminal"
            result["terminal"] = str(prepare_terminal(args.prepare.resolve()))
            result["terminal_version"] = TERMINAL_VERSION
            result["terminal_archive_sha256"] = TERMINAL_SHA256
            result["ready"] = True
            try:
                result["pinyin_source"] = prepare_pinyin()
                result.update(pinyin_registered=pinyin_registered(), pinyin_configured=pinyin_configured())
            except Exception as error:
                result["pinyin_setup_error"] = str(error)[:2048]
    except Exception as error:
        result["failure"] = {"stage": stage, "message": str(error)[:2048]}
        print(f"Windows desktop {stage}: {error}", file=sys.stderr)
        return 1
    finally:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
