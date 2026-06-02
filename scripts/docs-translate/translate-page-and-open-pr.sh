#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd -- "$SCRIPT_DIR/../.." && pwd)"
ENV_FILE="${KUKU_ENV_FILE:-$SCRIPT_DIR/.env.local}"
BASE_BRANCH="${KUKU_PR_BASE:-main}"

if [[ $# -lt 1 ]]; then
    printf 'usage: %s docs/en/path.md\n' "${0##*/}" >&2
    exit 1
fi

SOURCE_PATH="$1"

if [[ "$SOURCE_PATH" != docs/en/*.md ]]; then
    printf 'source path must be under docs/en and end with .md\n' >&2
    exit 1
fi

TARGET_PATH="docs/zh/${SOURCE_PATH#docs/en/}"

if [[ -f "$ENV_FILE" ]]; then
    set -a
    # shellcheck source=/dev/null
    source "$ENV_FILE"
    set +a
fi

if [[ -z "${GH_TOKEN:-}" ]]; then
    printf 'GH_TOKEN is required in %s\n' "$ENV_FILE" >&2
    exit 1
fi

if [[ -z "${KUKU_ANTHROPIC_API_KEY:-}" ]]; then
    printf 'KUKU_ANTHROPIC_API_KEY is required in %s\n' "$ENV_FILE" >&2
    exit 1
fi

SLUG="${SOURCE_PATH#docs/en/}"
SLUG="${SLUG%.md}"
SLUG="${SLUG//\//-}"
BRANCH_NAME="${KUKU_PR_BRANCH:-docs-translate-${SLUG}}"
PR_TITLE="${KUKU_PR_TITLE:-docs: sync zh for ${SOURCE_PATH#docs/en/}}"

PROMPT="/docs-translate Sync ${TARGET_PATH} from ${SOURCE_PATH}. Only update that mirrored page unless a directly related translated page clearly needs a consistency fix. Do not use gh. Keep canonical product terms, commands, paths, and links unchanged. When finished, reply with a short factual summary."

KUKU_CHECK_PROMPT="$PROMPT" bash "$SCRIPT_DIR/run-local-check.sh"

UPDATED_FILES="$(git diff --name-only -- docs/zh | sed '/^$/d')"

if [[ -z "$UPDATED_FILES" ]]; then
    printf 'no translated docs changes detected\n' >&2
    exit 1
fi

printf -v SOURCE_PAGES -- '- `%s`' "$SOURCE_PATH"
UPDATED_PAGES="$(printf '%s\n' "$UPDATED_FILES" | sed 's#^#- `#; s#$#`#')"
RELATED_UPDATES="$(printf '%s\n' "$UPDATED_FILES" | grep -Fvx "$TARGET_PATH" | sed 's#^#- `#; s#$#` - consistency update#' || true)"
if [[ -z "$RELATED_UPDATES" ]]; then
    RELATED_UPDATES='- none'
fi
NOTES='- machine-translated output; review before merge'

PR_BODY_FILE="$(mktemp /tmp/kuku-docs-pr-body.XXXXXX.md)"
cleanup() {
    rm -f "$PR_BODY_FILE"
}
trap cleanup EXIT

python3 - <<'PY' "$SCRIPT_DIR/pr-body-template.md" "$PR_BODY_FILE" "$SOURCE_PAGES" "$UPDATED_PAGES" "$RELATED_UPDATES" "$NOTES"
from pathlib import Path
import sys

template = Path(sys.argv[1]).read_text()
out = template.replace("__SOURCE_PAGES__", sys.argv[3])
out = out.replace("__UPDATED_PAGES__", sys.argv[4])
out = out.replace("__RELATED_UPDATES__", sys.argv[5])
out = out.replace("__NOTES__", sys.argv[6])
Path(sys.argv[2]).write_text(out)
PY

if git show-ref --verify --quiet "refs/heads/$BRANCH_NAME"; then
    git checkout "$BRANCH_NAME"
else
    git checkout -b "$BRANCH_NAME"
fi

git add .kuku/skills/docs-translate scripts/docs-translate "$TARGET_PATH"
git commit -m "docs: add local docs translate flow"
git push -u origin "$BRANCH_NAME"
gh pr create --base "$BASE_BRANCH" --head "$BRANCH_NAME" --title "$PR_TITLE" --body-file "$PR_BODY_FILE"
