#!/usr/bin/env bash
set -euo pipefail

usage() {
    printf 'usage: %s --head <branch> --base <branch> --title <title> --body-file <path>\n' "${0##*/}" >&2
}

HEAD_BRANCH=""
BASE_BRANCH=""
PR_TITLE=""
BODY_FILE=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --head)
            HEAD_BRANCH="${2:-}"
            shift 2
            ;;
        --base)
            BASE_BRANCH="${2:-}"
            shift 2
            ;;
        --title)
            PR_TITLE="${2:-}"
            shift 2
            ;;
        --body-file)
            BODY_FILE="${2:-}"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            usage
            exit 1
            ;;
    esac
done

if [[ -z "$HEAD_BRANCH" || -z "$BASE_BRANCH" || -z "$PR_TITLE" || -z "$BODY_FILE" ]]; then
    usage
    exit 1
fi

if [[ ! -f "$BODY_FILE" ]]; then
    printf 'PR body file does not exist: %s\n' "$BODY_FILE" >&2
    exit 1
fi

existing_url="$(gh pr list --head "$HEAD_BRANCH" --base "$BASE_BRANCH" --state open --json url --jq '.[0].url // empty')"
if [[ -n "$existing_url" ]]; then
    printf '%s\n' "$existing_url"
    exit 0
fi

gh pr create --head "$HEAD_BRANCH" --base "$BASE_BRANCH" --title "$PR_TITLE" --body-file "$BODY_FILE"
