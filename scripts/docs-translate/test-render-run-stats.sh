#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

SUMMARY_JSON="$TMP_DIR/session-run.json"
cat > "$SUMMARY_JSON" <<'EOF'
{"type":"session","event":"completed","session_id":"s_123","tier":"balanced","model":"claude-sonnet-4-6","turns":2,"input_tokens":1234,"output_tokens":567,"cache_read_input_tokens":89,"cache_creation_input_tokens":21,"duration_ms":3456}
EOF

OUTPUT="$(bash "$SCRIPT_DIR/render-run-stats.sh" "$SUMMARY_JSON")"

[[ "$OUTPUT" == *"### Run stats"* ]] || {
    printf 'expected summary header\n' >&2
    exit 1
}
[[ "$OUTPUT" == *'- Session: `s_123`'* ]] || {
    printf 'expected session id\n' >&2
    exit 1
}
[[ "$OUTPUT" == *'- Turns: `2`'* ]] || {
    printf 'expected turns\n' >&2
    exit 1
}
[[ "$OUTPUT" == *'- Input tokens: `1234`'* ]] || {
    printf 'expected input tokens\n' >&2
    exit 1
}
[[ "$OUTPUT" == *'- Duration: `3.4s`'* ]] || {
    printf 'expected formatted duration\n' >&2
    exit 1
}

MISSING_OUTPUT="$(bash "$SCRIPT_DIR/render-run-stats.sh" "$TMP_DIR/missing.json")"
[[ "$MISSING_OUTPUT" == *'Run stats unavailable'* ]] || {
    printf 'expected missing-file fallback\n' >&2
    exit 1
}

printf 'render-run-stats tests passed\n'
