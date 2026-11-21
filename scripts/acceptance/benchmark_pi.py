"""Configure the isolated Pi reference runner for the same direct DeepSeek route."""

from collections import Counter
import json
import os
from pathlib import Path
import shutil

from benchmark_runtime import bundle_binary
from run import config_record


PACKAGE = "@earendil-works/pi-coding-agent"
USAGE_HOOK = """const fs = require("node:fs");
process.on("exit", () => {
  const usage = process.resourceUsage();
  fs.writeFileSync("/logs/agent/result.json", JSON.stringify({process_resources: {
    user_cpu_seconds: usage.userCPUTime / 1e6,
    system_cpu_seconds: usage.systemCPUTime / 1e6,
    peak_rss_kib: usage.maxRSS,
    input_blocks: usage.fsRead,
    output_blocks: usage.fsWrite,
  }}));
});
"""


def pi_config(route, endpoint, limits, token_variable):
    return {"providers": {"deepseek": {
        "baseUrl": endpoint, "api": "openai-completions", "apiKey": "$" + token_variable,
        "models": [{
            "id": route.model, "name": route.model, "reasoning": True, "input": ["text"],
            "contextWindow": route.context, "maxTokens": limits.output_tokens,
            "thinkingLevelMap": {"low": "low", "high": "high", "max": "max"},
            "compat": {"thinkingFormat": "deepseek", "maxTokensField": "max_tokens",
                       "supportsDeveloperRole": False, "supportsReasoningEffort": True,
                       "supportsStore": False, "requiresReasoningContentOnAssistantMessages": True},
        }],
    }}}


def pi_install(prefix, node_binary=None):
    prefix = prefix.resolve(strict=True)
    modules = prefix / "node_modules"
    manifest = json.loads((modules / PACKAGE / "package.json").read_text())
    lock = prefix / "package-lock.json"
    locked_package = json.loads(lock.read_text())["packages"]["node_modules/" + PACKAGE]
    cli = (modules / PACKAGE / manifest["bin"]["pi"]).resolve(strict=True)
    if (manifest["name"] != PACKAGE or manifest["version"] != locked_package["version"]
            or not cli.is_relative_to(modules) or not cli.is_file()):
        raise ValueError("Pi package or CLI does not match the isolated npm lockfile")
    node_binary = node_binary or shutil.which("node")
    if not node_binary:
        raise ValueError("Pi comparison needs Node.js on PATH or --node-binary")
    node = Path(node_binary).resolve(strict=True)
    if not node.is_file() or not os.access(node, os.X_OK):
        raise ValueError("Pi comparison needs a Linux Node.js executable")
    return modules, cli.relative_to(modules), node, {
        "name": PACKAGE, "version": manifest["version"], "lock_sha256": config_record(lock)["sha256"],
        "integrity": locked_package["integrity"], "cli_sha256": config_record(cli)["sha256"],
    }


def prepare_pi(args, attempt, instruction, record):
    modules, cli, node, record["pi_package"] = pi_install(args.pi_prefix, args.node_binary)
    command, record["runtime_files"] = bundle_binary(node, attempt, filename="node")
    hook = attempt / "runtime" / "pi-usage.cjs"
    hook.write_text(USAGE_HOOK, encoding="utf-8")
    record["runtime_files"][hook.name] = config_record(hook)["sha256"]
    command += ["--require", "/kuku/runtime/pi-usage.cjs", (Path("/kuku/modules") / cli).as_posix(),
                "--offline", "--mode", "json", "--session-dir", "/kuku/data/sessions",
                "--provider", "deepseek", "--model", args.pi_model, "--thinking", args.pi_effort,
                "--", instruction]
    docker_args = ["--mount", f"type=bind,source={modules},target=/kuku/modules,readonly",
                   "--env", "PI_CODING_AGENT_DIR=/kuku/data"]
    return command, docker_args, "/kuku/data/models.json"


def inspect_pi(data_dir, instruction):
    sessions = list((data_dir / "sessions").glob("*.jsonl"))
    if len(sessions) != 1:
        return {"passed": False, "error": "expected one Pi Session ledger"}
    path, = sessions
    delivered = False
    last_assistant = None
    last_result = -1
    pending = Counter()
    calls = results = 0
    try:
        with path.open("rb") as stream:
            for index, line in enumerate(iter(lambda: stream.readline(1024 * 1024 + 1), b"")):
                if index >= 100_000 or len(line) > 1024 * 1024 or not line.endswith(b"\n"):
                    raise ValueError("Pi Session ledger is incomplete or exceeds inspection limits")
                event = json.loads(line)
                if event.get("type") != "message":
                    continue
                message = event["message"]
                if message.get("role") == "user":
                    content = message["content"]
                    delivered |= content == instruction if isinstance(content, str) else any(
                        block.get("type") == "text" and block.get("text") == instruction for block in content)
                if not delivered:
                    continue
                if message.get("role") == "assistant":
                    last_assistant = (index, message.get("stopReason"))
                    for block in message["content"]:
                        if block.get("type") == "toolCall":
                            pending[block["id"]] += 1
                            calls += 1
                elif message.get("role") == "toolResult":
                    call_id = message["toolCallId"]
                    if pending[call_id] <= 0:
                        raise ValueError("unpaired Pi tool result")
                    pending[call_id] -= 1
                    last_result = index
                    results += 1
    except (OSError, ValueError, AttributeError, TypeError, KeyError) as error:
        return {"passed": False, "error": type(error).__name__}
    terminal, stop_reason = last_assistant or (-1, None)
    completed = delivered and stop_reason == "stop" and 0 < calls == results and terminal > last_result
    return {"passed": completed, "submitted_exact_instruction": delivered, "tool_calls": calls,
            "tool_results": results, "terminal_stop_reason": stop_reason,
            "session_file": str(path.relative_to(data_dir))}
