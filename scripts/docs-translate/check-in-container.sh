#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
RUNTIME_HOME="${KUKU_HOME:-/tmp/kuku-home}"
CONFIG_SOURCE="${KUKU_CONFIG_SOURCE:-$SCRIPT_DIR/config.toml}"
DO_RUN="${KUKU_CHECK_RUN:-1}"
RUN_MODEL="${KUKU_CHECK_MODEL:-balanced}"
OUTPUT_MODE="${KUKU_CHECK_OUTPUT_MODE:-raw}"
CHECK_PROMPT="${KUKU_CHECK_PROMPT:-/docs-translate This is a runtime check for the docs translation workflow. Read docs/AGENTS.md and reply with READY only. Do not modify files. Do not use gh.}"
SHOW_SESSION_ID="${KUKU_CHECK_SESSION_ID:-}"
SHOW_SKILL_INFO="${KUKU_CHECK_SHOW_SKILL_INFO:-0}"
WORKSPACE_ROOT="$(pwd)"
BUILD_TARGET_DIR="${CARGO_TARGET_DIR:-$WORKSPACE_ROOT/target}"

mkdir -p "$RUNTIME_HOME"

if [[ -n "$CONFIG_SOURCE" ]]; then
    cp "$CONFIG_SOURCE" "$RUNTIME_HOME/config.toml"
fi

case "$OUTPUT_MODE" in
    raw)
        OUTPUT_ARGS=(--raw)
        ;;
    json)
        OUTPUT_ARGS=(--json)
        ;;
    stream-json)
        OUTPUT_ARGS=(--stream-json)
        ;;
    session-id)
        OUTPUT_ARGS=(--json)
        ;;
    *)
        printf 'unsupported KUKU_CHECK_OUTPUT_MODE: %s\n' "$OUTPUT_MODE" >&2
        exit 1
        ;;
esac

cargo build -p kuku-app >&2

BIN="$BUILD_TARGET_DIR/debug/kuku"

pushd "$WORKSPACE_ROOT" >/dev/null

if [[ "$SHOW_SKILL_INFO" == "1" && "$OUTPUT_MODE" == "raw" ]]; then
    echo "== skills list =="
    "$BIN" skills list

    echo "== skill show =="
    "$BIN" skills show docs-translate
fi

if [[ -n "$SHOW_SESSION_ID" ]]; then
    "$BIN" show "$SHOW_SESSION_ID"
elif [[ "$DO_RUN" == "1" ]]; then
    if [[ "$SHOW_SKILL_INFO" == "1" && "$OUTPUT_MODE" == "raw" ]]; then
        echo "== docs-translate run =="
    fi
    if [[ "$OUTPUT_MODE" == "session-id" ]]; then
        RUN_JSON="$($BIN run -y --no-agents --model "$RUN_MODEL" "${OUTPUT_ARGS[@]}" "$CHECK_PROMPT")"
        printf '%s\n' "$RUN_JSON" | jq -r '.session_id // empty'
    else
        "$BIN" run -y --no-agents --model "$RUN_MODEL" "${OUTPUT_ARGS[@]}" "$CHECK_PROMPT"
    fi
fi

popd >/dev/null
