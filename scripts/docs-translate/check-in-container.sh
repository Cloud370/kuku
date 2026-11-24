#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
RUNTIME_HOME="${KUKU_HOME:-/tmp/kuku-home}"
CONFIG_SOURCE="${KUKU_CONFIG_SOURCE:-$SCRIPT_DIR/config.toml}"
DO_RUN="${KUKU_CHECK_RUN:-1}"
OUTPUT_MODE="${KUKU_CHECK_OUTPUT_MODE:-raw}"
CHECK_PROMPT="${KUKU_CHECK_PROMPT:-Read docs/AGENTS.md and reply with READY.}"
SHOW_SESSION_ID="${KUKU_CHECK_SESSION_ID:-}"
SHOW_SKILL_INFO="${KUKU_CHECK_SHOW_SKILL_INFO:-0}"
WORKSPACE_ROOT="$(pwd)"
export CI_LLM_MODEL="${CI_LLM_MODEL:-deepseek-flash}"
BIN=/usr/local/bin/kuku
RUN_ARGS=(--no-agents)
if [[ -n "${KUKU_CHECK_PRESET:-}" ]]; then
    RUN_ARGS+=(--preset "$KUKU_CHECK_PRESET")
fi

mkdir -p "$RUNTIME_HOME"

if [[ -n "$CONFIG_SOURCE" ]]; then
    cp "$CONFIG_SOURCE" "$RUNTIME_HOME/config.toml"
fi

case "$OUTPUT_MODE" in
    raw)
        OUTPUT_ARGS=(--raw)
        ;;
    stream-json)
        OUTPUT_ARGS=(--stream-json)
        ;;
    json|session-id|response)
        OUTPUT_ARGS=(--json)
        ;;
    *)
        printf 'unsupported KUKU_CHECK_OUTPUT_MODE: %s\n' "$OUTPUT_MODE" >&2
        exit 1
        ;;
esac

pushd "${KUKU_CHECK_WORKSPACE:-$WORKSPACE_ROOT}" >/dev/null

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
    case "$OUTPUT_MODE" in
        session-id|response)
            run_status=0
            printf '%s' "$CHECK_PROMPT" | "$BIN" run "${RUN_ARGS[@]}" "${OUTPUT_ARGS[@]}" --prompt-file - \
                > "$RUNTIME_HOME/session-run.json" || run_status=$?
            if [[ "$run_status" != "0" ]]; then
                printf 'kuku run exited with status %s\n' "$run_status" >&2
                exit "$run_status"
            fi
            if [[ "$OUTPUT_MODE" == "session-id" ]]; then
                jq -r '.session_id // empty' "$RUNTIME_HOME/session-run.json"
            else
                jq -r '.response // empty' "$RUNTIME_HOME/session-run.json"
            fi
            ;;
        *)
            printf '%s' "$CHECK_PROMPT" | "$BIN" run "${RUN_ARGS[@]}" "${OUTPUT_ARGS[@]}" --prompt-file -
            ;;
    esac
else
    CI_LLM_BASE_URL=https://example.invalid/v1 "$BIN" config validate |
        jq -e 'select(.status == "valid")' >&2
fi

popd >/dev/null
