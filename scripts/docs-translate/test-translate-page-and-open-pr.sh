#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf -- "$TMP_DIR"' EXIT

fixture_scripts="$TMP_DIR/checkout/scripts/docs-translate"
mkdir -p "$TMP_DIR/bin" "$fixture_scripts/prompts" "$TMP_DIR/checkout/.kuku/skills/docs-translate"
cp -- "$SCRIPT_DIR"/{translate-page-and-open-pr,run-local-check,check-in-container,open-pr-from-body,render-run-stats,report-docs-en-changes}.sh \
    "$SCRIPT_DIR"/{agent.md,config.toml} "$fixture_scripts/"
cp -- "$SCRIPT_DIR/prompts/translate-page-and-open-pr.txt" "$fixture_scripts/prompts/"
printf 'Translate the selected page.\n' > "$TMP_DIR/checkout/.kuku/skills/docs-translate/SKILL.md"

cat > "$TMP_DIR/bin/docker" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${!#}" == kuku-app ]]; then
    exit 0
fi
if [[ "${!#}" == kuku ]]; then
    for arg; do
        case "$arg" in
            *:/workspace/docs:ro) docs="${arg%:/workspace/docs:ro}" ;;
            KUKU_DOCS_INPUT_TREE=*) tree="${arg#*=}" ;;
        esac
    done
    [[ -f "$docs/zh/${MODEL_PAGE:-start/new.md}" ]]
    [[ "$tree" == "$(git -C "$docs/.." write-tree --prefix=docs/)" ]]
    printf 'validate\n' >> "$RUN_LOG"
    [[ "${FAIL_VALIDATION:-0}" == 0 ]] || exit 42
    exit 0
fi
[[ "${!#}" == /run/kuku-docs-translate/check-in-container.sh ]]
for arg; do
    case "$arg" in
        "$PWD:"*|"$PWD/scripts/docs-translate:"*) exit 99 ;;
        *:/translation) workspace="${arg%:/translation}" ;;
        *:/workspace/docs:ro) smoke_docs="${arg%:/workspace/docs:ro}" ;;
        *:/run/kuku-docs-translate/check-in-container.sh:ro) entrypoint="${arg%:*:ro}" ;;
        *:/run/kuku-docs-translate/config.toml:ro) config="${arg%:*:ro}" ;;
        *:/run/kuku-docs-translate/agents/docs-translation.md:ro) agent="${arg%:*:ro}" ;;
        *:/run/kuku-docs-translate/skills/docs-translate/SKILL.md:ro) skill="${arg%:*:ro}" ;;
        *:/run/kuku-docs-translate.key:ro) key_file="${arg%:/run/kuku-docs-translate.key:ro}" ;;
        KUKU_CHECK_PROMPT=*) prompt="${arg#*=}" ;;
        GH_TOKEN*|GITHUB_TOKEN*|CI_LLM_API_KEY*) exit 99 ;;
    esac
done
[[ -f "$entrypoint" && -f "$config" && -f "$agent" && -f "$skill" ]]
if [[ -z "${KUKU_CHECK_WORKSPACE:-}" ]]; then
    [[ "$smoke_docs" == "$PWD/docs" && -f "$smoke_docs/AGENTS.md" && -z "${workspace:-}" ]]
    exit 0
fi
page="${MODEL_PAGE:-start/new.md}"
[[ -f "$workspace/docs/en/$page" && ! -e "$workspace/.git" && -z "${smoke_docs:-}" ]]
[[ "$prompt" == *"\`docs/en/$page\`"* && "$prompt" == *"\`docs/zh/$page\`"* ]]
[[ -s "$key_file" ]]
printf 'model\n' >> "$RUN_LOG"
if [[ "${MODEL_RESULT:-}" == "waiting" ]]; then
    exit 3
fi
if [[ "${MODEL_RESULT:-}" != "unchanged" ]]; then
    mkdir -p "$(dirname -- "$workspace/docs/zh/$page")"
    printf '%s\n' "${MODEL_TEXT:-Translated}" > "$workspace/docs/zh/$page"
fi
printf 'Unrelated\n' > "$workspace/docs/en/$page"
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
if [[ "$1" == pr ]]; then
    [[ "$GH_REPO" == github.example.invalid/fixture/docs ]]
fi
case "$1 $2" in
    "repo view")
        if [[ "$3" == "$TEST_ORIGIN" ]]; then
            printf '{"id":"docs","url":"https://github.example.invalid/fixture/docs"}\n'
        else
            printf '{"id":"other"}\n'
        fi
        ;;
    "pr list")
        if [[ -f "$PR_CREATED" ]]; then
            printf 'https://example.invalid/pull/7\n'
        fi
        ;;
    "pr create")
        [[ "${FAIL_CREATE:-0}" == 0 ]] || exit 43
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
export GH_TOKEN=fixture GH_REPO=github.example.invalid/other/docs
export CI_LLM_API_KEY=fixture CI_LLM_BASE_URL=https://example.invalid/v1
export TEST_ORIGIN="$TMP_DIR/origin"

