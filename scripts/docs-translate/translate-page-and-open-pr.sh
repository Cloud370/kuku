#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="${KUKU_ENV_FILE:-$SCRIPT_DIR/.env.local}"
PROMPT_TEMPLATE_FILE="$SCRIPT_DIR/prompts/translate-page-and-open-pr.txt"

if [[ -f "$ENV_FILE" ]]; then
    set -a
    # shellcheck source=/dev/null
    source "$ENV_FILE"
    set +a
fi

BASE_BRANCH="${KUKU_PR_BASE:-main}"
TARGET_LOCALE="${KUKU_DOCS_TARGET_LOCALE:-}"
EXISTING_PR_REF="${KUKU_DOCS_PR:-}"
RUN_ID="${KUKU_RUN_ID:-}"
if [[ -z "$RUN_ID" ]]; then
    RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
fi

usage() {
    printf 'usage: %s --locale <locale> [--pr <number-or-url>] docs/en/path.md\n' "${0##*/}" >&2
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --locale|-l)
            if [[ $# -lt 2 ]]; then
                usage
                exit 1
            fi
            TARGET_LOCALE="$2"
            shift 2
            ;;
        --pr)
            if [[ $# -lt 2 ]]; then
                usage
                exit 1
            fi
            EXISTING_PR_REF="$2"
            shift 2
            ;;
        --base)
            if [[ $# -lt 2 ]]; then
                usage
                exit 1
            fi
            BASE_BRANCH="$2"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        --)
            shift
            break
            ;;
        -*)
            usage
            exit 1
            ;;
        *)
            break
            ;;
    esac
done

if [[ $# -ne 1 ]]; then
    usage
    exit 1
fi

SOURCE_PATH="$1"

if [[ -z "$TARGET_LOCALE" ]]; then
    printf 'target locale is required\n' >&2
    usage
    exit 1
fi

if [[ "$SOURCE_PATH" != docs/en/*.md ]]; then
    printf 'source path must be under docs/en and end with .md\n' >&2
    exit 1
fi

if [[ ! "$TARGET_LOCALE" =~ ^[A-Za-z0-9_-]+$ ]]; then
    printf 'target locale must match [A-Za-z0-9_-]+\n' >&2
    exit 1
fi

TARGET_PATH="docs/$TARGET_LOCALE/${SOURCE_PATH#docs/en/}"
TARGET_EXISTS="no"
if [[ -f "$TARGET_PATH" ]]; then
    TARGET_EXISTS="yes"
fi

if [[ -z "${GH_TOKEN:-}" ]]; then
    printf 'GH_TOKEN is required in %s\n' "$ENV_FILE" >&2
    exit 1
fi

if [[ -z "${KUKU_ANTHROPIC_API_KEY:-}" ]]; then
    printf 'KUKU_ANTHROPIC_API_KEY is required in %s\n' "$ENV_FILE" >&2
    exit 1
fi

if [[ -z "${KUKU_ANTHROPIC_BASE_URL:-}" ]]; then
    printf 'KUKU_ANTHROPIC_BASE_URL is required in %s\n' "$ENV_FILE" >&2
    exit 1
fi

CURRENT_BRANCH="$(git branch --show-current)"
if [[ -z "$CURRENT_BRANCH" ]]; then
    printf 'current branch is required; detached HEAD is not supported\n' >&2
    exit 1
fi

if [[ ! -f "$PROMPT_TEMPLATE_FILE" ]]; then
    printf 'prompt template file not found: %s\n' "$PROMPT_TEMPLATE_FILE" >&2
    exit 1
fi
PROMPT_TEMPLATE="$(<"$PROMPT_TEMPLATE_FILE")"
PROMPT="${PROMPT_TEMPLATE//\{\{SOURCE_PATH\}\}/$SOURCE_PATH}"
PROMPT="${PROMPT//\{\{TARGET_PATH\}\}/$TARGET_PATH}"
PROMPT="${PROMPT//\{\{TARGET_LOCALE\}\}/$TARGET_LOCALE}"
PROMPT="$PROMPT

Context:
- Mode: open-pr
- Source path: \`$SOURCE_PATH\`
- Target locale: \`$TARGET_LOCALE\`
- Target path: \`$TARGET_PATH\`
- Target page exists: $TARGET_EXISTS
- Current branch: \`$CURRENT_BRANCH\`
- Base branch: \`$BASE_BRANCH\`"
if [[ -n "$EXISTING_PR_REF" ]]; then
    PROMPT="$PROMPT
- Explicit PR reference: \`$EXISTING_PR_REF\`"
fi
PROMPT="$PROMPT

When finished, return only the PR body text."

SESSION_ID="$(KUKU_RUN_ID="$RUN_ID" KUKU_CHECK_SHOW_SKILL_INFO=0 KUKU_CHECK_OUTPUT_MODE=session-id KUKU_CHECK_PROMPT="$PROMPT" bash "$SCRIPT_DIR/run-local-check.sh")"

if [[ -z "$SESSION_ID" ]]; then
    printf 'json output did not include a session id\n' >&2
    exit 1
fi

PR_BODY="$(KUKU_RUN_ID="$RUN_ID" KUKU_CHECK_RUN=0 KUKU_CHECK_SHOW_SKILL_INFO=0 KUKU_CHECK_SESSION_ID="$SESSION_ID" bash "$SCRIPT_DIR/run-local-check.sh")"

if [[ -z "$PR_BODY" ]]; then
    printf 'kuku show returned no final output for session %s\n' "$SESSION_ID" >&2
    exit 1
fi

printf '%s\n' "$PR_BODY"
