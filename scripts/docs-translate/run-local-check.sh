#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd -- "$SCRIPT_DIR/../.." && pwd)"
ENV_FILE="${KUKU_ENV_FILE:-$SCRIPT_DIR/.env.local}"

if [[ -f "$ENV_FILE" ]]; then
    set -a
    # shellcheck source=/dev/null
    source "$ENV_FILE"
    set +a
fi

IMAGE_TAG="${KUKU_AGENT_RUNNER_IMAGE:-kuku-agent-runner:local}"
CACHE_ROOT="${KUKU_CACHE_ROOT:-/tmp/kuku/docs-translate}"
CONTAINER_CACHE_DIR="${KUKU_CONTAINER_CACHE_DIR:-/kuku-cache}"
RUN_ID="${KUKU_RUN_ID:-}"
if [[ -z "$RUN_ID" ]]; then
    RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
fi
KUKU_HOME_HOST="$CACHE_ROOT/kuku-home-runs/$RUN_ID"
KUKU_HOME_CONTAINER="$CONTAINER_CACHE_DIR/kuku-home-runs/$RUN_ID"
mkdir -p "$CACHE_ROOT/target" "$CACHE_ROOT/kuku-home-runs" "$CACHE_ROOT/cargo-home" "$CACHE_ROOT/rustup" "$KUKU_HOME_HOST"

SOURCE_REVISION="${KUKU_SOURCE_REVISION:-$(git -C "$WORKSPACE_ROOT" rev-parse HEAD 2>/dev/null || true)}"
docker run --rm \
    --user "$(id -u):$(id -g)" \
    -e CARGO_TARGET_DIR="$CONTAINER_CACHE_DIR/target" \
    -e CARGO_HOME="$CONTAINER_CACHE_DIR/cargo-home" \
    -e RUSTUP_HOME="$CONTAINER_CACHE_DIR/rustup" \
    -e KUKU_SOURCE_REVISION="$SOURCE_REVISION" \
    -v "$WORKSPACE_ROOT:/workspace:ro" \
    -v "$CACHE_ROOT/target:$CONTAINER_CACHE_DIR/target" \
    -v "$CACHE_ROOT/cargo-home:$CONTAINER_CACHE_DIR/cargo-home" \
    -v "$CACHE_ROOT/rustup:$CONTAINER_CACHE_DIR/rustup" \
    -w /workspace \
    "$IMAGE_TAG" \
    cargo build --locked -p kuku-app >&2

KEY_ARGS=()
if [[ "${KUKU_CHECK_RUN:-1}" == "1" && -z "${KUKU_CHECK_SESSION_ID:-}" ]]; then
    if [[ -z "${CI_LLM_API_KEY:-}" || -z "${CI_LLM_BASE_URL:-}" ]]; then
        printf 'CI_LLM_API_KEY and CI_LLM_BASE_URL are required for a model run\n' >&2
        exit 1
    fi
    key_file="$(mktemp)"
    trap 'rm -f -- "$key_file"' EXIT
    printf '%s' "$CI_LLM_API_KEY" > "$key_file"
    KEY_ARGS=(-v "$key_file:/run/kuku-docs-translate.key:ro")
fi

WORKSPACE_ARGS=()
if [[ -n "${KUKU_CHECK_WORKSPACE:-}" ]]; then
    WORKSPACE_ARGS=(-e KUKU_CHECK_WORKSPACE=/translation -v "$KUKU_CHECK_WORKSPACE:/translation")
fi

docker run --rm --read-only --tmpfs /tmp \
    --user "$(id -u):$(id -g)" \
    -e KUKU_HOME="$KUKU_HOME_CONTAINER" \
    -e KUKU_CHECK_RUN="${KUKU_CHECK_RUN:-1}" \
    -e KUKU_CHECK_OUTPUT_MODE="${KUKU_CHECK_OUTPUT_MODE:-raw}" \
    -e KUKU_CHECK_PRESET \
    -e KUKU_CHECK_PROMPT="${KUKU_CHECK_PROMPT:-}" \
    -e KUKU_CHECK_SESSION_ID="${KUKU_CHECK_SESSION_ID:-}" \
    -e KUKU_CHECK_SHOW_SKILL_INFO="${KUKU_CHECK_SHOW_SKILL_INFO:-0}" \
    -e KUKU_CONFIG_SOURCE=/workspace/scripts/docs-translate/config.toml \
    -e CI_LLM_BASE_URL \
    -e CI_LLM_MODEL \
    -v "$WORKSPACE_ROOT:/workspace:ro" \
    -v "$CACHE_ROOT/target/debug/kuku:/usr/local/bin/kuku:ro" \
    -v "$KUKU_HOME_HOST:$KUKU_HOME_CONTAINER" \
    "${KEY_ARGS[@]}" \
    "${WORKSPACE_ARGS[@]}" \
    -w /workspace \
    "$IMAGE_TAG" \
    bash scripts/docs-translate/check-in-container.sh