git init -q --initial-branch=main "$TMP_DIR/checkout"
git init -q --bare --initial-branch=main "$TMP_DIR/origin"
cd "$TMP_DIR/checkout"
git config user.name Fixture
git config user.email fixture@example.invalid
mkdir -p docs/en/start docs/zh/start
printf '# Docs rules\n' > docs/AGENTS.md
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
literal_page='start/help&{{TARGET_PATH}}.md'
printf '# Help and FAQ\n' > "docs/en/$literal_page"
git add -- "docs/en/$literal_page"
git commit -qm source
source_revision="$(git rev-parse HEAD)"
git push -q origin source
git switch -q main

runner=scripts/docs-translate/translate-page-and-open-pr.sh
branch=docs-translate/zh/start-new-test
KUKU_CHECK_RUN=0 bash scripts/docs-translate/run-local-check.sh
mkdir "$TMP_DIR/scratch"
ln -s "$TMP_DIR/scratch" "$TMP_DIR/scratch-link"
git fetch -q origin main
status=0
TMPDIR="$TMP_DIR/scratch-link" KUKU_RUN_ID=test FAIL_CREATE=1 \
    bash "$runner" --locale zh --base source docs/en/start/new.md || status=$?
[[ "$status" == 43 && ! -f "$PR_CREATED" ]]
[[ "$(git rev-parse FETCH_HEAD)" == "$initial" ]]
git fetch -q origin "$branch"
[[ "$(git diff-tree --no-commit-id --name-only -r FETCH_HEAD)" == docs/zh/start/new.md ]]
[[ "$(git show FETCH_HEAD:docs/zh/start/new.md)" == Translated ]]
[[ "$(git status --porcelain)" == "" ]]
[[ "$(git branch --show-current)" == main ]]

KUKU_RUN_ID=test bash "$runner" --locale zh --base source docs/en/start/new.md
KUKU_RUN_ID=test bash "$runner" --locale zh --base source docs/en/start/new.md
[[ "$(<"$RUN_LOG")" == $'model\nvalidate\ncreated' ]]

export PR_RECORD='{"state":"OPEN","isCrossRepository":false,"baseRefName":"source","headRefName":"docs-translate/zh/start-new-test","url":"https://example.invalid/pull/7"}'
MODEL_RESULT=unchanged KUKU_RUN_ID=unchanged bash "$runner" --locale zh --pr 7 docs/en/start/new.md
[[ "$(git ls-remote origin "refs/heads/$branch" | cut -f1)" == "$(git rev-parse FETCH_HEAD)" ]]
[[ "$(grep -c '^validate$' "$RUN_LOG")" == 1 ]]
MODEL_TEXT=revised KUKU_RUN_ID=update bash "$runner" --locale zh --pr 7 docs/en/start/new.md
git fetch -q origin "$branch"
[[ "$(git show FETCH_HEAD:docs/zh/start/new.md)" == revised ]]
[[ "$(grep -c '^created$' "$RUN_LOG")" == 1 ]]

MODEL_PAGE="$literal_page" KUKU_RUN_ID=literal-prompt \
    bash "$runner" --locale zh --base source --branch docs-translate/literal-prompt "docs/en/$literal_page"
git fetch -q origin docs-translate/literal-prompt
[[ "$(git show "FETCH_HEAD:docs/zh/$literal_page")" == Translated ]]

git config remote.origin.pushurl "$TMP_DIR/other"
if KUKU_RUN_ID=wrong-remote bash "$runner" --locale zh --base source docs/en/start/new.md; then
    printf 'expected mismatched GitHub repositories to block publish\n' >&2
    exit 1
fi
git config --unset remote.origin.pushurl

status=0
MODEL_RESULT=waiting KUKU_RUN_ID=waiting bash "$runner" --locale zh --base source docs/en/start/new.md || status=$?
[[ "$status" == 3 ]]
[[ -z "$(git ls-remote origin refs/heads/docs-translate/zh/start-new-waiting)" ]]

status=0
FAIL_VALIDATION=1 KUKU_RUN_ID=invalid-docs bash "$runner" --locale zh --base source docs/en/start/new.md || status=$?
[[ "$status" == 42 ]]
[[ -z "$(git ls-remote origin refs/heads/docs-translate/zh/start-new-invalid-docs)" ]]

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
