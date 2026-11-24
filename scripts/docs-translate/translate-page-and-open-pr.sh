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

die() {
    printf '%s\n' "$*" >&2
    exit 1
}

usage() {
    printf 'usage: %s --locale LOCALE [--base BRANCH] [--branch BRANCH] [--pr NUMBER] docs/en/path.md\n' "${0##*/}" >&2
}

locale="${KUKU_DOCS_TARGET_LOCALE:-}"
base="${KUKU_PR_BASE:-}"
branch=""
pr="${KUKU_DOCS_PR:-}"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --locale|--base|--branch|--pr)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            case "$1" in
                --locale) locale="$2" ;;
                --base) base="$2" ;;
                --branch) branch="$2" ;;
                --pr) pr="$2" ;;
            esac
            shift 2
            ;;
        --) shift; break ;;
        --help|-h) usage; exit 0 ;;
        -*) usage; exit 2 ;;
        *) break ;;
    esac
done
[[ $# == 1 ]] || { usage; exit 2; }
source_path="$1"
[[ "$source_path" == docs/en/*.md && "$source_path" != *[[:cntrl:]]* ]] ||
    die "source path must be a Markdown page under docs/en"
case "/$source_path/" in
    *//*|*/./*|*/../*) die "source path cannot contain empty, dot, or dot-dot components" ;;
esac
[[ "$locale" =~ ^[A-Za-z0-9_-]+$ && "$locale" != en ]] ||
    die "target locale must match [A-Za-z0-9_-]+ and differ from en"
[[ -z "$pr" || "$pr" =~ ^[1-9][0-9]*$ ]] || die "PR number must be a positive integer"
[[ -n "${GH_TOKEN:-}" ]] || die "GH_TOKEN is required"

cd "$WORKSPACE_ROOT"
relative_path="${source_path#docs/en/}"
target_path="docs/$locale/$relative_path"
title="docs: sync $locale for $relative_path"
run_id="${KUKU_RUN_ID:-${GITHUB_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)-$$}}"
[[ "$run_id" =~ ^[A-Za-z0-9_-]+$ ]] || die "invalid run identifier"
stats_file="${KUKU_CACHE_ROOT:-/tmp/kuku/docs-translate}/kuku-home-runs/$run_id/session-run.json"

check_branch() {
    [[ "$1" =~ ^[A-Za-z0-9._/-]+$ && "$1" != -* ]] &&
        git check-ref-format "refs/heads/$1" ||
        die "invalid branch name: $1"
}

authenticated_git() {
    git -c credential.helper= -c 'credential.helper=!gh auth git-credential' "$@"
}

pr_url=""
if [[ -n "$pr" ]]; then
    pr_record="$(gh pr view "$pr" --json state,isCrossRepository,baseRefName,headRefName,url)"
    jq -e '.state == "OPEN" and .isCrossRepository == false' <<< "$pr_record" >/dev/null ||
        die "PR $pr must be open and from this repository"
    pr_base="$(jq -er '.baseRefName' <<< "$pr_record")"
    pr_branch="$(jq -er '.headRefName' <<< "$pr_record")"
    [[ -z "$base" || "$base" == "$pr_base" ]] || die "PR $pr has base branch $pr_base"
    [[ -z "$branch" || "$branch" == "$pr_branch" ]] || die "PR $pr has head branch $pr_branch"
    base="$pr_base"
    branch="$pr_branch"
    pr_url="$(jq -er '.url' <<< "$pr_record")"
else
    base="${base:-${GITHUB_REF_NAME:-$(git branch --show-current)}}"
    slug="$(printf '%s' "${relative_path%.md}" | tr '/' '-' | tr -cd 'A-Za-z0-9._-')"
    branch="${branch:-docs-translate/$locale/$slug-$run_id}"
fi
check_branch "$base"
check_branch "$branch"
[[ "$base" != "$branch" ]] || die "head and base branches must differ"

remote_head_status=0
remote_head="$(authenticated_git ls-remote --exit-code --heads origin "refs/heads/$branch")" ||
    remote_head_status=$?
[[ "$remote_head_status" == 0 || "$remote_head_status" == 2 ]] || exit "$remote_head_status"
[[ -z "$pr" || -n "$remote_head" ]] || die "PR $pr head branch is unavailable"
expected_head="${remote_head%%$'\t'*}"

source_branch="$base"
if [[ -n "$remote_head" ]]; then
    source_branch="$branch"
fi
authenticated_git fetch --no-tags origin "refs/heads/$source_branch"
revision="$(git rev-parse FETCH_HEAD)"
if [[ -n "$expected_head" && "$revision" != "$expected_head" ]]; then
    die "head branch $branch changed during preparation"
fi
retry=false
if [[ -n "$remote_head" ]]; then
    previous_run="$(git show -s --format='%(trailers:key=Docs-Translate-Run,valueonly)' "$revision")"
    previous_title="$(git show -s --format='%s' "$revision")"
    if [[ "$previous_run" == "$run_id" && "$previous_title" == "$title" ]]; then
        retry=true
        revision="$(git rev-parse "$revision^")"
    elif [[ -z "$pr" ]]; then
        die "branch $branch already exists. Supply its open PR number or choose another branch"
    fi
elif [[ "$base" == "${GITHUB_REF_NAME:-}" && -n "${GITHUB_SHA:-}" ]]; then
    git merge-base --is-ancestor "$GITHUB_SHA" "$revision" ||
        die "workflow revision is no longer on $base"
    revision="$GITHUB_SHA"
fi

scratch="$(mktemp -d)"
cleanup() {
    if [[ -f "$scratch/checkout/.git" ]]; then
        git worktree remove --force "$scratch/checkout"
    fi
    rm -rf -- "$scratch"
}
trap cleanup EXIT

summarize() {
    if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
        {
            printf '## Docs translation\n\n'
            printf -- '- Source: `%s` at `%s`\n- Target: `%s`\n- Result: %s\n' \
                "$source_path" "$revision" "$target_path" "$1"
            printf '\n'
            bash "$SCRIPT_DIR/render-run-stats.sh" "$stats_file"
        } >> "$GITHUB_STEP_SUMMARY"
    fi
}

if [[ "$retry" == false ]]; then
    mkdir "$scratch/translation"
    git archive "$revision" docs | tar -x -C "$scratch/translation"
    source_file="$scratch/translation/$source_path"
    [[ -f "$source_file" && "$(realpath -- "$source_file")" == "$source_file" ]] ||
        die "source page is missing or is not a regular file at $revision: $source_path"
    [[ -n "${CI_LLM_API_KEY:-}" && -n "${CI_LLM_BASE_URL:-}" ]] ||
        die "CI_LLM_API_KEY and CI_LLM_BASE_URL are required"

    mkdir -p "$scratch/translation/.kuku/skills" "$scratch/translation/.kuku/agents"
    cp -R -- "$WORKSPACE_ROOT/.kuku/skills/docs-translate" "$scratch/translation/.kuku/skills/"
    cp -- "$SCRIPT_DIR/agent.md" "$scratch/translation/.kuku/agents/docs-translation.md"
    prompt="$(<"$SCRIPT_DIR/prompts/translate-page-and-open-pr.txt")"
    prompt="${prompt//\{\{SOURCE_PATH\}\}/$source_path}"
    prompt="${prompt//\{\{TARGET_PATH\}\}/$target_path}"
    prompt="${prompt//\{\{TARGET_LOCALE\}\}/$locale}"
    run_status=0
    KUKU_ENV_FILE=/dev/null KUKU_RUN_ID="$run_id" KUKU_CHECK_RUN=1 KUKU_CHECK_SESSION_ID= \
    KUKU_CHECK_OUTPUT_MODE=response KUKU_CHECK_PROMPT="$prompt" KUKU_CHECK_WORKSPACE="$scratch/translation" \
    KUKU_CHECK_PRESET=docs-translation \
        bash "$SCRIPT_DIR/run-local-check.sh" >/dev/null || run_status=$?
    if [[ "$run_status" != 0 ]]; then
        summarize "Model run exited with status $run_status"
        exit "$run_status"
    fi

    translated="$scratch/translation/$target_path"
    [[ -s "$translated" && -f "$translated" && "$(realpath -- "$translated")" == "$translated" ]] ||
        die "model did not produce a regular non-empty page at $target_path"
    git worktree add --detach "$scratch/checkout" "$revision"
    destination="$scratch/checkout/$target_path"
    [[ "$(realpath -m -- "$destination")" == "$destination" ]] ||
        die "target page must resolve inside the checkout: $target_path"
    mkdir -p -- "$(dirname -- "$destination")"
    cp -- "$translated" "$destination"
    git -C "$scratch/checkout" --literal-pathspecs add -- "$target_path"
    if git -C "$scratch/checkout" diff --cached --quiet; then
        printf 'Translation is unchanged: %s\n' "$target_path"
        summarize "Page unchanged${pr_url:+ · $pr_url}"
        exit 0
    fi
    git -C "$scratch/checkout" diff --cached --check
    if [[ -n "$pr" ]]; then
        gh pr view "$pr" --json state,baseRefName,headRefName |
            jq -e --arg base "$base" --arg branch "$branch" \
                '.state == "OPEN" and .baseRefName == $base and .headRefName == $branch' >/dev/null ||
            die "PR $pr changed during translation"
    fi
    git -C "$scratch/checkout" commit -m "$title" --trailer "Docs-Translate-Run: $run_id"
    if [[ -n "$expected_head" ]]; then
        git -C "$scratch/checkout" merge-base --is-ancestor "$expected_head" HEAD ||
            die "translation commit must preserve the remote branch history"
    fi
    authenticated_git -C "$scratch/checkout" push \
        --force-with-lease="refs/heads/$branch:$expected_head" origin "HEAD:refs/heads/$branch"
fi

if [[ -z "$pr_url" ]]; then
    printf 'Sync the %s translation of `%s` to `%s`.\n\nSource revision: `%s`.\n' \
        "$locale" "$source_path" "$target_path" "$revision" > "$scratch/pr-body.md"
    pr_url="$(bash "$SCRIPT_DIR/open-pr-from-body.sh" --head "$branch" --base "$base" \
        --title "$title" --body-file "$scratch/pr-body.md")"
fi
printf 'Translation PR: %s\n' "$pr_url"
summarize "$pr_url"
