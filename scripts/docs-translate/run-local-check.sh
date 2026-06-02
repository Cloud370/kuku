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
CACHE_ROOT="${KUKU_CACHE_ROOT:-/tmp/opencode/docs-translate}"
CONTAINER_CACHE_DIR="${KUKU_CONTAINER_CACHE_DIR:-/kuku-cache}"
RUN_ID="${KUKU_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
KUKU_HOME_HOST="$CACHE_ROOT/kuku-home-runs/$RUN_ID"
KUKU_HOME_CONTAINER="$CONTAINER_CACHE_DIR/kuku-home-runs/$RUN_ID"

mkdir -p "$CACHE_ROOT/target" "$CACHE_ROOT/kuku-home-runs" "$CACHE_ROOT/cargo-home" "$CACHE_ROOT/rustup" "$KUKU_HOME_HOST"

docker run --rm \
    --user "$(id -u):$(id -g)" \
    -e CARGO_TARGET_DIR="$CONTAINER_CACHE_DIR/target" \
    -e CARGO_HOME="$CONTAINER_CACHE_DIR/cargo-home" \
    -e KUKU_HOME="$KUKU_HOME_CONTAINER" \
    -e RUSTUP_HOME="$CONTAINER_CACHE_DIR/rustup" \
    -e KUKU_CHECK_RUN="${KUKU_CHECK_RUN:-1}" \
    -e KUKU_CHECK_MODEL="${KUKU_CHECK_MODEL:-balanced}" \
    -e KUKU_CHECK_OUTPUT_MODE="${KUKU_CHECK_OUTPUT_MODE:-raw}" \
    -e KUKU_CHECK_PROMPT="${KUKU_CHECK_PROMPT:-}" \
    -e KUKU_CHECK_SESSION_ID="${KUKU_CHECK_SESSION_ID:-}" \
    -e KUKU_CHECK_SHOW_SKILL_INFO="${KUKU_CHECK_SHOW_SKILL_INFO:-0}" \
    -e KUKU_CONFIG_SOURCE=/workspace/scripts/docs-translate/config.toml \
    -e GH_TOKEN="${GH_TOKEN:-}" \
    -e GITHUB_TOKEN="${GITHUB_TOKEN:-}" \
    -e KUKU_ANTHROPIC_BASE_URL="${KUKU_ANTHROPIC_BASE_URL:-}" \
    -e KUKU_ANTHROPIC_API_KEY="${KUKU_ANTHROPIC_API_KEY:-}" \
    -v "$WORKSPACE_ROOT:/workspace" \
    -v "$CACHE_ROOT:$CONTAINER_CACHE_DIR" \
    -w /workspace \
    "$IMAGE_TAG" \
    bash scripts/docs-translate/check-in-container.sh
