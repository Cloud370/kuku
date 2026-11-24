#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf -- "$TMP_DIR"' EXIT

mkdir -p "$TMP_DIR/bin" "$TMP_DIR/checkout/scripts" "$TMP_DIR/checkout/.kuku/skills/docs-translate"
cp -R -- "$SCRIPT_DIR" "$TMP_DIR/checkout/scripts/"
printf 'Translate the selected page.\n' > "$TMP_DIR/checkout/.kuku/skills/docs-translate/SKILL.md"

cat > "$TMP_DIR/bin/docker" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
for arg; do
    case "$arg" in
        cargo) exit 0 ;;
        *:/translation) workspace="${arg%:/translation}" ;;
        *:/run/kuku-docs-translate.key:ro) key_file="${arg%:/run/kuku-docs-translate.key:ro}" ;;
        GH_TOKEN*|GITHUB_TOKEN*|CI_LLM_API_KEY*) exit 99 ;;
    esac
done
[[ -f "$workspace/docs/en/start/new.md" && ! -e "$workspace/.git" ]]
[[ -f "$workspace/.kuku/agents/docs-translation.md" && -s "$key_file" ]]
printf 'model\n' >> "$RUN_LOG"
if [[ "${MODEL_RESULT:-}" == "waiting" ]]; then
    exit 3
fi
if [[ "${MODEL_RESULT:-}" != "unchanged" ]]; then
    mkdir -p "$workspace/docs/zh/start"
    printf '%s\n' "${MODEL_TEXT:-Translated}" > "$workspace/docs/zh/start/new.md"
fi
printf 'Unrelated\n' > "$workspace/docs/en/start/new.md"
if [[ -n "${RACE_REF:-}" ]]; then
    if [[ -n "${RACE_COMMIT:-}" ]]; then
        git --git-dir "$TEST_ORIGIN" update-ref "$RACE_REF" "$RACE_COMMIT"
    else
        git --git-dir "$TEST_ORIGIN" update-ref -d "$RACE_REF"
    fi
fi
EOF

cat > "$TMP_DIR/bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$1 $2" in
    "pr list")
        if [[ -f "$PR_CREATED" ]]; then
            printf 'https://example.invalid/pull/7\n'
        fi
        ;;
    "pr create")
        printf 'created\n' >> "$RUN_LOG"
        touch "$PR_CREATED"
        printf 'https://example.invalid/pull/7\n'
        ;;
    "pr view")
        printf '%s\n' "$PR_RECORD"
        ;;
    *) exit 99 ;;
esac
EOF
chmod +x "$TMP_DIR/bin/docker" "$TMP_DIR/bin/gh"
export PATH="$TMP_DIR/bin:$PATH" RUN_LOG="$TMP_DIR/run.log" PR_CREATED="$TMP_DIR/created"
export KUKU_ENV_FILE=/dev/null KUKU_CACHE_ROOT="$TMP_DIR/cache" GITHUB_STEP_SUMMARY="$TMP_DIR/summary"
export GH_TOKEN=fixture CI_LLM_API_KEY=fixture CI_LLM_BASE_URL=https://example.invalid/v1
export TEST_ORIGIN="$TMP_DIR/origin"

git init -q --initial-branch=main "$TMP_DIR/checkout"
git init -q --bare --initial-branch=main "$TMP_DIR/origin"
cd "$TMP_DIR/checkout"
git config user.name Fixture
git config user.email fixture@example.invalid
mkdir -p docs/en/start docs/zh/start
printf '# Docs\n' > docs/zh/index.md
printf '# Old page\n' > docs/en/start/old.md
printf '# Removal\n' > docs/en/start/removed.md
git add .
git commit -qm setup
initial="$(git rev-parse HEAD)"
git remote add origin "$TMP_DIR/origin"
git push -q origin main
git switch -qc source
git mv docs/en/start/{old,new}.md
git rm -q docs/en/start/removed.md
git commit -qm source
source_revision="$(git rev-parse HEAD)"
git push -q origin source
git switch -q main

