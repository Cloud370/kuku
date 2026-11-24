"""Select contract jobs, installed cases, and Web assets for GitHub CI."""

import argparse
from fnmatch import fnmatchcase
import json
from pathlib import Path
import subprocess


PLATFORMS = {
    "linux": ("ubuntu-latest", "Linux PTY"),
    "macos": ("macos-latest", "macOS PTY"),
    "windows": ("windows-latest", "Windows ConPTY"),
}

CASE_RULES = (
    (("crates/kuku/src/permission/*", "crates/kuku/src/query/run_js*",
      "crates/kuku-tui/src/*permission*", "crates/kuku-tui/src/notifications.rs",
      "scripts/acceptance/interactive_permissions.py"),
     ("permissions",)),
    (("crates/kuku-tui/*", "crates/kuku-client/*", "crates/kuku-cli/*", "apps/kuku/*",
      "scripts/acceptance/interactive_composer.py", "scripts/acceptance/interactive_rendering.py"),
     ("startup-info", "composer-actions", "paste", "statusline", "interrupt", "keyboard-queue")),
    (("crates/kuku-tui/src/setup*", "crates/kuku/src/config/*", "crates/kuku/src/config.rs",
      "crates/kuku-server/src/routes/models.rs", "scripts/acceptance/interactive_model_recovery.py"),
     ("first-run", "model-setup")),
    (("crates/kuku-tui/src/ui/*", "crates/kuku-tui/src/markup/*",
      "scripts/acceptance/interactive_rendering.py"),
     ("rendering",)),
    (("crates/kuku-server/*", "crates/kuku-client/*", "crates/kuku/src/event/store*",
      "crates/kuku/src/session/runtime/mod.rs", "crates/kuku/src/session/runtime/driver.rs",
      "crates/kuku/src/session/runtime/controls*", "crates/kuku/src/session/runtime/creation.rs",
      "crates/kuku/src/session/runtime/observe.rs",
      "crates/kuku/src/session/runtime/receipts.rs", "crates/kuku/src/session/runtime/recovery.rs",
      "crates/kuku/src/session/runtime/record*", "scripts/acceptance/interactive_lifecycle.py"),
     ("provider-failure", "lost-receipt")),
    (("crates/kuku/src/event/store*", "crates/kuku/src/context/revert*",
      "crates/kuku/src/session/runtime/history.rs", "crates/kuku-server/src/routes/events.rs",
      "crates/kuku-server/src/routes/rewind*", "crates/kuku-tui/src/*rewind*",
      "scripts/acceptance/interactive_history.py", "scripts/acceptance/interactive_lifecycle.py"),
     ("rewind", "rewind-history", "rewind-files")),
)
WEB = ("apps/web/*", ".github/workflows/web-ci.yml")
BROWSER = (*WEB, "crates/kuku-server/src/routes/*", "crates/kuku-server/src/service/*",
           "crates/kuku-server/src/service.rs", "crates/kuku-server/src/wire/*",
           "crates/kuku-server/src/wire.rs", "crates/kuku-client/src/http*",
           "scripts/acceptance/interactive_browser.py")
NATIVE_SDK = ("Cargo.toml", "Cargo.lock", ".cargo/*", "crates/kuku/build.rs",
              "crates/kuku/src/event/store.rs", "crates/kuku/src/event/store/archive.rs",
              "crates/kuku/src/event/store/snapshot.rs", "crates/kuku/src/event/store/packing.rs",
              "crates/kuku/src/util/fs_sync*", "crates/kuku/src/util/private_directory*",
              "crates/kuku/src/tool/builtin/common/file_io.rs",
              "crates/kuku/src/tool/builtin/run_command/*", "crates/kuku/tests/event_store.rs")
NATIVE_TERMINAL = ("crates/kuku-tui/src/terminal*", "crates/kuku-client/src/local*",
                   "apps/kuku/*", "scripts/acceptance/desktop/*", ".github/workflows/release.yml")
INSTALL_HARNESS = (".github/workflows/ci.yml", ".github/workflows/ci-platform.yml",
                   "scripts/acceptance/ci_plan.py",
                   "scripts/acceptance/run.py", "scripts/acceptance/interactive.py",
                   "scripts/acceptance/interactive_fixture.py")


def matches(paths, patterns):
    return any(fnmatchcase(path, pattern) for path in paths for pattern in patterns)


def platform_paths(paths, platform):
    other = "macos" if platform == "windows" else "windows"
    return tuple(path for path in paths if other not in Path(path).stem.lower())


def job(platform, contract, installed="none", cases=(), browser=False, web=False):
    os, terminal = PLATFORMS[platform]
    return dict(os=os, terminal=terminal, contract=contract, installed=installed,
                cases=",".join(cases), browser=browser, web=web,
                artifact="browser-linux" if browser and contract == "none" else os)


def plan(event, draft=False, platform="all", paths=()):
    if event in ("schedule", "workflow_dispatch"):
        platforms = PLATFORMS if platform == "all" else (platform,)
        jobs = [job(name, "workspace", "full", browser=name == "linux", web=True)
                for name in platforms]
        return jobs, [entry["os"] for entry in jobs], platform == "all"
    if event != "pull_request" or draft:
        return [job("linux", "workspace")], ["ubuntu-latest"], False

    harness = matches(paths, INSTALL_HARNESS)
    cases = dict.fromkeys(case for patterns, names in CASE_RULES if harness or matches(paths, patterns)
                          for case in names)
    jobs = [job("linux", "workspace", "selected" if cases else "none", cases=cases)]
    if harness or matches(paths, BROWSER):
        jobs.append(job("linux", "none", "selected", cases=("browser",), browser=True, web=True))
    tooling = ["ubuntu-latest"]
    for name in ("macos", "windows"):
        relevant = platform_paths(paths, name)
        sdk = harness or matches(relevant, NATIVE_SDK)
        terminal = harness or matches(relevant, NATIVE_TERMINAL)
        if sdk or terminal:
            jobs.append(job(name, "sdk" if sdk else "none", "smoke" if terminal else "none"))
        if harness or matches(relevant, ("scripts/acceptance/desktop/*",
                                         "scripts/acceptance/test_windows_desktop.py")):
            tooling.append(PLATFORMS[name][0])
    return jobs, tooling, matches(paths, WEB)


def changed_paths():
    data = subprocess.check_output(["git", "diff", "--name-only", "--no-renames", "-z", "HEAD^1", "HEAD"])
    return tuple(path.decode("utf-8", "surrogateescape") for path in data.split(b"\0") if path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--event", required=True, choices=("pull_request", "push", "schedule", "workflow_dispatch"))
    parser.add_argument("--draft", choices=("true", "false"), default="false")
    parser.add_argument("--platform", default="all", choices=("all", *PLATFORMS))
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    draft = args.draft == "true"
    paths = changed_paths() if args.event == "pull_request" and not draft else ()
    jobs, tooling, web_checks = plan(args.event, draft, args.platform, paths)
    outputs = {"platform_jobs": [entry for entry in jobs if not entry["web"]],
               "web_jobs": [entry for entry in jobs if entry["web"]],
               "tooling": tooling, "web_checks": web_checks}
    with args.output.open("a", encoding="utf-8") as stream:
        for name, value in outputs.items():
            print(f"{name}={json.dumps(value, separators=(',', ':'))}", file=stream)
    print(json.dumps(outputs, indent=2))


if __name__ == "__main__":
    main()
