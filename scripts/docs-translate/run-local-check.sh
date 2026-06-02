#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd -- "$SCRIPT_DIR/../.." && pwd)"
ENV_FILE="${KUKU_ENV_FILE:-$SCRIPT_DIR/.env.local}"
IMAGE_TAG="${KUKU_AGENT_RUNNER_IMAGE:-kuku-agent-runner:local}"
CACHE_ROOT="${KUKU_CACHE_ROOT:-${XDG_CACHE_HOME:-$HOME/.cache}/kuku/docs-translate}"

if [[ -f "$ENV_FILE" ]]; then
    set -a
    # shellcheck source=/dev/null
    source "$ENV_FILE"
    set +a
fi

mkdir -p "$CACHE_ROOT/target" "$CACHE_ROOT/kuku-home" "$CACHE_ROOT/cargo-home" "$CACHE_ROOT/rustup"

docker run --rm \
    -e CARGO_TARGET_DIR=/cache/target \
    -e CARGO_HOME=/cache/cargo-home \
    -e KUKU_HOME=/cache/kuku-home \
    -e RUSTUP_HOME=/cache/rustup \
    -e KUKU_CHECK_RUN="${KUKU_CHECK_RUN:-1}" \
    -e KUKU_CHECK_MODEL="${KUKU_CHECK_MODEL:-balanced}" \
    -e KUKU_CHECK_PROMPT="${KUKU_CHECK_PROMPT:-}" \
    -e KUKU_CONFIG_SOURCE=/workspace/scripts/docs-translate/config.toml \
    -e GH_TOKEN="${GH_TOKEN:-}" \
    -e GITHUB_TOKEN="${GITHUB_TOKEN:-}" \
    -e KUKU_ANTHROPIC_API_KEY="${KUKU_ANTHROPIC_API_KEY:-}" \
    -v "$WORKSPACE_ROOT:/workspace" \
    -v "$CACHE_ROOT:/cache" \
    -w /workspace \
    "$IMAGE_TAG" \
    bash scripts/docs-translate/check-in-container.sh
