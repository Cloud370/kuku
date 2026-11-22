"""Prepare and inspect an isolated reference Codex CLI benchmark."""

from collections import Counter
import json
import os
import platform
import re
import shutil

from benchmark_runtime import bundle_binary
from frontier_report import EvidenceError, session_events
from run import config_record


PACKAGE = "@openai/codex"
CATALOG = "deepseek-flash-models.json"


def codex_install(prefix, model):
    prefix = prefix.resolve(strict=True)
    arch = {"x86_64": "x64", "aarch64": "arm64"}.get(platform.machine())
    if not arch:
        raise ValueError("Codex comparison requires a Linux x64 or arm64 executable")
    native_package = f"{PACKAGE}-linux-{arch}"
    modules = prefix / "node_modules"
    lockfile = prefix / "package-lock.json"
    packages = json.loads(lockfile.read_text(encoding="utf-8"))["packages"]
    wrapper = packages[f"node_modules/{PACKAGE}"]
    native = packages[f"node_modules/{native_package}"]
    version = wrapper["version"]
    installed = modules / native_package
    manifest = json.loads((installed / "package.json").read_text(encoding="utf-8"))
    if (not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version)
            or native["version"] != f"{version}-linux-{arch}"
            or manifest["version"] != native["version"] or manifest["name"] != PACKAGE):
        raise ValueError("Codex native binary must match the pinned npm package")
    binary, = installed.glob("vendor/*/bin/codex")
    binary = binary.resolve(strict=True)
    if not binary.is_relative_to(installed.resolve()) or not os.access(binary, os.X_OK):
        raise ValueError("Codex comparison needs the installed executable")
    catalog = prefix / CATALOG
    if not 0 < catalog.stat().st_size <= 512 * 1024:
        raise ValueError("Codex benchmark catalog is missing or oversized")
    catalog_models = json.loads(catalog.read_text(encoding="utf-8"))["models"]
    if len(catalog_models) != 1 or catalog_models[0]["slug"] != model:
        raise ValueError("Codex benchmark catalog must contain the selected Flash model")
    return binary, catalog, {
        "name": PACKAGE, "version": version, "lock_sha256": config_record(lockfile)["sha256"],
        "integrity": native["integrity"], "catalog_sha256": config_record(catalog)["sha256"],
    }


def codex_config(route, endpoint, token_variable):
    return f'''
model = {json.dumps(route.model)}
model_provider = "deepseek"
model_catalog_json = "/kuku/runtime/{CATALOG}"
model_context_window = {route.context}
model_reasoning_effort = {json.dumps(route.effort)}
model_reasoning_summary = "none"
tool_output_token_limit = 10000
web_search = "disabled"
approval_policy = "never"
sandbox_mode = "danger-full-access"

[features]
multi_agent = false
multi_agent_v2 = false
goals = false

[agents]
enabled = false

[model_providers.deepseek]
name = "DeepSeek"
base_url = {json.dumps(endpoint)}
env_key = {json.dumps(token_variable)}
wire_api = "responses"
supports_websockets = false
stream_idle_timeout_ms = 900000
stream_max_retries = 1
request_max_retries = 1
'''


def prepare_codex(args, attempt, instruction, record):
    binary, catalog, record["codex_package"] = codex_install(args.codex_prefix, args.codex_model)
    command, record["runtime_files"] = bundle_binary(binary, attempt, filename="codex")
    shutil.copy2(catalog, attempt / "runtime" / CATALOG)
    record["runtime_files"][CATALOG] = record["codex_package"]["catalog_sha256"]
    launcher = 'ln -sT /kuku/data "$HOME/.codex" && exec "$@"'
    command = ["sh", "-c", launcher, "sh", *command, "exec", "--json", "--skip-git-repo-check", "--", instruction]
    return command, [], "/kuku/data/config.toml"


def inspect_codex(data_dir, instruction):
    journals = list((data_dir / "sessions").glob("*/*/*/rollout-*.jsonl"))
    if len(journals) != 1:
        return {"passed": False, "error": "expected one Codex rollout"}
    journal, = journals
    delivered = False
    complete = False
    pending = Counter()
    calls = results = 0
    try:
        for row in session_events(journal):
            event = row["payload"]
            event_type = event.get("type")
            if row["type"] == "response_item" and event_type == "message" and event.get("role") == "user":
                delivered |= any(part.get("type") == "input_text" and part.get("text") == instruction
                                 for part in event["content"])
            if not delivered:
                continue
            if row["type"] == "response_item" and event_type in ("function_call", "custom_tool_call"):
                pending[event["call_id"]] += 1
                calls += 1
            elif row["type"] == "response_item" and event_type in ("function_call_output", "custom_tool_call_output"):
                call_id = event["call_id"]
                if pending[call_id] <= 0:
                    raise ValueError("unpaired Codex tool result")
                pending[call_id] -= 1
                results += 1
            elif row["type"] == "event_msg" and event_type in ("task_complete", "turn_complete"):
                complete = isinstance(event.get("last_agent_message"), str)
    except (OSError, AttributeError, TypeError, ValueError, KeyError, EvidenceError) as error:
        return {"passed": False, "error": type(error).__name__}
    return {"passed": delivered and complete and 0 < calls == results,
            "submitted_exact_instruction": delivered, "terminal_turn_complete": complete,
            "tool_calls": calls, "tool_results": results, "session_file": str(journal.relative_to(data_dir))}
