#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
RUNTIME_HOME="${KUKU_HOME:-/tmp/kuku-home}"
CONFIG_SOURCE="${KUKU_CONFIG_SOURCE:-$SCRIPT_DIR/config.toml}"
DO_RUN="${KUKU_CHECK_RUN:-1}"
RUN_MODEL="${KUKU_CHECK_MODEL:-balanced}"
CHECK_PROMPT="${KUKU_CHECK_PROMPT:-/docs-translate This is a runtime check for the docs translation workflow. Read docs/AGENTS.md and reply with READY only. Do not modify files. Do not use gh.}"
WORKSPACE_ROOT="$(pwd)"
BUILD_TARGET_DIR="${CARGO_TARGET_DIR:-$WORKSPACE_ROOT/target}"

mkdir -p "$RUNTIME_HOME"

if [[ -n "$CONFIG_SOURCE" ]]; then
    cp "$CONFIG_SOURCE" "$RUNTIME_HOME/config.toml"
fi

cargo build -p kuku-app

BIN="$BUILD_TARGET_DIR/debug/kuku"

pushd "$WORKSPACE_ROOT" >/dev/null

echo "== skills list =="
"$BIN" skills list

echo "== skill show =="
"$BIN" skills show docs-translate

if [[ "$DO_RUN" == "1" ]]; then
    echo "== docs-translate run =="
    "$BIN" run -y --no-agents --model "$RUN_MODEL" --raw "$CHECK_PROMPT"
fi

popd >/dev/null