runner=scripts/docs-translate/translate-page-and-open-pr.sh
branch=docs-translate/zh/start-new-test
KUKU_RUN_ID=test bash "$runner" --locale zh --base source docs/en/start/new.md
git fetch -q origin "$branch"
[[ "$(git diff-tree --no-commit-id --name-only -r FETCH_HEAD)" == docs/zh/start/new.md ]]
[[ "$(git show FETCH_HEAD:docs/zh/start/new.md)" == Translated ]]
[[ "$(git status --porcelain)" == "" ]]
[[ "$(git branch --show-current)" == main ]]

KUKU_RUN_ID=test bash "$runner" --locale zh --base source docs/en/start/new.md
[[ "$(<"$RUN_LOG")" == $'model\ncreated' ]]

export PR_RECORD='{"state":"OPEN","isCrossRepository":false,"baseRefName":"source","headRefName":"docs-translate/zh/start-new-test","url":"https://example.invalid/pull/7"}'
MODEL_RESULT=unchanged KUKU_RUN_ID=unchanged bash "$runner" --locale zh --pr 7 docs/en/start/new.md
[[ "$(git ls-remote origin "refs/heads/$branch" | cut -f1)" == "$(git rev-parse FETCH_HEAD)" ]]
MODEL_TEXT=revised KUKU_RUN_ID=update bash "$runner" --locale zh --pr 7 docs/en/start/new.md
git fetch -q origin "$branch"
[[ "$(git show FETCH_HEAD:docs/zh/start/new.md)" == revised ]]
[[ "$(grep -c '^created$' "$RUN_LOG")" == 1 ]]

status=0
MODEL_RESULT=waiting KUKU_RUN_ID=waiting bash "$runner" --locale zh --base source docs/en/start/new.md || status=$?
[[ "$status" == 3 ]]
[[ -z "$(git ls-remote origin refs/heads/docs-translate/zh/start-new-waiting)" ]]

race_ref=refs/heads/docs-translate/zh/start-new-race
if RACE_REF="$race_ref" RACE_COMMIT="$source_revision" KUKU_RUN_ID=race \
    bash "$runner" --locale zh --base source docs/en/start/new.md; then
    printf 'expected newly created remote head to block publish\n' >&2
    exit 1
fi
[[ "$(git ls-remote origin "$race_ref" | cut -f1)" == "$source_revision" ]]
if RACE_REF="refs/heads/$branch" MODEL_TEXT=race KUKU_RUN_ID=deleted \
    bash "$runner" --locale zh --pr 7 docs/en/start/new.md; then
    printf 'expected deleted remote head to block publish\n' >&2
    exit 1
fi
[[ -z "$(git ls-remote origin "refs/heads/$branch")" ]]

if GITHUB_REF_NAME=source GITHUB_SHA="$initial" KUKU_RUN_ID=pinned \
    bash "$runner" --locale zh docs/en/start/new.md; then
    printf 'expected missing page at the dispatch revision to fail\n' >&2
    exit 1
fi
[[ -z "$(git ls-remote origin refs/heads/docs-translate/zh/start-new-pinned)" ]]

bash scripts/docs-translate/report-docs-en-changes.sh "$initial" "$source_revision"
grep -Fq '| `docs/en/start/new.md` | `docs/zh/start/new.md` |' "$GITHUB_STEP_SUMMARY"
grep -Fqx -- '- `docs/en/start/old.md`' "$GITHUB_STEP_SUMMARY"
grep -Fqx -- '- `docs/en/start/removed.md`' "$GITHUB_STEP_SUMMARY"
GITHUB_STEP_SUMMARY="$TMP_DIR/initial-report" \
    bash scripts/docs-translate/report-docs-en-changes.sh "" "$source_revision"
grep -Fq '| `docs/en/start/new.md` | `docs/zh/start/new.md` |' "$TMP_DIR/initial-report"
if bash scripts/docs-translate/report-docs-en-changes.sh missing "$source_revision" 2>/dev/null; then
    printf 'expected missing diff base to fail\n' >&2
    exit 1
fi

printf 'translation workflow tests passed\n'
