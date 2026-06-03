#!/usr/bin/env bash
set -euo pipefail

usage() {
    printf 'usage: %s <before-sha> <after-sha>\n' "${0##*/}" >&2
}

if [[ $# -ne 2 ]]; then
    usage
    exit 1
fi

SUMMARY_FILE="${GITHUB_STEP_SUMMARY:-}"
if [[ -z "$SUMMARY_FILE" ]]; then
    printf 'GITHUB_STEP_SUMMARY is required\n' >&2
    exit 1
fi

BEFORE_SHA="$1"
AFTER_SHA="$2"

if [[ -z "$BEFORE_SHA" || "$BEFORE_SHA" == "0000000000000000000000000000000000000000" ]]; then
    BEFORE_SHA="$(git rev-list --max-parents=0 "$AFTER_SHA")"
fi

mapfile -d '' CHANGED_PATHS < <(git diff --name-only -z "$BEFORE_SHA" "$AFTER_SHA" -- docs/en)

SOURCES=()
for PATHNAME in "${CHANGED_PATHS[@]}"; do
    if [[ "$PATHNAME" == docs/en/*.md ]]; then
        SOURCES+=("$PATHNAME")
    fi
done

shopt -s nullglob
LOCALES=()
for INDEX_PATH in docs/*/index.md; do
    LOCALE_DIR="${INDEX_PATH%/index.md}"
    LOCALE="${LOCALE_DIR#docs/}"
    if [[ "$LOCALE" == "en" || ! "$LOCALE" =~ ^[A-Za-z0-9_-]+$ ]]; then
        continue
    fi
    if [[ ! -d "docs/$LOCALE/start" && ! -d "docs/$LOCALE/reference" ]]; then
        continue
    fi
    LOCALES+=("$LOCALE")
done
shopt -u nullglob

{
    printf '## docs/en changes\n\n'
    printf 'Range: `%s..%s`\n\n' "$BEFORE_SHA" "$AFTER_SHA"

    if [[ "${#SOURCES[@]}" -eq 0 ]]; then
        printf 'No changed Markdown pages under `docs/en/**`.\n'
        exit 0
    fi

    printf '| Source | Mirrored target path(s) |\n'
    printf '|---|---|\n'

    for SOURCE in "${SOURCES[@]}"; do
        RELATIVE_PATH="${SOURCE#docs/en/}"
        TARGETS=""

        for LOCALE in "${LOCALES[@]}"; do
            TARGET_PATH="docs/$LOCALE/$RELATIVE_PATH"
            if [[ -z "$TARGETS" ]]; then
                TARGETS="\`$TARGET_PATH\`"
            else
                TARGETS="$TARGETS<br>\`$TARGET_PATH\`"
            fi
        done

        if [[ -z "$TARGETS" ]]; then
            TARGETS="_no target locales found_"
        fi

        printf '| `%s` | %s |\n' "$SOURCE" "$TARGETS"
    done

    printf '\nThis report is deterministic and does not run a translation.\n'
} >> "$SUMMARY_FILE"
